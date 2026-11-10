use super::*;
fn connection() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "CREATE TABLE request_source_pages(id INTEGER PRIMARY KEY,hash BLOB NOT NULL,bytes BLOB NOT NULL)",
        )
        .unwrap();
    install_cache(&connection).unwrap();
    connection
}
#[test]
fn compact_page_has_exact_versioned_lexical_bytes() {
    let page = Page::Leaf {
        entries: vec![("F[]".into(), b"1.00".to_vec())],
    };
    let expected = b"RXSP\x04\x00\x01\x00\x03\x00F[]\x04\x00\x00\x001.00";
    assert_eq!(codec::encode(&page).unwrap(), expected);
    let Page::Leaf { entries } = codec::decode(expected).unwrap() else {
        panic!("leaf expected")
    };
    assert_eq!(entries, vec![("F[]".to_owned(), b"1.00".to_vec())]);
    let branch = Page::Branch {
        children: vec![Reference {
            id: 1,
            hash: [7; 32],
            last: "key".into(),
            depth: 3,
        }],
    };
    let encoded = codec::encode(&branch).unwrap();
    assert_eq!(&encoded[..14], b"RXSP\x04\x01\x01\x00\x03\x00key\x03");
    assert_eq!(&encoded[14..22], &1_u64.to_le_bytes());
    assert_eq!(&encoded[22..], &[7; 32]);
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
    for (offset, value) in [(4, 1), (5, 2), (6, 0), (6, 65)] {
        let mut changed = valid.clone();
        changed[offset] = value;
        assert!(codec::decode(&changed).is_err());
    }
    for raw in [
        b"RXSP\x04\x00\x01\x00\x41\x10".as_slice(),
        b"RXSP\x04\x00\x01\x00\x00\x00\xff\xff\xff\xff",
        b"RXSP\x04\x00\x01\x00\x01\x00\xff",
        b"RXSP\x01\x00\x01\x00\x01\x00k\x01\x00\x00\x00v",
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
                    id: 1,
                    hash: [0; 32],
                    last: "a".into(),
                    depth: 0,
                },
                Reference {
                    id: 1,
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

#[test]
fn full_keys_preserve_unicode_and_exact_leaf_and_branch_geometry() {
    let keys = ["F[\"公共路径\",\"水\"]", "F[\"公共路径\",\"汉\"]"];
    let entries = vec![
        (keys[0].into(), b"1.00".to_vec()),
        (keys[1].into(), b"18446744073709551615".to_vec()),
    ];
    let size = 8 + entries
        .iter()
        .map(|(key, value): &(String, Vec<u8>)| 6 + key.len() + value.len())
        .sum::<usize>();
    let encoded = codec::encode(&Page::Leaf {
        entries: entries.clone(),
    })
    .unwrap();
    assert_eq!(encoded.len(), size);
    let Page::Leaf { entries: decoded } = codec::decode(&encoded).unwrap() else {
        panic!("leaf")
    };
    assert_eq!(decoded, entries);
    let second = 8 + 6 + keys[0].len() + 4;
    assert_eq!(
        usize::from(u16::from_le_bytes([encoded[second], encoded[second + 1]])),
        keys[1].len()
    );
    assert_eq!(
        &encoded[second + 2..second + 2 + keys[1].len()],
        keys[1].as_bytes()
    );
    let children = keys
        .iter()
        .enumerate()
        .map(|(i, key)| Reference {
            id: 1,
            hash: [u8::try_from(i).unwrap(); 32],
            last: (*key).into(),
            depth: 0,
        })
        .collect::<Vec<_>>();
    let size = 8 + children
        .iter()
        .map(|child| 43 + child.last.len())
        .sum::<usize>();
    let encoded = codec::encode(&Page::Branch { children }).unwrap();
    assert_eq!(encoded.len(), size);
    let Page::Branch { children } = codec::decode(&encoded).unwrap() else {
        panic!("branch")
    };
    assert_eq!(
        children
            .iter()
            .map(|child| child.last.as_str())
            .collect::<Vec<_>>(),
        keys
    );
    assert_eq!(children[1].hash, [1; 32]);
}
#[test]
fn full_key_builder_and_decoder_agree_at_the_exact_page_limit() {
    for keys in [["a", "b"], ["shared/000", "shared/001"]] {
        let first = vec![1; 40_960];
        let second = vec![2; PAGE_BYTES - 8 - 12 - keys[0].len() - keys[1].len() - first.len()];
        let entries = vec![
            (keys[0].to_owned(), first.clone()),
            (keys[1].to_owned(), second.clone()),
        ];
        let mut encoded = codec::encode(&Page::Leaf { entries }).unwrap();
        assert_eq!(encoded.len(), PAGE_BYTES);
        assert!(codec::decode(&encoded).is_ok());
        encoded.push(0);
        assert!(codec::decode(&encoded).is_err());
        let mut larger = second;
        larger.push(2);
        assert!(
            codec::encode(&Page::Leaf {
                entries: vec![
                    (keys[0].into(), first.clone()),
                    (keys[1].into(), larger.clone())
                ]
            })
            .is_err()
        );
        let connection = connection();
        let mut builder = Builder::new(&connection);
        builder.push(keys[0].into(), first).unwrap();
        builder.push(keys[1].into(), larger.clone()).unwrap();
        let root = builder.finish().unwrap().unwrap();
        assert_eq!(root.depth, 1);
        assert_eq!(get(&connection, &root, keys[1]).unwrap().unwrap(), larger);
    }
}
#[test]
fn full_keys_have_no_predecessor_dependency_and_reject_the_prefix_format() {
    for tag in [0_u8, 1] {
        let mut raw = b"RXSP\x04".to_vec();
        raw.push(tag);
        raw.extend_from_slice(&2_u16.to_le_bytes());
        for key in ["水", "汉"] {
            raw.extend_from_slice(&u16::try_from(key.len()).unwrap().to_le_bytes());
            raw.extend_from_slice(key.as_bytes());
            if tag == 0 {
                raw.extend_from_slice(&0_u32.to_le_bytes());
            } else {
                raw.push(0);
                raw.extend_from_slice(&1_u64.to_le_bytes());
                raw.extend_from_slice(&[0; 32]);
            }
        }
        assert!(codec::decode(&raw).is_ok());
        let mut wrong_utf8 = raw.clone();
        wrong_utf8[10] = 0xff;
        assert!(codec::decode(&wrong_utf8).is_err());
        let mut oversized = raw.clone();
        oversized[8..10].copy_from_slice(&4161_u16.to_le_bytes());
        assert!(codec::decode(&oversized).is_err());
        for n in 0..raw.len() {
            assert!(codec::decode(&raw[..n]).is_err());
        }
        raw[4] = 2;
        assert!(codec::decode(&raw).is_err());
    }
}

#[test]
fn full_key_pages_preserve_multilevel_lookup_and_immutable_roots() {
    let connection = connection();
    let prefix = format!("catalog/{}", "雪".repeat(1380));
    let key = |n: usize| format!("{prefix}{n:012}");
    assert_eq!(key(0).len(), 4160);
    let build = |count: usize| {
        let mut builder = Builder::new(&connection);
        for n in 0..count {
            builder.push(key(n), n.to_string().into_bytes()).unwrap();
        }
        builder.finish().unwrap().unwrap()
    };
    let old = build(240);
    let current = build(241);
    assert_eq!(
        old.depth, 2,
        "full-key branch size forces a third tree level"
    );
    assert_eq!(current.depth, 2);
    for n in [0, 14, 15, 224, 225, 239] {
        assert_eq!(
            get(&connection, &old, &key(n)).unwrap().unwrap(),
            n.to_string().as_bytes()
        );
        assert_eq!(
            get(&connection, &current, &key(n)).unwrap().unwrap(),
            n.to_string().as_bytes()
        );
    }
    assert!(get(&connection, &old, &key(240)).unwrap().is_none());
    assert_eq!(
        get(&connection, &current, &key(240)).unwrap().unwrap(),
        b"240"
    );
    let mut statement = connection
        .prepare("SELECT bytes FROM request_source_pages")
        .unwrap();
    let pages = statement
        .query_map([], |row| row.get::<_, Vec<u8>>(0))
        .unwrap();

    for bytes in pages {
        let bytes = bytes.unwrap();
        let full = 8 + match codec::decode(&bytes).unwrap() {
            Page::Leaf { entries } => entries
                .iter()
                .map(|(key, value)| 6 + key.len() + value.len())
                .sum::<usize>(),
            Page::Branch { children } => children
                .iter()
                .map(|child| 43 + child.last.len())
                .sum::<usize>(),
        };
        assert!(bytes.len() <= PAGE_BYTES && full <= PAGE_BYTES);
        assert_eq!(bytes.len(), full);
        assert_eq!(
            &bytes[8..10],
            &4160_u16.to_le_bytes(),
            "every page owns its full key"
        );
    }
}
