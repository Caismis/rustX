use super::*;
use sha2::Digest as _;

fn native(
    connection: &Connection,
    id: u64,
    children: Option<(manifest::NodeRef, manifest::NodeRef)>,
) -> manifest::NodeRef {
    let mut digest = sha2::Sha256::new();
    let (count, height) = if let Some((left, right)) = children {
        digest.update(b"rustx-native-input-branch-v1\0");
        for child in [left, right] {
            digest.update(child.digest);
            digest.update(child.count.to_le_bytes());
            digest.update([child.height]);
        }
        (left.count + right.count, 1 + left.height.max(right.height))
    } else {
        digest.update(b"rustx-native-input-leaf-v1\0");
        digest.update(hash(b"null"));
        (1, 1)
    };
    let native = manifest::NodeRef {
        id,
        digest: digest.finalize().into(),
        count,
        height,
    };
    if let Some((left, right)) = children {
        connection.execute("INSERT INTO native_input_nodes(id,digest,count,height,left_id,right_id,left_digest,right_digest) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", params![id,native.digest.as_slice(),count,height,left.id,right.id,left.digest.as_slice(),right.digest.as_slice()]).unwrap();
    } else {
        connection.execute("INSERT INTO native_input_nodes(id,digest,count,height,item_digest,item_json) VALUES(?1,?2,1,1,?3,'null')", params![id,native.digest.as_slice(),hash(b"null").as_slice()]).unwrap();
    }
    native
}
fn leaf(
    connection: &Connection,
    id: u64,
    high_class: Option<usize>,
    max_path: u64,
) -> Augmentation {
    Augmentation {
        native: native(connection, id, None),
        widths: WidthTable::leaf((0..weights::CLASSES).map(|class| {
            if high_class == Some(class) {
                super::super::SAFE - 100
            } else {
                0
            }
        }))
        .unwrap(),
        max_path,
        max_key_path: 0,
        content: Content::Leaf {
            leaf: Leaf {
                templates: vec![[0; 32]],
                variants: vec![0; weights::CLASSES],
            },
        },
    }
}
fn branch(
    connection: &Connection,
    id: u64,
    left: &Augmentation,
    right: &Augmentation,
) -> Augmentation {
    let left_hash = store(connection, left.clone()).unwrap().digest;
    let right_hash = store(connection, right.clone()).unwrap().digest;
    Augmentation {
        native: native(connection, id, Some((left.native, right.native))),
        widths: WidthTable::join(&left.widths, &right.widths).unwrap(),
        max_path: left.max_path.max(right.max_path),
        max_key_path: 0,
        content: Content::Branch {
            left: left.native,
            left_hash,
            right: right.native,
            right_hash,
        },
    }
}

#[test]
fn borrowed_root_geometry_preserves_path_error_before_left_length_overflow() {
    let connection = Connection::open_in_memory().unwrap();
    manifest::create_schema(&connection).unwrap();
    super::super::create_schema(&connection).unwrap();
    // At rank 9 the left subtree selects two different width classes. Each
    // stored same-class sum is valid, but that mixed-class sum overflows. The
    // right subtree independently has an invalid producer path. Path validation
    // must finish across the whole root before length evaluation can begin.
    let a = leaf(&connection, 1, Some(0), 0);
    let b = leaf(&connection, 2, Some(1), 0);
    let c = leaf(&connection, 3, None, 3600);
    let left = branch(&connection, 4, &a, &b);
    let root = branch(&connection, 5, &left, &c);
    let proof = store(&connection, root.clone()).unwrap();
    let digest = proof.digest;
    let checked = proof.into_node(&connection, root.native).unwrap();
    let old_path = validate_paths(&connection, root.native, digest, 9)
        .unwrap_err()
        .to_string();
    let new_path = validate_node_paths(&connection, &checked, 9)
        .unwrap_err()
        .to_string();
    assert_eq!(new_path, old_path);
    assert!(new_path.contains("original request field path exceeds bound"));
    let old_length = length(&connection, root.native, digest, 9, 1)
        .unwrap_err()
        .to_string();
    let new_length = length_from_node(&connection, &checked, 9, 1)
        .unwrap_err()
        .to_string();
    assert_eq!(new_length, old_length);
    assert!(new_length.contains("composed observation byte offset exceeds bound"));
    let staged = validate_node_paths(&connection, &checked, 9)
        .and_then(|()| length_from_node(&connection, &checked, 9, 1));
    assert_eq!(staged.unwrap_err().to_string(), old_path);
}
