//! One fragment's bounded-subtree proof, independent of how geometry is routed.
use super::{
    ConversationStoreError, Fact, FactSpool, FieldMetadata, ObservationByteReader,
    persist_encoded_fact, persist_key_fact, source_bytes, storage,
};

#[derive(Default)]
pub(super) struct Capture {
    pub(super) max_path: u64,
    pub(super) max_key_path: u64,
    validated_subtree: Option<(u64, u64)>,
}
impl Capture {
    pub(super) fn field(
        &mut self,
        spool: &mut FactSpool<'_>,
        reader: &ObservationByteReader<'_>,
        field: &FieldMetadata<'_>,
        start: u64,
    ) -> Result<(), ConversationStoreError> {
        let path = field.path;
        let key_span = field.key_span;
        let field_start = field.start;
        let field_end = field.end;
        self.max_path = self.max_path.max(field.encoded_path.len() as u64);
        let span = field_end - field_start;
        let nodes = if span > 16384 {
            33
        } else {
            // Geometry alone cannot prove numeric or serde-depth admission.
            // Each fragment validates its maximal bounded subtrees in preorder.
            if !self
                .validated_subtree
                .is_some_and(|(start, end)| field_start >= start && field_end <= end)
            {
                super::super::super::original_node_count(&source_bytes(
                    reader,
                    field_start,
                    field_end,
                )?)?;
                self.validated_subtree = Some((field_start, field_end));
            }
            field.recursive_nodes
        };
        let fact = Fact {
            path: path.to_vec(),
            parent: field.parent.map(|(path, _, _)| path.to_vec()),
            ordinal: field.parent.map(|(_, _, ordinal)| ordinal),
            start: field_start - start,
            end: field_end - start,
            children: field.children,
            nodes,
            object: field.object,
            container: field.container,
            key: None,
            name: key_span
                .filter(|(start, end)| end - start <= 256)
                .map(|_| {
                    field
                        .key
                        .map(str::to_owned)
                        .ok_or_else(|| storage("original field key unavailable"))
                })
                .transpose()?,
        };
        let fact = if path.last().is_some_and(|part| part.starts_with('@')) {
            Fact {
                key: Some(
                    key_span
                        .map(|(start, end)| end - start)
                        .ok_or_else(|| storage("original aliased key span unavailable"))?,
                ),
                ..fact
            }
        } else {
            fact
        };
        persist_encoded_fact(
            spool,
            &fact,
            field.encoded_path,
            field.parent.map(|(_, encoded, ordinal)| (encoded, ordinal)),
        )?;
        if let Some((key_start, key_end)) = key_span
            && path.last().is_some_and(|part| part.starts_with('@'))
        {
            self.max_key_path = self.max_key_path.max(persist_key_fact(
                spool,
                path,
                key_start - start,
                key_end - start,
            )?);
        }
        Ok(())
    }
}
