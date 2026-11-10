use super::*;

fn template(length: u64) -> Template {
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
        suffix: fragment,
        header_prefix: String::new(),
        header_suffix: String::new(),
        provenance_bytes: 9,
        children: 1,
        nodes: 1,
    }
}
fn connection() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "CREATE TABLE request_source_pages(hash BLOB PRIMARY KEY, bytes BLOB NOT NULL)",
        )
        .unwrap();
    connection
}

#[test]
fn captured_templates_reuse_decode_only_after_current_byte_comparison() {
    let connection = connection();
    // Include an identical pre-existing row: INSERT OR IGNORE is not evidence
    // of ownership, but exact current bytes prove reuse equally for both cases.
    store_template(&connection, &template(47)).unwrap();
    let captured = store_templates(
        &connection,
        vec![template(47), template(501)],
        vec![0; weights::CLASSES],
    )
    .unwrap();
    super::super::take_work();
    for (digest, prepared) in captured.leaf.templates.into_iter().zip(captured.templates) {
        let expected = load_template(&connection, digest).unwrap();
        let actual = load_template_reusing(&connection, digest, Some(prepared)).unwrap();
        assert_eq!(
            super::super::record::encode(&actual, 65536).unwrap(),
            super::super::record::encode(&expected, 65536).unwrap()
        );
    }
    let work = super::super::take_work();
    assert_eq!(
        (
            work.template_loads,
            work.template_decodes,
            work.template_reuses
        ),
        (4, 2, 2)
    );
}

#[test]
fn captured_templates_reject_insert_trigger_and_ignored_corrupt_rows() {
    for trigger in [false, true] {
        let connection = connection();
        if trigger {
            connection.execute_batch("CREATE TEMP TRIGGER rewrite_template AFTER INSERT ON request_source_pages BEGIN UPDATE request_source_pages SET bytes=x'7b' WHERE hash=NEW.hash; END;").unwrap();
        } else {
            let digest = store_template(&connection, &template(47)).unwrap();
            connection
                .execute(
                    "UPDATE request_source_pages SET bytes=x'7b' WHERE hash=?1",
                    [digest.as_slice()],
                )
                .unwrap();
        }
        let captured =
            store_templates(&connection, vec![template(47)], vec![0; weights::CLASSES]).unwrap();
        let digest = captured.leaf.templates[0];
        super::super::take_work();
        let failure =
            load_template_reusing(&connection, digest, captured.templates.into_iter().next())
                .unwrap_err();
        assert!(failure.to_string().contains("digest differs"));
        assert_eq!(super::super::take_work().template_reuses, 0);
        assert!(
            load_template(&connection, digest)
                .unwrap_err()
                .to_string()
                .contains("digest differs")
        );
    }
}

#[test]
fn captured_templates_read_earlier_rows_after_all_template_inserts() {
    let connection = connection();
    connection.execute_batch("CREATE TEMP TRIGGER rewrite_earlier_template AFTER INSERT ON request_source_pages BEGIN UPDATE request_source_pages SET bytes=x'7b' WHERE hash!=NEW.hash; END;").unwrap();
    let captured = store_templates(
        &connection,
        vec![template(47), template(501)],
        vec![0; weights::CLASSES],
    )
    .unwrap();
    assert!(
        load_template_reusing(
            &connection,
            captured.leaf.templates[0],
            captured.templates.into_iter().next()
        )
        .unwrap_err()
        .to_string()
        .contains("digest differs")
    );
}
