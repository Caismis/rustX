use super::*;
use crate::durable::sqlite::observation_request::composed::{Fragment, Template, take_work};

fn template(length: u64, provenance_bytes: u64) -> Template {
    let reference = index::Reference {
        id: 1,
        hash: [0; 32],
        last: "fixture".into(),
        depth: 0,
    };
    let fragment = Fragment {
        bytes: reference.clone(),
        fields: reference,
        length,
        max_path: 17,
        max_key_path: 23,
    };
    Template {
        prefix: fragment.clone(),
        suffix: Fragment {
            max_path: 31,
            ..fragment
        },
        header_prefix: String::new(),
        header_suffix: String::new(),
        provenance_bytes,
        children: 1,
        nodes: 1,
    }
}

fn fixture(templates: &[Template]) -> (Connection, Leaf) {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "CREATE TABLE request_source_pages(id INTEGER PRIMARY KEY,hash BLOB NOT NULL, bytes BLOB NOT NULL)",
        )
        .unwrap();
    index::install_cache(&connection).unwrap();
    let hashes = templates
        .iter()
        .map(|t| fragments::store_template(&connection, t).unwrap())
        .collect();
    let leaf = Leaf {
        templates: hashes,
        variants: (0..weights::CLASSES)
            .map(|n| u8::try_from(n % templates.len()).unwrap())
            .collect(),
    };
    (connection, leaf)
}

fn assert_legacy_geometry(templates: &[Template], expected_encodes: usize) {
    let (connection, leaf) = fixture(templates);
    take_work();
    let geometry = leaf_geometry(&connection, &leaf).unwrap();
    let work = take_work();
    assert_eq!(work.leaf_width_calls, 1);
    assert_eq!(work.template_loads, templates.len());
    assert_eq!(work.width_hole_encodes, expected_encodes);
    let expected: Vec<_> = leaf
        .variants
        .iter()
        .enumerate()
        .map(|(class, index)| {
            let template = &templates[usize::from(*index)];
            let hole = hole(1, 0, template.provenance_bytes).unwrap();
            add(
                add(template.prefix.length, template.suffix.length).unwrap(),
                add(hole.len() as u64, weights::delta(class).unwrap()).unwrap(),
            )
            .unwrap()
        })
        .collect();
    assert_eq!((geometry.1, geometry.2), (31, 23));
    for (class, expected) in expected.into_iter().enumerate() {
        assert_eq!(geometry.0.length(class).unwrap(), expected);
    }
}

#[test]
fn single_template_load_and_hole_encode_cover_all_eighty_widths() {
    assert_legacy_geometry(&[template(47, u64::MAX)], 1);
}

#[test]
fn multiple_templates_keep_each_width_class_exact() {
    assert_legacy_geometry(
        &[template(47, 9), template(501, 10000), template(1, u64::MAX)],
        3,
    );
}

#[test]
fn repeated_provenance_encodes_once_for_adjacent_templates() {
    assert_legacy_geometry(&[template(47, 9), template(501, 9), template(1, 9)], 1);
    assert_legacy_geometry(&[template(47, 9), template(501, 10), template(1, 9)], 3);
}

#[test]
fn prepared_hole_length_is_reused_only_for_matching_provenance() {
    let (connection, leaf) = fixture(&[template(47, 9), template(501, 9)]);
    let expected = leaf_geometry(&connection, &leaf).unwrap();
    let length = hole(1, 0, 9).unwrap().len() as u64;
    take_work();
    let actual = leaf_geometry_reusing(&connection, &leaf, Vec::new(), Some((9, length))).unwrap();
    let work = take_work();
    assert_eq!((work.width_hole_encodes, work.width_hole_reuses), (0, 2));
    for class in 0..weights::CLASSES {
        assert_eq!(
            actual.0.length(class).unwrap(),
            expected.0.length(class).unwrap()
        );
    }
    take_work();
    let _ = leaf_geometry_reusing(&connection, &leaf, Vec::new(), Some((10, 0))).unwrap();
    let work = take_work();
    assert_eq!((work.width_hole_encodes, work.width_hole_reuses), (1, 1));
}

#[test]
fn completed_turn_hole_work() {
    use crate::durable::CanonicalStore;
    for tool in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::durable::SqliteConversationStore::open(
            crate::runtime::identity::ConversationId::generate(),
            &dir.path().join("conversation.sqlite"),
        )
        .unwrap();
        store.initialize(&[]).unwrap();
        crate::durable::sqlite::append_completed_turns(&store, 0, 8, tool);
        take_work();
        store.begin_query_work();
        crate::durable::sqlite::append_completed_turns(&store, 8, 1, tool);
        let sql = store.finish_query_work();
        let work = take_work();
        let calls: usize = sql.values().map(|entry| entry.0).sum();
        let steps: i64 = sql.values().map(|entry| entry.2).sum();
        let commits: usize = sql
            .iter()
            .filter(|(query, _)| query.starts_with("COMMIT"))
            .map(|(_, entry)| entry.0)
            .sum();
        eprintln!(
            "TURN_HOLE_WORK tool={tool} encodes={} reuses={} leaves={} sql_calls={calls} vm_steps={steps} commits={commits}",
            work.width_hole_encodes, work.width_hole_reuses, work.leaf_width_calls
        );
        for (query, (calls, rows, steps)) in sql {
            eprintln!("TURN_SQL tool={tool} calls={calls} rows={rows} steps={steps} sql={query:?}");
        }
        assert_eq!(work.width_hole_encodes, 0);
        assert!(work.width_hole_reuses > 0);
    }
}

#[test]
fn metrics_still_authenticate_original_template_bytes() {
    let (connection, leaf) = fixture(&[template(47, 9)]);
    connection
        .execute(
            "UPDATE request_source_pages SET bytes=?1",
            [b"{}".as_slice()],
        )
        .unwrap();
    assert!(
        leaf_geometry(&connection, &leaf)
            .unwrap_err()
            .to_string()
            .contains("digest differs")
    );
}

#[test]
fn invalid_variant_and_width_overflow_are_rejected() {
    let (connection, mut leaf) = fixture(&[template(47, 9)]);
    leaf.variants[79] = 1;
    assert!(leaf_geometry(&connection, &leaf).is_err());
    let (connection, leaf) = fixture(&[template(super::super::SAFE, 9)]);
    assert!(leaf_geometry(&connection, &leaf).is_err());
}
