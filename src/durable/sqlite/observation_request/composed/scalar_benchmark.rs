//! Same-binary representation diagnostic, not an end-to-end B/E comparison.
//! Run alone in release mode with RUSTX_SCALAR_BENCH_OUTPUT pointing to JSON:
//! cargo test --release --lib request_scalar_reader_representation_benchmark -- \
//!   --ignored --nocapture --test-threads=1
use super::{Connection, Root, RootKind, bounded, fragments, params, read, root_checksum, scalar};
use crate::durable::observation::{ObservationValueChunk, RequestObservationPage};
use crate::runtime::read_layout::SeekableObservationSource;
use serde_json::{Value, json};
use std::{hint::black_box, path::Path, time::Instant};

const PAIRS: usize = 7;
const READS: usize = 32;
const DENSE_BYTES: usize = 16_383;

#[derive(Clone, Copy)]
enum Operation {
    Field,
    Page32,
    RootPreview,
    Range257,
}
impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Field => "field",
            Self::Page32 => "page32",
            Self::RootPreview => "root_preview",
            Self::Range257 => "range257",
        }
    }
}
struct Paths {
    field: Vec<String>,
    properties: Vec<String>,
}
impl Paths {
    fn new() -> Self {
        let properties = ["tools", "0", "input_schema", "properties"]
            .map(str::to_owned)
            .to_vec();
        let mut field = properties.clone();
        field.push("f0031".into());
        Self { field, properties }
    }
}
enum Output {
    Field(super::super::FieldWithKey),
    Page(RequestObservationPage),
    Range(ObservationValueChunk),
}
impl Output {
    fn oracle(&self) -> Value {
        match self {
            Self::Field(value) => serde_json::to_value(value).unwrap(),
            Self::Page(value) => serde_json::to_value(value).unwrap(),
            Self::Range(value) => json!({
                "bytes":value.bytes,"end_offset":value.end_offset,
                "readable_end":value.readable_end,"next_offset":value.next_offset
            }),
        }
    }
}
fn execute(connection: &Connection, id: &str, operation: Operation, paths: &Paths) -> Output {
    match operation {
        Operation::Field => Output::Field(read::field(connection, id, &paths.field).unwrap()),
        Operation::Page32 => {
            Output::Page(read::page(connection, id, &paths.properties, 0, 32).unwrap())
        }
        Operation::RootPreview => Output::Field(read::field(connection, id, &[]).unwrap()),
        Operation::Range257 => Output::Range(read::range(connection, id, &[], 17, 257).unwrap()),
    }
}
fn fixture(dense: bool) -> (Value, usize) {
    let mut value = json!({
        "request_id":"same-original-request", "model":"scalar-diagnostic",
        "messages":[], "parameters":{"temperature":0.5,"max_tokens":256},
        "tools":[{"name":"dense_tool","description":"","input_schema":{
            "type":"object","properties":{}
        }}]
    });
    let mut count = 0;
    loop {
        let key = format!("f{count:04}");
        value["tools"][0]["input_schema"]["properties"]
            .as_object_mut()
            .unwrap()
            .insert(key.clone(), json!({"type":"string"}));
        if dense && serde_json::to_vec(&value).unwrap().len() > DENSE_BYTES {
            value["tools"][0]["input_schema"]["properties"]
                .as_object_mut()
                .unwrap()
                .remove(&key);
            break;
        }
        count += 1;
        if !dense && count == 32 {
            break;
        }
    }
    if dense {
        let remaining = DENSE_BYTES - serde_json::to_vec(&value).unwrap().len();
        value["tools"][0]["description"] = Value::String("x".repeat(remaining));
        assert_eq!(serde_json::to_vec(&value).unwrap().len(), DENSE_BYTES);
    }
    (value, count)
}
fn install(path: &Path, source: &SeekableObservationSource) {
    let connection = Connection::open(path).unwrap();
    // Minimal native empty-selection identity seats for the actual composed
    // Reader. This diagnostic does not model native history or store-open cost.
    connection.execute_batch("PRAGMA foreign_keys=ON;
        CREATE TABLE native_input_nodes(id INTEGER PRIMARY KEY);
        CREATE TABLE request_snapshots(observation_source_id TEXT PRIMARY KEY,input_root INTEGER,input_root_digest BLOB,input_count INTEGER NOT NULL);
        CREATE TABLE content_frontiers(source_kind TEXT,source_id TEXT,block_index INTEGER,representation TEXT,revision INTEGER,readable_end INTEGER);").unwrap();
    super::create_schema(&connection).unwrap();
    let (original, start, end) = scalar::capture(&connection, source).unwrap();
    assert!(matches!(original, scalar::Scalar::Original { .. }));
    let (indexed, indexed_start, indexed_end) = fragments::scalar(&connection, source).unwrap();
    assert_eq!((start, end), (indexed_start, indexed_end));
    for (sequence, scalar) in [
        (1, original),
        (2, scalar::Scalar::Indexed { fragment: indexed }),
    ] {
        let root = Root {
            kind: RootKind::Request,
            sequence,
            native: None,
            source_hash: None,
            length: scalar.length(),
            scalar,
            messages_start: start,
            messages_end: end,
        };
        let bytes = bounded(&root, 16_384).unwrap();
        connection
            .execute(
                "INSERT INTO request_snapshots VALUES(?1,NULL,NULL,0)",
                [sequence.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO request_source_roots VALUES(?1,?2,?3)",
                params![
                    sequence.to_string(),
                    &bytes,
                    root_checksum(&bytes).as_slice()
                ],
            )
            .unwrap();
        connection.execute("INSERT INTO content_frontiers VALUES('trace_request',?1,0,'canonical_json_v1',1,?2)", params![sequence.to_string(), root.length]).unwrap();
    }
}
fn clock_ticks_per_second() -> Option<u64> {
    let output = std::process::Command::new("getconf")
        .arg("CLK_TCK")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|ticks| *ticks > 0)
}
fn process_cpu_ticks() -> Option<u64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let mut fields = stat.rsplit_once(") ")?.1.split_whitespace();
    let user = fields.nth(11)?.parse::<u64>().ok()?;
    let system = fields.next()?.parse::<u64>().ok()?;
    user.checked_add(system)
}
fn nanos(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_nanos()).unwrap()
}
fn sample(
    connection: &Connection,
    id: &str,
    operation: Operation,
    paths: &Paths,
    open_ns: u64,
) -> Value {
    let cpu_before = process_cpu_ticks();
    let start = Instant::now();
    let output = execute(connection, id, operation, paths);
    black_box(&output);
    let wall_ns = nanos(start);
    let cpu_after = process_cpu_ticks();
    // JSON conversion and destruction of the returned payload happen outside
    // Reader latency. CPU has tick resolution and includes the counter reads.
    json!({"read_wall_ns":wall_ns,"connection_open_wall_ns":open_ns,
        "process_cpu_ticks":cpu_after.zip(cpu_before).and_then(|(after,before)| after.checked_sub(before))})
}

#[test]
#[ignore = "release-only paired request scalar Reader diagnostic; emits JSON"]
fn request_scalar_reader_representation_benchmark() {
    assert!(
        !cfg!(debug_assertions),
        "run this diagnostic with --release"
    );
    let output = std::env::var_os("RUSTX_SCALAR_BENCH_OUTPUT")
        .expect("set RUSTX_SCALAR_BENCH_OUTPUT to a JSON output path");
    let directory = tempfile::tempdir().unwrap();
    let paths = Paths::new();
    let operations = [
        Operation::Field,
        Operation::Page32,
        Operation::RootPreview,
        Operation::Range257,
    ];
    let mut fixtures = Vec::new();
    let mut samples = Vec::new();
    for (name, dense) in [("small", false), ("near_16k_dense", true)] {
        let (value, property_count) = fixture(dense);
        let source = SeekableObservationSource::new(&value).unwrap();
        let length = source.read(&[], 0, 1).unwrap().readable_end;
        assert!(length <= 16_384);
        if dense {
            assert_eq!(length, DENSE_BYTES as u64);
        }
        let exact = source.indexed_span(0, length, 16_384).unwrap();
        let mut nodes = 0;
        source
            .visit_field_geometry(|_| {
                nodes += 1;
                Ok(())
            })
            .unwrap();
        let path = directory.path().join(format!("{name}.sqlite"));
        install(&path, &source);
        {
            let connection = Connection::open(&path).unwrap();
            for operation in operations {
                let original = execute(&connection, "1", operation, &paths);
                let indexed = execute(&connection, "2", operation, &paths);
                assert_eq!(
                    original.oracle(),
                    indexed.oracle(),
                    "{name}: {}",
                    operation.name()
                );
                if let Output::Range(range) = &original {
                    assert_eq!(range.bytes, exact[17..274]);
                }
                if let Output::Page(page) = &original {
                    assert_eq!(page.total, property_count as u64);
                    assert_eq!(page.fields.len(), 32);
                    for (i, field) in page.fields.iter().enumerate() {
                        assert_eq!(field.path.last().unwrap(), &format!("f{i:04}"));
                        assert_eq!(field.value, json!({"type":"string"}));
                    }
                }
            }
        }
        fixtures.push(json!({"name":name,"original_bytes":length,"geometry_nodes":nodes,"schema_properties":property_count}));
        for reopened in [false, true] {
            let connection = Connection::open(&path).unwrap();
            for operation in operations {
                // Warm mode starts with both representations read once. Every
                // reopened read instead starts with a fresh SQLite connection.
                if !reopened {
                    for id in ["1", "2"] {
                        black_box(execute(&connection, id, operation, &paths));
                    }
                }
                for pair in 0..PAIRS {
                    let order = if pair % 2 == 0 {
                        [("original", "1"), ("indexed", "2")]
                    } else {
                        [("indexed", "2"), ("original", "1")]
                    };
                    for (position, (representation, id)) in order.into_iter().enumerate() {
                        let mut reads = Vec::with_capacity(READS);
                        for _ in 0..READS {
                            if reopened {
                                let started = Instant::now();
                                let fresh = Connection::open(&path).unwrap();
                                let open_ns = nanos(started);
                                reads.push(sample(&fresh, id, operation, &paths, open_ns));
                            } else {
                                reads.push(sample(&connection, id, operation, &paths, 0));
                            }
                        }
                        samples.push(json!({"fixture":name,"state":if reopened { "reopened" } else { "warm" },
                            "operation":operation.name(),"pair":pair,"order_in_pair":position,
                            "representation":representation,"reads":reads}));
                    }
                }
            }
        }
    }
    let report = json!({
        "format":"rustx-scalar-reader-diagnostic-v1",
        "comparison":"Original versus forced Indexed inside the same candidate E binary; not B/E end-to-end performance",
        "build":"release test binary; existing cfg(test) instrumentation is active in both representations",
        "fixture_storage":"minimal empty native selection seats, actual composed Reader and authenticated scalar pages",
        "pairs":PAIRS,"reads_per_representation_per_pair":READS,"fixtures":fixtures,
        "clock_ticks_per_second":clock_ticks_per_second(),
        "cpu_source":"/proc/self/stat utime+stime; null if unavailable; tick resolution can produce zero; process-wide counters include other threads and counter-read overhead",
        "timing_scope":"Reader call and return only; excludes fixture construction, oracle comparison, connection open/close, payload destruction, and JSON output; reopened has one fresh connection per read, not a cold OS page cache",
        "samples":samples
    });
    let output = Path::new(&output);
    if let Some(parent) = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(output, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    eprintln!("RUSTX_SCALAR_BENCH_OUTPUT={}", output.display());
}
