use super::*;
use crate::durable::sqlite::observation_request::composed::{Fragment, Template, take_work};

fn template(length: u64, provenance_bytes: u64) -> Template {
    let reference = index::Reference {
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
            "CREATE TABLE request_source_pages(hash BLOB PRIMARY KEY, bytes BLOB NOT NULL)",
        )
        .unwrap();
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

fn assert_legacy_geometry(templates: &[Template]) {
    let (connection, leaf) = fixture(templates);
    take_work();
    let geometry = leaf_geometry(&connection, &leaf).unwrap();
    let work = take_work();
    assert_eq!(work.leaf_width_calls, 1);
    assert_eq!(work.template_loads, templates.len());
    assert_eq!(work.width_hole_encodes, templates.len());
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
    assert_legacy_geometry(&[template(47, u64::MAX)]);
}

#[test]
fn multiple_templates_keep_each_width_class_exact() {
    assert_legacy_geometry(&[template(47, 9), template(501, 10000), template(1, u64::MAX)]);
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
