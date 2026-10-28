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
                "delivery_index":0},"roots":["/workspace"]
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
        let schema = crate::runtime_client::structural_json::root::<T>();
        crate::runtime_client::structural_json::serialize_wire(self.0, &schema, serializer)
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
