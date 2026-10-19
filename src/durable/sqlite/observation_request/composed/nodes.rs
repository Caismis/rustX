use super::{
    Augmentation, Connection, Content, ConversationStoreError, Leaf, Template, add, bounded,
    fragments, hash, index, manifest, params, storage, weights,
};
pub(super) fn stored_hash(
    connection: &Connection,
    native: &manifest::NodeRef,
) -> Result<index::Hash, ConversationStoreError> {
    #[cfg(feature = "issue-387-profile")]
    let _profile = super::profile::Timer::new(super::profile::Phase::Authenticate);
    #[cfg(feature = "issue-387-profile")]
    super::profile::auth_call();
    #[cfg(test)]
    let _measurement = super::measurement::Measurement::new(
        crate::agent::execution::attribution::SourceScope::Authenticate,
    );
    let (digest,origin,rank):(Option<Vec<u8>>,Option<String>,u64)=connection.prepare_cached("SELECT CASE WHEN typeof(hash)='blob' AND length(hash)=32 AND native_digest=?2 THEN hash END,CASE WHEN typeof(origin_source_id)='text' AND length(CAST(origin_source_id AS BLOB))<=20 THEN origin_source_id END,origin_rank FROM request_source_nodes WHERE native_id=?1").map_err(|error|storage(error.to_string()))?.query_row(params![native.id,native.digest.as_slice()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(|error|storage(error.to_string()))?;
    let digest: index::Hash = digest
        .ok_or_else(|| storage("native selected Source augmentation unavailable"))?
        .try_into()
        .map_err(|_| storage("selected Source hash unavailable"))?;
    let origin = origin.ok_or_else(|| storage("original Source node seat unavailable"))?;
    #[cfg(feature = "issue-387-profile")]
    super::profile::origin_root_load();
    let root = super::load_root(connection, &origin)?;
    let mut at = root
        .native
        .ok_or_else(|| storage("original Source native root unavailable"))?;
    let mut expected = root
        .source_hash
        .ok_or_else(|| storage("original Source root hash unavailable"))?;
    let mut first = 0;
    for depth in 0..manifest::MAX_HEIGHT {
        if at == *native {
            if expected != digest {
                return Err(storage("original Source node capture differs"));
            }
            #[cfg(feature = "issue-387-profile")]
            super::profile::proof_visit(connection, at, expected, &root, true);
            return Ok(expected);
        }
        #[cfg(feature = "issue-387-profile")]
        super::profile::path_hop(depth + 1);
        #[cfg(not(feature = "issue-387-profile"))]
        let _ = depth;
        let node = load(connection, at, expected)?;
        #[cfg(feature = "issue-387-profile")]
        super::profile::proof_visit(connection, at, expected, &root, false);
        match node.content {
            Content::Branch {
                left,
                left_hash,
                right,
                right_hash,
            } => {
                if rank < first + left.count {
                    at = left;
                    expected = left_hash;
                } else {
                    first += left.count;
                    at = right;
                    expected = right_hash;
                }
            }
            Content::Leaf { .. } => break,
        }
    }
    Err(storage("Source node has no original capture path"))
}
pub(super) fn load(
    connection: &Connection,
    native: manifest::NodeRef,
    expected: index::Hash,
) -> Result<Augmentation, ConversationStoreError> {
    #[cfg(feature = "issue-387-profile")]
    super::profile::source_node_load();
    #[cfg(test)]
    super::WORK.with(|work| {
        let mut value = work.get();
        value.native_nodes += 1;
        work.set(value);
    });
    let actual = manifest::node(connection, &native)?;
    let bytes:Option<Vec<u8>>=connection.prepare_cached("SELECT CASE WHEN typeof(metadata)='blob' AND length(metadata)<=65536 AND native_digest=?2 AND hash=?3 THEN metadata END FROM request_source_nodes WHERE native_id=?1").map_err(|error|storage(error.to_string()))?.query_row(params![native.id,native.digest.as_slice(),expected.as_slice()],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
    let bytes = bytes.ok_or_else(|| storage("captured Source augmentation unavailable"))?;
    #[cfg(test)]
    super::record_metadata(bytes.len());
    if hash(&bytes) != expected {
        return Err(storage("captured Source node digest differs"));
    }
    let node: Augmentation =
        serde_json::from_slice(&bytes).map_err(|_| storage("captured Source node malformed"))?;
    if node.native != native
        || node.widths.len() != weights::CLASSES
        || node.widths.iter().any(|length| *length > super::SAFE)
    {
        return Err(storage("captured Source node geometry differs"));
    }
    match &node.content {
        Content::Leaf { leaf } => {
            if native.count != 1
                || native.height != 1
                || leaf.variants.len() != weights::CLASSES
                || leaf.templates.is_empty()
                || leaf.templates.len() > weights::CLASSES
                || leaf
                    .variants
                    .iter()
                    .any(|index| usize::from(*index) >= leaf.templates.len())
            {
                return Err(storage("captured Source leaf geometry differs"));
            }
        }
        Content::Branch { left, right, .. } => {
            if !matches!(actual.kind,manifest::NodeKind::Branch{left:actual_left,right:actual_right} if actual_left==*left&&actual_right==*right)
            {
                return Err(storage("Source branch differs from native selection"));
            }
        }
    }
    Ok(node)
}
fn template_width(template: &Template, class: usize) -> Result<u64, ConversationStoreError> {
    // A representative one-digit J/rank hole plus its exact width delta. The
    // source label and encoded native u64 field retain their established codec.
    let hole = hole(1, 0, template.provenance_bytes)?;
    add(
        add(template.prefix.length, template.suffix.length)?,
        add(hole.len() as u64, weights::delta(class)?)?,
    )
}
fn leaf_widths(connection: &Connection, leaf: &Leaf) -> Result<Vec<u64>, ConversationStoreError> {
    leaf.variants
        .iter()
        .enumerate()
        .map(|(class, index)| {
            template_width(
                &fragments::load_template(connection, leaf.templates[usize::from(*index)])?,
                class,
            )
        })
        .collect()
}
pub(super) fn hole(
    sequence: u64,
    rank: u64,
    encoded_bytes: u64,
) -> Result<Vec<u8>, ConversationStoreError> {
    let value = super::super::typed_value(&Some(super::super::provenance_source(
        sequence,
        rank,
        encoded_bytes,
    )))?;
    bounded(&value, 2048)
}
fn store(
    connection: &Connection,
    node: &Augmentation,
    sequence: u64,
    rank: u64,
) -> Result<index::Hash, ConversationStoreError> {
    #[cfg(test)]
    super::WORK.with(|work| {
        let mut value = work.get();
        value.new_nodes += 1;
        work.set(value);
    });
    let bytes = bounded(node, 65536)?;
    let digest = hash(&bytes);
    connection.prepare_cached("INSERT INTO request_source_nodes(native_id,native_digest,hash,metadata,origin_source_id,origin_rank) VALUES(?1,?2,?3,?4,?5,?6)").map_err(|error|storage(error.to_string()))?.execute(params![node.native.id,node.native.digest.as_slice(),digest.as_slice(),bytes,sequence.to_string(),rank]).map_err(|error|storage(error.to_string()))?;
    Ok(digest)
}
struct Capture<'a> {
    connection: &'a Connection,
    sequence: u64,
    scope: &'a crate::runtime::identity::ConversationId,
    input: &'a crate::model::SelectedModelInput,
    snapshot: &'a crate::model::RequestSnapshot,
    created: std::collections::HashMap<u64, index::Hash>,
}
impl Capture<'_> {
    fn ensure(
        &mut self,
        native: manifest::NodeRef,
        rank: u64,
    ) -> Result<index::Hash, ConversationStoreError> {
        if let Some(digest) = self.created.get(&native.id) {
            return Ok(*digest);
        }
        let connection = self.connection;
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM request_source_nodes WHERE native_id=?1)",
                [native.id],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        if exists {
            return stored_hash(connection, &native);
        }
        let node = manifest::node(connection, &native)?;
        let (content, widths, max_path, max_key_path) = match node.kind {
            manifest::NodeKind::Leaf => {
                #[cfg(test)]
                let _measurement = super::measurement::Measurement::new(
                    crate::agent::execution::attribution::SourceScope::Leaf,
                );
                #[cfg(test)]
                super::WORK.with(|work| {
                    let mut value = work.get();
                    value.new_leaves += 1;
                    work.set(value);
                });
                let at = usize::try_from(rank)
                    .map_err(|_| storage("native selected rank exceeds bound"))?;
                let message = self
                    .input
                    .get(at)
                    .ok_or_else(|| storage("actual selected input missing new leaf"))?;
                let frozen = self
                    .snapshot
                    .input
                    .get(at)
                    .ok_or_else(|| storage("actual frozen input missing new leaf"))?;
                let leaf = fragments::capture(connection, self.scope, message, frozen)?;
                let widths = leaf_widths(connection, &leaf)?;
                let mut max_path = 0;
                let mut max_key_path = 0;
                for digest in &leaf.templates {
                    let template = fragments::load_template(connection, *digest)?;
                    max_path = max_path.max(template.prefix.max_path.max(template.suffix.max_path));
                    max_key_path = max_key_path.max(
                        template
                            .prefix
                            .max_key_path
                            .max(template.suffix.max_key_path),
                    );
                }
                (Content::Leaf { leaf }, widths, max_path, max_key_path)
            }
            manifest::NodeKind::Branch { left, right } => {
                let left_hash = self.ensure(left, rank)?;
                let right_hash = self.ensure(right, rank + left.count)?;
                let a = load(connection, left, left_hash)?;
                let b = load(connection, right, right_hash)?;
                let widths = a
                    .widths
                    .iter()
                    .zip(b.widths)
                    .map(|(a, b)| add(add(*a, b)?, 1))
                    .collect::<Result<Vec<_>, _>>()?;
                (
                    Content::Branch {
                        left,
                        left_hash,
                        right,
                        right_hash,
                    },
                    widths,
                    a.max_path.max(b.max_path),
                    a.max_key_path.max(b.max_key_path),
                )
            }
        };
        let digest = store(
            connection,
            &Augmentation {
                native,
                widths,
                max_path,
                max_key_path,
                content,
            },
            self.sequence,
            rank,
        )?;
        if self.created.len()
            >= usize::try_from(manifest::MAX_ITEMS * 2)
                .map_err(|_| storage("native selection capacity unavailable"))?
        {
            return Err(storage("Source capture metadata exceeds native node bound"));
        }
        self.created.insert(native.id, digest);
        Ok(digest)
    }
}
pub(super) fn capture(
    connection: &Connection,
    sequence: u64,
    scope: &crate::runtime::identity::ConversationId,
    input: &crate::model::SelectedModelInput,
    snapshot: &crate::model::RequestSnapshot,
    selection: &manifest::Selection,
) -> Result<Option<index::Hash>, ConversationStoreError> {
    let mut capture = Capture {
        connection,
        sequence,
        scope,
        input,
        snapshot,
        created: std::collections::HashMap::new(),
    };
    selection
        .root
        .map(|root| capture.ensure(root, 0))
        .transpose()
}

pub(super) fn length(
    connection: &Connection,
    native: manifest::NodeRef,
    digest: index::Hash,
    rank: u64,
    sequence: u64,
) -> Result<u64, ConversationStoreError> {
    let node = load(connection, native, digest)?;
    let last = rank
        .checked_add(native.count)
        .and_then(|end| end.checked_sub(1))
        .ok_or_else(|| storage("native selected interval invalid"))?;
    let first_class = weights::class(sequence, rank)?;
    let last_class = weights::class(sequence, last)?;
    if first_class == last_class {
        return node
            .widths
            .get(first_class)
            .copied()
            .ok_or_else(|| storage("Source width unavailable"));
    }
    match node.content {
        Content::Branch {
            left,
            left_hash,
            right,
            right_hash,
        } => add(
            add(
                length(connection, left, left_hash, rank, sequence)?,
                length(connection, right, right_hash, rank + left.count, sequence)?,
            )?,
            1,
        ),
        Content::Leaf { .. } => Err(storage("native selected width interval invalid")),
    }
}
pub(super) struct Selected {
    pub(super) template: Template,
    pub(super) start: u64,
    pub(super) length: u64,
}
pub(super) fn select(
    connection: &Connection,
    root: manifest::NodeRef,
    digest: index::Hash,
    sequence: u64,
    wanted: u64,
) -> Result<Selected, ConversationStoreError> {
    if wanted >= root.count {
        return Err(super::invalid());
    }
    let (mut native, mut digest, mut rank, mut start) = (root, digest, 0, 0);
    for _ in 0..manifest::MAX_HEIGHT {
        let node = load(connection, native, digest)?;
        match node.content {
            Content::Leaf { leaf } => {
                let class = weights::class(sequence, wanted)?;
                let template = fragments::load_template(
                    connection,
                    *leaf
                        .templates
                        .get(usize::from(
                            *leaf
                                .variants
                                .get(class)
                                .ok_or_else(|| storage("Source variant unavailable"))?,
                        ))
                        .ok_or_else(|| storage("Source template unavailable"))?,
                )?;
                let length = add(
                    add(template.prefix.length, template.suffix.length)?,
                    hole(sequence, wanted, template.provenance_bytes)?.len() as u64,
                )?;
                return Ok(Selected {
                    template,
                    start,
                    length,
                });
            }
            Content::Branch {
                left,
                left_hash,
                right,
                right_hash,
            } => {
                if wanted < rank + left.count {
                    native = left;
                    digest = left_hash;
                } else {
                    start = add(
                        start,
                        add(length(connection, left, left_hash, rank, sequence)?, 1)?,
                    )?;
                    rank += left.count;
                    native = right;
                    digest = right_hash;
                }
            }
        }
    }
    Err(storage("Source selected traversal exceeds native height"))
}

// Full producer paths include the current occurrence prefix. Width crossings
// recurse at most the fixed decimal thresholds through the native AVL tree.
pub(super) fn validate_paths(
    connection: &Connection,
    native: manifest::NodeRef,
    digest: index::Hash,
    rank: u64,
) -> Result<(), ConversationStoreError> {
    let node = load(connection, native, digest)?;
    let last = rank
        .checked_add(native.count)
        .and_then(|end| end.checked_sub(1))
        .ok_or_else(|| storage("request rank range invalid"))?;
    if weights::digits(rank) == weights::digits(last) {
        let prefix =
            super::super::encoded_path(&["messages".into(), rank.to_string()])?.len() as u64;
        if node.max_path + prefix - 1 > 3600
            || node.max_key_path > 0 && node.max_key_path + prefix - 1 > 4096
        {
            return Err(storage("original request field path exceeds bound"));
        }
        return Ok(());
    }
    match node.content {
        Content::Branch {
            left,
            left_hash,
            right,
            right_hash,
        } => {
            validate_paths(connection, left, left_hash, rank)?;
            validate_paths(connection, right, right_hash, rank + left.count)
        }
        Content::Leaf { .. } => Err(storage("request path width interval invalid")),
    }
}
