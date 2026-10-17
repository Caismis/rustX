use super::*;

#[derive(Debug, PartialEq)]
struct Evidence {
    path: Vec<String>,
    encoded: String,
    parent: Option<(String, u64)>,
    start: u64,
    end: u64,
    children: u64,
    key: Option<(u64, u64)>,
    preview: Value,
}
fn evidence(
    source: &SeekableObservationSource,
    legacy: bool,
) -> Result<Vec<Evidence>, &'static str> {
    let mut fields = Vec::new();
    let visit = |path: &[String],
                 encoded: &str,
                 parent: Option<(&str, u64)>,
                 start,
                 end,
                 children,
                 key,
                 preview: &Value| {
        fields.push(Evidence {
            path: path.to_vec(),
            encoded: encoded.to_owned(),
            parent: parent.map(|(path, ordinal)| (path.to_owned(), ordinal)),
            start,
            end,
            children,
            key,
            preview: preview.clone(),
        });
        Ok(())
    };
    if legacy {
        source.visit_fields_legacy(visit)?;
    } else {
        source.visit_fields(visit)?;
    }
    Ok(fields)
}
fn deep_fixture() -> SeekableObservationSource {
    #[derive(serde::Serialize, schemars::JsonSchema)]
    struct Native {
        revision: u64,
        opaque: Value,
    }
    let mut deep = serde_json::json!({"leaf":"original水🙂","unsafe":u64::MAX});
    for depth in 0..48 {
        deep = serde_json::json!({format!("level-{depth}-水\"\\\n/~"):deep,"sibling":[depth,true]});
    }
    let opaque = serde_json::json!({
        "before": [null,{"":17,"quoted\"\\\n水":[-0.0,1.000_125]}],
        "deep": deep,
        "reserved": {"@literal":{"$key":"original"},"$key":u64::MAX},
        "k".repeat(300):"deferred original key",
        "after":(0..35).map(|ordinal|serde_json::json!({"ordinal":ordinal,"value":"sibling"})).collect::<Vec<_>>()
    });
    SeekableObservationSource::new(&Native {
        revision: u64::MAX,
        opaque,
    })
    .unwrap()
}

#[test]
fn t08_t14_t19_t20_mutable_field_paths_reuse_prefixes_with_exact_legacy_evidence() {
    let source = deep_fixture();
    PATH_WORK.with(|work| work.set((0, 0)));
    let actual = evidence(&source, false).unwrap();
    let (encoded, reused) = PATH_WORK.with(std::cell::Cell::get);
    let expected = evidence(&source, true).unwrap();
    assert_eq!(actual, expected);
    assert!(actual.iter().any(|field| field.path.len() > 40));
    assert!(
        actual
            .iter()
            .any(|field| field.path.last().is_some_and(|key| key.starts_with('@')))
    );
    assert_eq!(
        encoded,
        actual.len() - 1,
        "only each new nonroot component is encoded"
    );
    let old_components: usize = actual.iter().map(|field| field.path.len()).sum();
    assert_eq!(encoded + reused, old_components);
    assert!(
        reused > 6 * encoded,
        "deep prefix work is no longer repeated"
    );
    for field in &actual {
        assert_eq!(field.encoded, serde_json::to_string(&field.path).unwrap());
        assert!(field.encoded.len() <= 3600);
    }
    eprintln!(
        "field-path components_encoded={encoded} prefix_components_reused={reused} legacy_components={old_components}"
    );
}

fn path_boundary_source(last_bytes: usize) -> SeekableObservationSource {
    let mut path = vec!["x".repeat(195); 17];
    path.push("y".repeat(last_bytes));
    let value = path.iter().rev().fold(Value::Bool(true), |value, key| {
        Value::Object([(key.clone(), value)].into_iter().collect())
    });
    SeekableObservationSource::new(&value).unwrap()
}

#[test]
fn t08_t19_t20_incremental_field_path_encoding_keeps_the_exact_3600_byte_boundary() {
    let source = path_boundary_source(230);
    let actual = evidence(&source, false).unwrap();
    assert_eq!(actual, evidence(&source, true).unwrap());
    assert_eq!(actual.last().unwrap().encoded.len(), 3600);
    let source = path_boundary_source(231);
    assert_eq!(
        evidence(&source, false).unwrap_err(),
        "native layout path exceeds bound"
    );
    assert_eq!(
        evidence(&source, true).unwrap_err(),
        "native layout path exceeds bound"
    );
    assert_eq!(append_field_path("[]", ""), Ok("[\"\"]".into()));
    assert_eq!(
        append_field_path("[\"parent\"]", "水\"\\\n"),
        Ok(serde_json::to_string(&["parent", "水\"\\\n"]).unwrap())
    );
}
