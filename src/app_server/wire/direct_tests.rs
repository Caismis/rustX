use super::*;
use crate::app_server::protocol::{self, JsonRpcVersion, Method, Request, RequestId, Response};

trait Native: Serialize + schemars::JsonSchema {
    fn native(&self) -> Value;
}
macro_rules! native {
    ($($ty:ty),*) => {$ (
        impl Native for $ty {
            fn native(&self) -> Value {
                <$ty>::serialize(self, serde_json::value::Serializer).unwrap()
            }
        }
    )*};
}
native!(
    Request,
    Response,
    protocol::Notification,
    crate::app_server::product_host::FileRead
);

fn legacy_bytes<T: Native>(value: &T) -> Result<Vec<u8>, String> {
    let mut value = value.native();
    if serde_json::to_vec(&value).unwrap().len() > super::super::transport::MAX_MESSAGE_BYTES {
        return Err("native record exceeds limit".into());
    }
    let schema = serde_json::to_value(schemars::schema_for!(T)).unwrap();
    convert(&mut value, &schema, &schema, true)?;
    serde_json::to_vec(&value).map_err(|error| error.to_string())
}
fn assert_wire<T: Native>(value: &T) {
    assert_eq!(
        serde_json::to_vec(value).map_err(|error| error.to_string()),
        legacy_bytes(value)
    );
}
fn request(name: String) -> Request {
    Request {
        jsonrpc: JsonRpcVersion::V2,
        id: RequestId::Integer(17),
        call: Method::Initialize(protocol::InitializeParams {
            protocol_version: protocol::APP_SERVER_PROTOCOL_VERSION,
            client: protocol::ClientIdentity {
                name,
                version: "水\n\"\\".into(),
            },
            presentation: protocol::PresentationCapabilities::default(),
        }),
    }
}

#[test]
fn direct_wire_matches_legacy_flattened_envelopes_and_nested_checkpoint() {
    let mut request = request("typed".into());
    for id in [17, -9_007_199_254_740_991, i64::MIN, i64::MAX] {
        request.id = RequestId::Integer(id);
        assert_wire(&request);
    }
    let mut checkpoint = crate::runtime_client::projection::RuntimeClientProjection::new(
        crate::runtime::identity::ConversationId::new("conv_77621431-ad5b-7155-9eca-9f21014c0bc9"),
        Vec::new(),
        serde_json::from_value(serde_json::json!({"configured_tools":[],"revision":1})).unwrap(),
        None,
        64,
    )
    .checkpoint_candidate()
    .unwrap();
    checkpoint.read_cut.through = u64::MAX;
    checkpoint.read_cut.revision = u64::MAX;
    assert_wire(&Response::Success(Box::new(protocol::Success {
        jsonrpc: JsonRpcVersion::V2,
        id: RequestId::String("checkpoint".into()),
        result: protocol::MethodResult::Checkpoint {
            checkpoint: Box::new(checkpoint),
        },
    })));
    for notification in [
        serde_json::json!({"jsonrpc":"2.0","method":"configuration/sourceRepairChanged",
            "params":{"target":{"kind":"user"},"receipt":null,"sequence":u64::MAX.to_string()}}),
        serde_json::json!({"jsonrpc":"2.0","method":"session/event","params":{
            "registration_id":"r", "target": {"session_id":"ses_00000000-0000-7000-8000-000000000001",
                "conversation_id":"conv_00000000-0000-7000-8000-000000000001",
                "runtime_incarnation":u64::MAX.to_string(),"attachment_id":"a"},
            "cursor":u64::MAX.to_string(),"event":{"type":"trace_changed"}}}),
    ] {
        let value: protocol::Notification = serde_json::from_value(notification).unwrap();
        assert_wire(&value);
    }
    let file: crate::app_server::product_host::FileRead =
        serde_json::from_value(serde_json::json!({
            "target": {"session_id":"ses_00000000-0000-7000-8000-000000000001",
                "conversation_id":"conv_00000000-0000-7000-8000-000000000001",
                "runtime_incarnation":u64::MAX.to_string(),"attachment_id":"a"},
            "source":{"kind":"session_file","locator":{"type":"by_message_id","message_id":"m"},
                "delivery_index":0},"roots":["/workspace"],"read":{"kind":"bytes","max_bytes":65536}
        }))
        .unwrap();
    assert_wire(&file);
}

#[test]
fn direct_wire_preserves_exact_native_record_limit_and_errors() {
    let empty = request(String::new());
    let overhead = serde_json::to_vec(&empty.native()).unwrap().len();
    let limit = super::super::transport::MAX_MESSAGE_BYTES;
    for extra in [0, 1] {
        let value = request("x".repeat(limit - overhead + extra));
        assert_wire(&value);
        assert_eq!(serde_json::to_vec(&value).is_ok(), extra == 0);
    }
    // Escaping consumes encoded bytes, not the source string's UTF-8 length.
    let value = request("\n".repeat((limit - overhead) / 2 + 1));
    assert_wire(&value);
    assert!(serde_json::to_vec(&value).is_err());
}

struct Direct<'a, T>(&'a T);
impl<T: Serialize + schemars::JsonSchema> Serialize for Direct<'_, T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let schema = crate::observation::structural_json::codec::<T>();
        crate::observation::structural_json::serialize_wire(self.0, &schema, serializer)
    }
}

#[test]
fn direct_wire_preserves_opaque_numbers_float_normalization_and_tuple_rules() {
    #[derive(Serialize, schemars::JsonSchema)]
    struct Row {
        exact: u64,
        opaque: Value,
        float: f32,
        tuple: (u64, String),
    }
    let source = Row {
        exact: u64::MAX,
        opaque: serde_json::json!({"wide":u64::MAX,"signed":i64::MIN,"negative":-0.0}),
        float: 1.2,
        tuple: (u64::MAX, "water 水\n".into()),
    };
    let schema = serde_json::to_value(schemars::schema_for!(Row)).unwrap();
    let mut legacy = serde_json::to_value(&source).unwrap();
    convert(&mut legacy, &schema, &schema, true).unwrap();
    assert_eq!(
        serde_json::to_vec(&Direct(&source)).unwrap(),
        serde_json::to_vec(&legacy).unwrap()
    );
    assert_eq!(legacy["exact"], u64::MAX.to_string());
    assert!(legacy["opaque"]["wide"].is_number());
    assert!(legacy["tuple"][0].is_number());
}

#[test]
fn compact_plans_match_schema_conversion_for_every_public_definition() {
    fn fixture(schema: &Value, root: &Value, depth: usize) -> Value {
        if depth == 0 {
            return Value::Null;
        }
        if let Some(reference) = schema["$ref"].as_str() {
            return fixture(
                root.pointer(reference.strip_prefix('#').unwrap()).unwrap(),
                root,
                depth - 1,
            );
        }
        if let Some(value) = schema.get("const") {
            return value.clone();
        }
        if let Some(value) = schema["enum"].get(0) {
            return value.clone();
        }
        if let Some(properties) = schema["properties"].as_object() {
            return properties
                .iter()
                .map(|(key, schema)| (key.clone(), fixture(schema, root, depth - 1)))
                .collect();
        }
        for keyword in ["oneOf", "anyOf", "allOf"] {
            if let Some(branch) = schema[keyword].get(0) {
                return fixture(branch, root, depth - 1);
            }
        }
        if schema["items"].is_object() {
            return serde_json::json!([fixture(&schema["items"], root, depth - 1)]);
        }
        match schema["type"].as_str() {
            Some("string") => Value::from("fixture"),
            Some("boolean") => Value::Bool(true),
            Some("integer") => Value::from(17),
            Some("number") => serde_json::json!(-0.0),
            _ => Value::Null,
        }
    }
    let native = serde_json::to_value(schemars::schema_for!(
        super::super::protocol::ProtocolMessage
    ))
    .unwrap();
    let mut count = 0;
    for schema in native["$defs"].as_object().unwrap().values() {
        let plan = crate::observation::structural_json::plan::Plan::new(schema, &native);
        let mut schemas = vec![schema];
        for keyword in ["oneOf", "anyOf", "allOf"] {
            if let Some(branches) = schema[keyword].as_array() {
                schemas.extend(branches);
            }
        }
        for branch in schemas {
            let source = fixture(branch, &native, 12);
            for source in [
                source,
                Value::from(u64::MAX),
                Value::from(u64::MAX.to_string()),
                serde_json::json!(-0.0),
            ] {
                for encode in [true, false] {
                    let mut expected = source.clone();
                    let mut actual = source.clone();
                    let expected_result = convert(&mut expected, schema, &native, encode);
                    let actual_result = plan
                        .root()
                        .convert_wire(&mut actual, encode)
                        .map_err(str::to_owned);
                    assert_eq!(
                        (actual_result, actual),
                        (expected_result, expected),
                        "{schema} encode={encode}"
                    );
                    count += 1;
                }
            }
        }
    }
    assert!(
        count > 1000,
        "the complete public definition vocabulary was compared"
    );
}

#[test]
fn compact_plans_preserve_export_artifact_identity() {
    let schema = format!(
        "{}\n",
        serde_json::to_string_pretty(&super::super::schema::protocol_schema()).unwrap()
    );
    assert_eq!(
        schema,
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/protocol/app-server/v78.schema.json"
        ))
    );
    let fixtures = format!(
        "{}\n",
        serde_json::to_string_pretty(&super::super::schema::fixtures()).unwrap()
    );
    assert_eq!(
        fixtures,
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/protocol/app-server/fixtures.json"
        ))
    );
}
