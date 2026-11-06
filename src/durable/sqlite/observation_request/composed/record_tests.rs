use super::super::{
    Augmentation, Content, Fact, Fragment, Leaf, Root, RootKind, Template, index, manifest,
};
use super::*;

fn fact() -> Fact {
    Fact {
        path: vec!["long_parent".into(), "opaque".into()],
        parent: Some(vec!["long_parent".into()]),
        ordinal: Some(7),
        start: 1,
        end: u64::MAX,
        children: 2,
        nodes: 33,
        object: true,
        container: true,
        key: Some(u64::MAX),
        name: Some("opaque".into()),
    }
}
fn fragment() -> Fragment {
    Fragment {
        bytes: index::Reference {
            hash: [255; 32],
            last: "00000000000000000000".into(),
            depth: 0,
        },
        fields: index::Reference {
            hash: [128; 32],
            last: "F[\"🙂\"]".into(),
            depth: 1,
        },
        length: 128,
        max_path: 19,
        max_key_path: 0,
    }
}

#[test]
fn compact_request_fact_contains_only_geometry_and_omits_preview_mirrors() {
    let original = fact();
    let bytes = encode(&original, 40960).unwrap();
    let stored: Fact = decode(&bytes).unwrap();
    assert_eq!(stored.start, original.start);
    assert_eq!(stored.end, u64::MAX);
    assert_eq!(stored.key, Some(u64::MAX));
    assert_eq!(bytes[0], 0x98, "exactly eight geometry/key fields");
    assert_eq!(stored.name, original.name);
    assert!(stored.path.is_empty() && stored.parent.is_none() && stored.ordinal.is_none());
    let mut duplicate = fact();
    duplicate.path = vec!["other".into()];
    duplicate.parent = None;
    duplicate.ordinal = Some(900);
    assert_eq!(bytes, encode(&duplicate, 40960).unwrap());
    let old = serde_json::to_vec(&serde_json::json!({
        "path":original.path,"parent":original.parent,"ordinal":original.ordinal,
        "start":original.start,"end":original.end,"children":original.children,"nodes":original.nodes,
        "object":original.object,"container":original.container,
        "key":original.key,"name":original.name,
    })).unwrap();
    assert!(
        bytes.len() * 2 < old.len(),
        "private repeated keys and coordinates removed"
    );
}

#[test]
fn compact_request_records_reject_trailing_missing_named_and_legacy_encodings() {
    let bytes = encode(&fact(), 40960).unwrap();
    for suffix in [vec![0], vec![0xc0], bytes.clone()] {
        let mut corrupt = bytes.clone();
        corrupt.extend(suffix);
        assert!(decode::<Fact>(&corrupt).is_err());
    }
    assert!(decode::<Fact>(&bytes[..bytes.len() - 1]).is_err());
    let mut missing = bytes.clone();
    assert_eq!(missing[0], 0x98);
    missing[0] = 0x97;
    assert!(decode::<Fact>(&missing).is_err());
    let mut extra = bytes.clone();
    extra[0] = 0x99;
    extra.push(0xc0);
    assert!(decode::<Fact>(&extra).is_err());
    assert!(decode::<Fact>(&rmp_serde::to_vec_named(&fact()).unwrap()).is_err());
    assert!(decode::<Fact>(&serde_json::to_vec(&fact()).unwrap()).is_err());
    let legacy = encode(
        &(
            1_u64,
            2_u64,
            0_u64,
            1_u64,
            false,
            false,
            b"null".to_vec(),
            None::<u64>,
            None::<String>,
        ),
        40960,
    )
    .unwrap();
    assert!(
        decode::<Fact>(&legacy).is_err(),
        "old nine-field preview records are obsolete"
    );
    let wrong = encode(
        &(
            "wrong",
            2_u64,
            0_u64,
            1_u64,
            false,
            false,
            None::<u64>,
            None::<String>,
        ),
        40960,
    )
    .unwrap();
    assert!(decode::<Fact>(&wrong).is_err());
}

#[test]
fn compact_request_metadata_declared_lengths_fail_before_element_allocation() {
    #[derive(Deserialize)]
    struct Text {
        value: String,
    }
    thread_local! { static ELEMENTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
    struct Element;
    impl<'de> Deserialize<'de> for Element {
        fn deserialize<D: de::Deserializer<'de>>(_: D) -> Result<Self, D::Error> {
            ELEMENTS.with(|count| count.set(count.get() + 1));
            Err(de::Error::custom("element visited"))
        }
    }
    fn list<'de, D: de::Deserializer<'de>>(decoder: D) -> Result<Vec<Element>, D::Error> {
        sequence::<_, _, 80>(decoder)
    }
    #[derive(Deserialize)]
    struct Record {
        #[serde(deserialize_with = "list")]
        values: Vec<Element>,
    }
    for bytes in [
        vec![0x91, 0xdc, 0, 81],
        vec![0x91, 0xdd, 255, 255, 255, 255],
    ] {
        assert!(decode::<Record>(&bytes).is_err());
        assert_eq!(ELEMENTS.with(std::cell::Cell::get), 0);
    }
    // This also consumes the test tuple field, keeping the derived shape explicit.
    assert_eq!(decode::<Record>(&[0x91, 0x90]).unwrap().values.len(), 0);
    assert!(decode::<Text>(&[0x91, 0xdb, 255, 255, 255, 255]).is_err());
    assert_eq!(decode::<Text>(&[0x91, 0xa0]).unwrap().value, "");
}

#[test]
fn compact_request_metadata_exact_byte_bound_is_enforced() {
    let value = fact();
    let bytes = encode(&value, 40960).unwrap();
    assert_eq!(encode(&value, bytes.len()).unwrap(), bytes);
    assert!(encode(&value, bytes.len() - 1).is_err());
}

#[test]
fn compact_request_nodes_roots_and_templates_keep_every_native_coordinate() {
    let native = manifest::NodeRef {
        id: u64::MAX,
        digest: [255; 32],
        count: 8192,
        height: 32,
    };
    let node = Augmentation {
        native,
        widths: super::super::width_table::WidthTable::leaf((0..80).map(|n| n + 100)).unwrap(),
        max_path: 3500,
        max_key_path: 4000,
        content: Content::Leaf {
            leaf: Leaf {
                variants: vec![0; 80],
                templates: vec![[128; 32]],
            },
        },
    };
    let bytes = encode(&node, 65536).unwrap();
    let decoded: Augmentation = decode(&bytes).unwrap();
    assert_eq!(decoded.native, native);
    assert_eq!(
        serde_json::to_value(&decoded).unwrap(),
        serde_json::to_value(&node).unwrap()
    );
    assert!(bytes.len() < serde_json::to_vec(&node).unwrap().len());
    let branch = Augmentation {
        content: Content::Branch {
            left: native,
            left_hash: [1; 32],
            right: manifest::NodeRef { id: 31, ..native },
            right_hash: [2; 32],
        },
        ..node
    };
    let decoded: Augmentation = decode(&encode(&branch, 65536).unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(decoded).unwrap(),
        serde_json::to_value(branch).unwrap()
    );
    let root = Root {
        kind: RootKind::Request,
        sequence: u64::MAX,
        native: Some(native),
        source_hash: Some([254; 32]),
        scalar: fragment(),
        prefix: Some(fragment()),
        children: 24,
        nodes: 33,
        messages_start: 100,
        messages_end: 102,
        length: 4096,
    };
    let decoded: Root = decode(&encode(&root, 16384).unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(decoded).unwrap(),
        serde_json::to_value(root).unwrap()
    );
    let template = Template {
        prefix: fragment(),
        suffix: fragment(),
        header_prefix: "{\"z\":".into(),
        header_suffix: ",\"a\":-0.0}".into(),
        provenance_bytes: u64::MAX,
        children: 64,
        nodes: 33,
    };
    let decoded: Template = decode(&encode(&template, 65536).unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(decoded).unwrap(),
        serde_json::to_value(template).unwrap()
    );
}
