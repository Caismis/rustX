use super::*;
use crate::app_server::protocol::{self, JsonRpcVersion, Method, Request, RequestId, Response};

#[allow(unsafe_code)]
mod allocation_count {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! {
        static COUNT: Cell<Option<usize>> = const { Cell::new(None) };
    }
    struct Allocator;
    #[global_allocator]
    static ALLOCATOR: Allocator = Allocator;
    // The system allocator owns every pointer; the thread-local counter only
    // observes allocation calls and never allocates itself.
    unsafe impl GlobalAlloc for Allocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let _ = COUNT.try_with(|count| count.set(count.get().map(|n| n + 1)));
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { System.dealloc(ptr, layout) }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            let _ = COUNT.try_with(|count| count.set(count.get().map(|n| n + 1)));
            unsafe { System.realloc(ptr, layout, size) }
        }
    }
    pub(super) fn start() {
        COUNT.with(|count| count.set(Some(0)));
    }
    pub(super) fn finish() -> usize {
        COUNT.with(|count| count.replace(None).unwrap())
    }
}

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
    protocol::ServerRequest,
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

#[test]
fn notification_tag_probe_work() {
    use crate::app_server::protocol::{NotificationMethod, ProtocolMessage};
    use crate::observation::structural_json::plan::{take_property_work, take_tag_probes};
    use crate::observation::{
        content::ContentRef, cut::ContentBinding, transcript::RuntimeClientTranscriptPage,
    };
    use crate::runtime_client::RuntimeClientEvent;
    let template = super::super::schema::fixtures()
        .into_iter()
        .find_map(|fixture| match fixture {
            ProtocolMessage::Notification(value)
                if matches!(value.notification, NotificationMethod::Event { .. }) =>
            {
                Some(value)
            }
            _ => None,
        })
        .unwrap();
    for count in [4, 16] {
        let mut notification = template.clone();
        if let NotificationMethod::Event { target, event, .. } = &mut notification.notification {
            let reference = std::sync::Arc::new(ContentRef::configuration_instructions(
                target.conversation_id.clone(),
                12,
                73,
            ));
            let page = RuntimeClientTranscriptPage {
                content: (0..count)
                    .map(|index| ContentBinding {
                        source_path: None,
                        path: vec!["entries".into(), index.to_string(), "content".into()],
                        content_ref: reference.clone(),
                        deferred: false,
                    })
                    .collect::<Vec<_>>()
                    .into(),
                ..Default::default()
            };
            *event = std::sync::Arc::new(RuntimeClientEvent::ReadDomainsUpdated {
                projection_head: None,
                transcript: Box::new(page),
                trace: None,
                occupancy: None,
                todos: None,
            });
        }
        let _ =
            super::super::transport::serialize_record(&bounded_envelope(&notification)).unwrap();
        take_tag_probes();
        take_property_work();
        take_native_passes();
        allocation_count::start();
        let bytes =
            super::super::transport::serialize_record(&bounded_envelope(&notification)).unwrap();
        let allocations = allocation_count::finish();
        let probes = take_tag_probes();
        let (lookups, comparisons) = take_property_work();
        let passes = take_native_passes();
        let decoded: Value = serde_json::from_str(&bytes).unwrap();
        let page = &decoded["params"]["event"]["transcript"];
        assert!(page.is_object());
        assert!(page["content"][0]["content_ref"].is_object());
        let page_bytes = serde_json::to_vec(page).unwrap().len();
        let reference_bytes = serde_json::to_vec(&page["content"][0]["content_ref"])
            .unwrap()
            .len();
        println!(
            "bindings={count} notifications=1 bytes={} page_bytes={page_bytes} repeated_reference_bytes={} native_passes={passes} allocations={allocations} tag_probes={probes} property_lookups={lookups} property_comparisons={comparisons}",
            bytes.len(),
            reference_bytes * count
        );
        assert_eq!(passes, 1);
        assert_eq!(
            bytes.as_bytes(),
            legacy_bytes(&notification).unwrap().as_slice()
        );
    }
}

#[test]
#[ignore = "requires locally captured notification frames"]
fn captured_notification_work() {
    use crate::observation::structural_json::plan::{take_property_work, take_tag_probes};
    use std::io::BufRead;
    let root = std::path::PathBuf::from(std::env::var_os("RUSTX_T1_CAPTURE").unwrap());
    for scenario in ["text", "tool"] {
        let directory = root.join(scenario);
        let windows: Value =
            serde_json::from_slice(&std::fs::read(directory.join("windows.json")).unwrap())
                .unwrap();
        let total = |window: usize| -> usize {
            windows[window]["notifications"]
                .as_object()
                .unwrap()
                .values()
                .map(|n| usize::try_from(n.as_u64().unwrap()).unwrap())
                .sum()
        };
        let (start, end) = (total(0), total(1));
        let input = std::io::BufReader::new(
            std::fs::File::open(directory.join("server-1.client.jsonl")).unwrap(),
        );
        let mut count = 0;
        let (mut bytes, mut allocations, mut probes, mut lookups, mut comparisons, mut passes) =
            (0, 0, 0, 0, 0, 0);
        for line in input.lines() {
            let line = line.unwrap();
            let value: Value = serde_json::from_str(&line).unwrap();
            if value.get("method").is_none() {
                continue;
            }
            if count == end {
                break;
            }
            let notification: protocol::Notification = serde_json::from_str(&line).unwrap();
            take_tag_probes();
            take_property_work();
            take_native_passes();
            allocation_count::start();
            let encoded =
                super::super::transport::serialize_record(&bounded_envelope(&notification))
                    .unwrap();
            let n = allocation_count::finish();
            let branch_work = take_tag_probes();
            let (property_lookups, property_comparisons) = take_property_work();
            let native_passes = take_native_passes();
            assert_eq!(encoded.as_bytes(), line.as_bytes());
            if count >= start {
                bytes += encoded.len();
                allocations += n;
                probes += branch_work;
                lookups += property_lookups;
                comparisons += property_comparisons;
                passes += native_passes;
            }
            count += 1;
        }
        assert_eq!(count, end);
        assert_eq!(passes, end - start);
        println!(
            "{scenario} turns=32 notifications={} bytes={bytes} native_passes={passes} allocations={allocations} tag_probes={probes} property_lookups={lookups} property_comparisons={comparisons}",
            end - start
        );
    }
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

#[test]
fn every_typed_fixture_matches_native_serialization_oracle() {
    use protocol::{ProtocolMessage, ServerMessage};
    for fixture in super::super::schema::fixtures() {
        match fixture {
            ProtocolMessage::Request(value) => assert_wire(value.as_ref()),
            ProtocolMessage::Response(value) => assert_wire(&value),
            ProtocolMessage::Notification(value) => assert_wire(&value),
            ProtocolMessage::ServerMessage(ServerMessage::Request(value)) => assert_wire(&value),
            _ => {}
        }
    }
}
