use super::*;
fn connection() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "CREATE TABLE request_source_pages(hash BLOB PRIMARY KEY,bytes BLOB NOT NULL)",
        )
        .unwrap();
    connection
}
#[test]
fn compact_page_has_exact_versioned_lexical_bytes() {
    let page = Page::Leaf {
        entries: vec![("F[]".into(), b"1.00".to_vec())],
    };
    let expected = b"RXSP\x01\x00\x01\x00\x03\x00F[]\x04\x00\x00\x001.00";
    assert_eq!(codec::encode(&page).unwrap(), expected);
    let Page::Leaf { entries } = codec::decode(expected).unwrap() else {
        panic!("leaf expected")
    };
    assert_eq!(entries, vec![("F[]".to_owned(), b"1.00".to_vec())]);
    let branch = Page::Branch {
        children: vec![Reference {
            hash: [7; 32],
            last: "key".into(),
            depth: 3,
        }],
    };
    let encoded = codec::encode(&branch).unwrap();
    assert_eq!(&encoded[..14], b"RXSP\x01\x01\x01\x00\x03\x00key\x03");
    assert_eq!(&encoded[14..], &[7; 32]);
    let Page::Branch { children } = codec::decode(&encoded).unwrap() else {
        panic!("branch expected")
    };
    assert_eq!(children[0].depth, 3);
    assert_eq!(children[0].hash, [7; 32]);
    assert_eq!(children[0].last, "key");
}
#[test]
fn compact_page_rejects_lengths_counts_versions_trailing_and_legacy() {
    let valid = codec::encode(&Page::Leaf {
        entries: vec![("k".into(), b"v".to_vec())],
    })
    .unwrap();
    for end in 0..valid.len() {
        assert!(codec::decode(&valid[..end]).is_err(), "truncation {end}");
    }
    let mut trailing = valid.clone();
    trailing.push(0);
    assert!(codec::decode(&trailing).is_err());
    for (offset, value) in [(4, 2), (5, 2), (6, 0), (6, 65)] {
        let mut changed = valid.clone();
        changed[offset] = value;
        assert!(codec::decode(&changed).is_err());
    }
    for raw in [
        b"RXSP\x01\x00\x01\x00\x41\x10".as_slice(),
        b"RXSP\x01\x00\x01\x00\x00\x00\xff\xff\xff\xff",
        b"RXSP\x01\x00\x01\x00\x01\x00\xff",
        br#"{"type":"leaf","entries":[["k","dg=="]]}"#,
    ] {
        assert!(codec::decode(raw).is_err());
    }
    assert!(codec::decode(&vec![0; PAGE_BYTES + 1]).is_err());
}
#[test]
fn compact_page_fanout_has_independent_byte_and_count_bounds() {
    for long_keys in [false, true] {
        let connection = connection();
        let mut builder = Builder::new(&connection);
        let count = if long_keys { 240 } else { 1000 };
        let key = |n| {
            if long_keys {
                format!("{n:06}{}", "x".repeat(4150))
            } else {
                format!("key{n:04}")
            }
        };
        for n in 0..count {
            builder.push(key(n), b"1".to_vec()).unwrap();
        }
        let root = builder.finish().unwrap().unwrap();
        assert_eq!(root.depth, if long_keys { 2 } else { 1 });
        let mut statement = connection
            .prepare("SELECT bytes FROM request_source_pages")
            .unwrap();
        let pages = statement
            .query_map([], |r| r.get::<_, Vec<u8>>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(pages.len(), if long_keys { 19 } else { 17 });
        for raw in pages {
            assert!(raw.len() <= PAGE_BYTES);
            match codec::decode(&raw).unwrap() {
                Page::Leaf { entries } => assert!(entries.len() <= FANOUT),
                Page::Branch { children } => assert!(children.len() <= FANOUT),
            }
        }
        for n in [0, count / 2, count - 1] {
            assert_eq!(get(&connection, &root, &key(n)).unwrap().unwrap(), b"1");
        }
        assert!(get(&connection, &root, "zzzz").unwrap().is_none());
    }
}
#[test]
fn compact_page_preserves_authentication_and_navigation_rejection() {
    let connection = connection();
    for page in [
        Page::Leaf {
            entries: vec![("b".into(), vec![]), ("a".into(), vec![])],
        },
        Page::Leaf {
            entries: vec![("a".into(), vec![]), ("a".into(), vec![])],
        },
        Page::Branch {
            children: vec![
                Reference {
                    hash: [0; 32],
                    last: "a".into(),
                    depth: 0,
                },
                Reference {
                    hash: [0; 32],
                    last: "b".into(),
                    depth: 1,
                },
            ],
        },
    ] {
        let root = persist(&connection, &page).unwrap();
        assert!(get(&connection, &root, "a").is_err());
    }
    let root = persist(
        &connection,
        &Page::Leaf {
            entries: vec![("k".into(), b"18446744073709551615".to_vec())],
        },
    )
    .unwrap();
    assert_eq!(
        get(&connection, &root, "k").unwrap().unwrap(),
        b"18446744073709551615"
    );
    connection
        .execute(
            "UPDATE request_source_pages SET bytes=x'00' WHERE hash=?1",
            [root.hash.as_slice()],
        )
        .unwrap();
    assert!(get(&connection, &root, "k").is_err());
}

#[test]
fn compact_page_checks_exact_key_value_and_payload_limits() {
    let page = Page::Leaf {
        entries: vec![("k".repeat(4160), vec![0; 40_960])],
    };
    let encoded = codec::encode(&page).unwrap();
    assert_eq!(encoded.len(), 45_134);
    let Page::Leaf { entries } = codec::decode(&encoded).unwrap() else {
        panic!("leaf expected")
    };
    assert_eq!(entries[0].0.len(), 4160);
    assert_eq!(entries[0].1.len(), 40_960);
    for page in [
        Page::Leaf {
            entries: vec![("k".repeat(4161), vec![0])],
        },
        Page::Leaf {
            entries: vec![("k".into(), vec![0; 40_961])],
        },
        Page::Leaf {
            entries: vec![("k".into(), vec![0; 1024]); 64],
        },
        Page::Leaf {
            entries: vec![("k".into(), vec![]); 65],
        },
    ] {
        assert!(codec::encode(&page).is_err());
    }
}
