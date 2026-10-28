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
    let expected = b"RXSP\x02\x00\x01\x00\x00\x00\x03\x00F[]\x04\x00\x00\x001.00";
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
    assert_eq!(
        &encoded[..16],
        b"RXSP\x02\x01\x01\x00\x00\x00\x03\x00key\x03"
    );
    assert_eq!(&encoded[16..], &[7; 32]);
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
        b"RXSP\x02\x00\x01\x00\x00\x00\x41\x10".as_slice(),
        b"RXSP\x02\x00\x01\x00\x00\x00\x00\x00\xff\xff\xff\xff",
        b"RXSP\x02\x00\x01\x00\x00\x00\x01\x00\xff",
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
    assert_eq!(encoded.len(), 45_136);
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

// Deliberately bypasses the encoder so malformed references and expansion bombs
// reach the decoder. Values are empty; branch children have fixed fake hashes.
fn prefix_page(tag: u8, keys: &[(u16, &[u8])]) -> Vec<u8> {
    let mut bytes = b"RXSP\x02".to_vec();
    bytes.push(tag);
    bytes.extend_from_slice(&u16::try_from(keys.len()).unwrap().to_le_bytes());
    for (prefix, suffix) in keys {
        bytes.extend_from_slice(&prefix.to_le_bytes());
        bytes.extend_from_slice(&u16::try_from(suffix.len()).unwrap().to_le_bytes());
        bytes.extend_from_slice(suffix);
        if tag == 0 {
            bytes.extend_from_slice(&0_u32.to_le_bytes());
        } else {
            bytes.push(0);
            bytes.extend_from_slice(&[0; 32]);
        }
    }
    bytes
}

#[test]
fn prefix_pages_reject_invalid_references_and_expansion() {
    for tag in [0, 1] {
        for keys in [
            vec![(1, b"x".as_slice())],
            vec![(0, b"a".as_slice()), (2, b"b".as_slice())],
            vec![(0, "水".as_bytes()), (1, "汉".as_bytes())],
            vec![(0, b"a".as_slice()), (1, b"\xff".as_slice())],
            vec![(0, b"a".as_slice()), (u16::MAX, b"".as_slice())],
        ] {
            assert!(codec::decode(&prefix_page(tag, &keys)).is_err());
        }
        let long = "x".repeat(4160);
        let mut keys = vec![(4160, b"".as_slice()); 16];
        keys[0] = (0, long.as_bytes());
        let compressed = prefix_page(tag, &keys);
        assert!(compressed.len() < 5000);
        assert!(codec::decode(&compressed).is_err(), "expanded page bound");
        assert!(codec::decode(&prefix_page(tag, &keys[..15])).is_ok());
        keys.truncate(2);
        keys[1] = (4160, b"x");
        assert!(
            codec::decode(&prefix_page(tag, &keys)).is_err(),
            "expanded key bound"
        );
    }
}

#[test]
fn prefix_pages_preserve_unicode_and_reduce_leaf_and_branch_bytes() {
    let keys = ["F[\"公共路径\",\"水\"]", "F[\"公共路径\",\"汉\"]"];
    let values = [b"1.00".as_slice(), b"18446744073709551615".as_slice()];
    let entries: Vec<(String, Vec<u8>)> = keys
        .iter()
        .zip(values)
        .map(|(key, value)| ((*key).into(), value.to_vec()))
        .collect();
    let old_bytes = 8 + entries
        .iter()
        .map(|(key, value)| 6 + key.len() + value.len())
        .sum::<usize>();
    let encoded = codec::encode(&Page::Leaf {
        entries: entries.clone(),
    })
    .unwrap();
    assert!(encoded.len() < old_bytes);
    let Page::Leaf { entries: decoded } = codec::decode(&encoded).unwrap() else {
        panic!("leaf expected")
    };
    assert_eq!(decoded, entries);
    let second = 8 + 8 + keys[0].len() + values[0].len();
    let prefix = usize::from(u16::from_le_bytes([encoded[second], encoded[second + 1]]));
    assert_eq!(prefix, "F[\"公共路径\",\"".len());
    assert!(keys[0].is_char_boundary(prefix));
    let children = keys
        .iter()
        .enumerate()
        .map(|(n, key)| Reference {
            hash: [u8::try_from(n).unwrap(); 32],
            last: (*key).into(),
            depth: 0,
        })
        .collect::<Vec<_>>();
    let old_bytes = 8 + children
        .iter()
        .map(|child| 35 + child.last.len())
        .sum::<usize>();
    let encoded = codec::encode(&Page::Branch { children }).unwrap();
    assert!(encoded.len() < old_bytes);
    let Page::Branch { children } = codec::decode(&encoded).unwrap() else {
        panic!("branch expected")
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
fn prefix_pages_builder_matches_stored_and_expanded_limits() {
    // No common prefix: the stored encoding is four bytes larger than the old
    // expanded charge. The second page must start with a zero prefix.
    let first = vec![1; 40_960];
    let second = vec![2; 24_550];
    let encoded = codec::encode(&Page::Leaf {
        entries: vec![("a".into(), first.clone()), ("b".into(), second.clone())],
    })
    .unwrap();
    assert_eq!(encoded.len(), PAGE_BYTES);
    assert!(codec::decode(&encoded).is_ok());
    let mut larger = second;
    larger.push(2);
    assert!(
        codec::encode(&Page::Leaf {
            entries: vec![("a".into(), first.clone()), ("b".into(), larger.clone())]
        })
        .is_err()
    );
    let connection = connection();
    let mut builder = Builder::new(&connection);
    builder.push("a".into(), first).unwrap();
    builder.push("b".into(), larger.clone()).unwrap();
    let root = builder.finish().unwrap().unwrap();
    assert_eq!(root.depth, 1);
    assert_eq!(get(&connection, &root, "b").unwrap().unwrap(), larger);

    // With a shared prefix the expanded limit wins, even though the stored page
    // still has space. Reject a manually extended stored value on decoding too.
    let first = vec![1; 40_960];
    let second = vec![2; 24_536];
    let entries = vec![("shared/000".into(), first), ("shared/001".into(), second)];
    let mut encoded = codec::encode(&Page::Leaf {
        entries: entries.clone(),
    })
    .unwrap();
    assert_eq!(encoded.len(), PAGE_BYTES - 5);
    assert!(codec::decode(&encoded).is_ok());
    let second_value_length = 8 + 8 + 10 + 40_960 + 4 + 1;
    encoded[second_value_length..second_value_length + 4]
        .copy_from_slice(&24_537_u32.to_le_bytes());
    encoded.push(2);
    assert!(encoded.len() < PAGE_BYTES);
    assert!(codec::decode(&encoded).is_err());
    let mut entries = entries;
    entries[1].1.push(2);
    assert!(
        codec::encode(&Page::Leaf {
            entries: entries.clone()
        })
        .is_err()
    );
    let mut builder = Builder::new(&connection);
    for (key, value) in entries {
        builder.push(key, value).unwrap();
    }
    let root = builder.finish().unwrap().unwrap();
    assert_eq!(root.depth, 1);
    assert_eq!(
        get(&connection, &root, "shared/001")
            .unwrap()
            .unwrap()
            .len(),
        24_537
    );
}

#[test]
fn prefix_pages_preserve_multilevel_lookup_and_immutable_roots() {
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
        "expanded branch size forces a third tree level"
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
    let (mut stored, mut expanded) = (0, 0);
    for bytes in pages {
        let bytes = bytes.unwrap();
        stored += bytes.len();
        let full = 8 + match codec::decode(&bytes).unwrap() {
            Page::Leaf { entries } => entries
                .iter()
                .map(|(key, value)| 6 + key.len() + value.len())
                .sum::<usize>(),
            Page::Branch { children } => children
                .iter()
                .map(|child| 35 + child.last.len())
                .sum::<usize>(),
        };
        assert!(bytes.len() <= PAGE_BYTES && full <= PAGE_BYTES);
        expanded += full;
        assert_eq!(&bytes[8..10], &[0, 0], "every page starts independently");
    }
    assert!(
        stored * 4 < expanded,
        "shared paths actually reduce persisted page bytes"
    );
}
