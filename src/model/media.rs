//! Immutable request-preparation media resources. Pure materialization is shared
//! by hypothetical context prices and the final frozen provider-neutral request.
use crate::message::types::{MessageBlock, UserContentBlock};
use crate::model::uploads::{UploadProjection, UploadProjectionResolver};
use crate::model::{
    input::ModelInputMessage,
    projected::{ModelMessageView, SemanticTransformation},
};
use crate::runtime::{
    CancellationSignal,
    identity::{ArtifactId, ConversationId},
};
use crate::tools::{artifacts::ArtifactStore, types::ToolResultContent};
use std::collections::{BTreeMap, BTreeSet};

/// Scoped to one logical generation (or idle compaction operation), never history.
#[derive(Clone, Debug, Default)]
pub(crate) struct MediaSnapshot {
    uploads: UploadProjection,
    images: super::images::ResolvedImages,
    costs: BTreeMap<ArtifactId, u64>,
    enabled: Option<bool>,
    conversation: Option<ConversationId>,
}

pub(crate) fn check_cancelled(signal: &CancellationSignal) -> Result<(), String> {
    if signal.is_cancelled() {
        Err("media preparation cancelled".into())
    } else {
        Ok(())
    }
}
fn image_ids<'a>(
    messages: impl IntoIterator<Item = &'a ModelInputMessage>,
) -> BTreeSet<ArtifactId> {
    let mut ids = BTreeSet::new();
    for message in messages {
        match message.view() {
            ModelMessageView::User(content) => {
                for block in content {
                    if let UserContentBlock::Image(image) = block {
                        ids.insert(image.artifact_id.clone());
                    }
                }
            }
            ModelMessageView::Tool(_, result) => {
                for block in &result.content {
                    if let ToolResultContent::Image(image) = block {
                        ids.insert(image.artifact_id.clone());
                    }
                }
            }
            _ => {}
        }
    }
    ids
}
// A fixed occurrence frontier, including duplicates, implements the exact
// reverse image-budget rule without copying any User/Tool message body.
fn newest_image_ids<'a>(
    messages: impl IntoIterator<Item = &'a ModelInputMessage>,
) -> BTreeSet<ArtifactId> {
    let mut newest = std::collections::VecDeque::with_capacity(16);
    let mut remember = |id: &ArtifactId| {
        if newest.len() == 16 {
            newest.pop_front();
        }
        newest.push_back(id.clone());
    };
    for message in messages {
        match message.view() {
            ModelMessageView::User(content) => {
                for block in content {
                    if let UserContentBlock::Image(image) = block {
                        remember(&image.artifact_id);
                    }
                }
            }
            ModelMessageView::Tool(_, result) => {
                for block in &result.content {
                    if let ToolResultContent::Image(image) = block {
                        remember(&image.artifact_id);
                    }
                }
            }
            _ => {}
        }
    }
    newest.into_iter().collect()
}
impl MediaSnapshot {
    /// Capture the union needed by historical-only and complete primary views.
    /// Prefix deletion cannot promote an omitted image: its newer sixteen
    /// occurrences also survive. Fixed core inputs keep their surviving order.
    /// Upload rendering is message-local. Neither pure transform resamples I/O.
    pub(crate) fn capture<'a, H, C>(
        previous: Option<&Self>,
        historical: H,
        complete: C,
        artifacts: Option<&ArtifactStore>,
        enabled: Option<bool>,
        owner: Option<&dyn UploadProjectionResolver>,
        signal: &CancellationSignal,
    ) -> Result<Self, String>
    where
        H: Clone + IntoIterator<Item = &'a ModelInputMessage>,
        C: Clone + IntoIterator<Item = &'a ModelInputMessage>,
        H::IntoIter: ExactSizeIterator,
        C::IntoIter: ExactSizeIterator,
    {
        check_cancelled(signal)?;
        if historical
            .clone()
            .into_iter()
            .len()
            .max(complete.clone().into_iter().len())
            > 8192
        {
            return Err("media input exceeds item budget".into());
        }
        let conversation = artifacts.map(|store| store.conversation_id().clone());
        if previous.is_some_and(|p| p.conversation != conversation || p.enabled != enabled) {
            return Err("media snapshot ownership changed".into());
        }
        let mut snapshot = previous.cloned().unwrap_or_default();
        snapshot.enabled = enabled;
        snapshot.conversation = conversation;
        snapshot.capture_uploads(historical.clone(), complete.clone(), owner, signal)?;
        check_cancelled(signal)?;
        snapshot.validate_render_budget(historical.clone())?;
        snapshot.uploads.validate(historical.clone())?;
        check_cancelled(signal)?;
        snapshot.validate_render_budget(complete.clone())?;
        snapshot.uploads.validate(complete.clone())?;
        check_cancelled(signal)?;
        let mut required_images = BTreeSet::new();
        if enabled == Some(true) {
            // Upload rendering preserves every User image and its order. The
            // image transform retains exactly the newest sixteen User/Tool
            // occurrences; its resource set needs no rendered history copy.
            required_images.extend(newest_image_ids(historical));
            check_cancelled(signal)?;
            required_images.extend(newest_image_ids(complete));
        }
        check_cancelled(signal)?;
        if required_images.len() > 32 {
            return Err("media snapshot exceeds image union budget".into());
        }
        snapshot.images.retain(|id, _| required_images.contains(id));
        snapshot.costs.retain(|id, _| required_images.contains(id));
        for id in required_images {
            check_cancelled(signal)?;
            if !snapshot.images.contains_key(&id) {
                #[cfg(test)]
                WORK.with(|work| {
                    let mut value = work.get();
                    value.reads += 1;
                    work.set(value);
                });
                let bytes = artifacts
                    .ok_or("image artifact owner unavailable")?
                    .read_bounded(&id)
                    .map_err(|e| e.to_string())?;
                check_cancelled(signal)?;
                let cost = super::images::measure_png(&bytes)?.input_tokens();
                snapshot.costs.insert(id.clone(), cost);
                snapshot.images.insert(id, bytes);
            }
        }
        check_cancelled(signal)?;
        Ok(snapshot)
    }

    fn capture_uploads<'a>(
        &mut self,
        historical: impl Clone + IntoIterator<Item = &'a ModelInputMessage>,
        complete: impl Clone + IntoIterator<Item = &'a ModelInputMessage>,
        owner: Option<&dyn UploadProjectionResolver>,
        signal: &CancellationSignal,
    ) -> Result<(), String> {
        let mut needed = BTreeSet::new();
        for message in historical.clone().into_iter().chain(complete.clone()) {
            check_cancelled(signal)?;
            if let ModelMessageView::User(content) = message.view() {
                for block in content {
                    if let UserContentBlock::UploadedFile(file) = block {
                        needed.insert(file.clone());
                        if needed.len() > 1024 {
                            return Err("media upload references exceed item budget".into());
                        }
                    }
                }
            }
        }
        self.uploads
            .files
            .retain(|file| needed.contains(&file.file));
        let existing: BTreeSet<_> = self
            .uploads
            .files
            .iter()
            .map(|file| file.file.clone())
            .collect();
        let missing: BTreeSet<_> = needed.difference(&existing).cloned().collect();
        if !missing.is_empty() {
            let owner = owner.ok_or("unresolved Session upload")?;
            // Pass only newly needed real references to the same Session owner.
            let mut input = Vec::new();
            let mut submitted = BTreeSet::new();
            for message in historical.into_iter().chain(complete) {
                check_cancelled(signal)?;
                if !matches!(message.view(), ModelMessageView::User(content)
                    if content.iter().any(|block| matches!(block, UserContentBlock::UploadedFile(file) if missing.contains(file))))
                {
                    continue;
                }
                let mut message = message.clone();
                let content = match &mut message {
                    ModelInputMessage::Canonical(MessageBlock::User(user)) => &mut user.content,
                    ModelInputMessage::Derived(d) => match &mut d.message {
                        super::projected::ProjectedMessage::User { content, .. } => content,
                        _ => continue,
                    },
                    _ => continue,
                };
                content.retain(|b| matches!(b, UserContentBlock::UploadedFile(f) if missing.contains(f) && submitted.insert(f.clone())));
                if !content.is_empty() {
                    input.push(message);
                }
            }
            check_cancelled(signal)?;
            #[cfg(test)]
            WORK.with(|work| {
                let mut value = work.get();
                value.resolutions += 1;
                work.set(value);
            });
            let resolved = owner.resolve(&input)?;
            check_cancelled(signal)?;
            let mut seen = BTreeSet::new();
            for file in resolved.files {
                if !missing.contains(&file.file) || !seen.insert(file.file.clone()) {
                    return Err("unexpected or duplicate upload resolution".into());
                }
                if !std::path::Path::new(&file.path).is_absolute() || file.path.contains('\0') {
                    return Err("upload projection path must be a valid absolute path".into());
                }
                self.uploads.files.push(file);
            }
            if seen != missing {
                return Err("unresolved Session upload".into());
            }
        }
        let bytes = self.uploads.files.iter().try_fold(0usize, |total, file| {
            total
                .checked_add(file.path.len())?
                .checked_add(file.file.name.len())
        });
        if bytes.is_none_or(|bytes| bytes > 16 * 1024 * 1024) {
            return Err("media upload paths exceed byte budget".into());
        }
        Ok(())
    }

    fn validate_render_budget<'a>(
        &self,
        messages: impl IntoIterator<Item = &'a ModelInputMessage>,
    ) -> Result<(), String> {
        let mut budget = 16usize * 1024 * 1024;
        let files: BTreeMap<_, _> = self
            .uploads
            .files
            .iter()
            .map(|file| (&file.file, file))
            .collect();
        for message in messages {
            if let ModelMessageView::User(content) = message.view()
                && content
                    .iter()
                    .any(|block| matches!(block, UserContentBlock::UploadedFile(_)))
            {
                for block in content {
                    let bytes = match block {
                        UserContentBlock::UploadedFile(reference) => {
                            let file = files.get(reference).ok_or("unresolved Session upload")?;
                            // Count the same XML escape mapping before allocating
                            // a rendered body; plain paths are not charged sixfold.
                            super::uploads::escaped_len(&file.path)
                                .and_then(|n| {
                                    n.checked_add(super::uploads::escaped_len(&file.file.name)?)
                                })
                                .and_then(|n| n.checked_add(128))
                                .ok_or("upload rendering exceeds byte budget")?
                        }
                        UserContentBlock::Text(text) => text.text.len(),
                        _ => 0,
                    };
                    budget = budget
                        .checked_sub(bytes)
                        .ok_or("upload rendering exceeds byte budget")?;
                }
            }
        }
        Ok(())
    }

    /// Capture only actual media-bearing occurrences from the same immutable input.
    pub(crate) fn capture_selected(
        previous: Option<&Self>,
        historical: &super::SelectedModelInput,
        complete: &super::SelectedModelInput,
        artifacts: Option<&ArtifactStore>,
        enabled: Option<bool>,
        owner: Option<&dyn UploadProjectionResolver>,
        signal: &CancellationSignal,
    ) -> Result<Self, String> {
        check_cancelled(signal)?;
        if historical.len().max(complete.len()) > 8192 {
            return Err("media input exceeds item budget".into());
        }
        let historical_ranks = historical.media_indices();
        let complete_ranks = complete.media_indices();
        Self::capture(
            previous,
            historical_ranks.iter().map(|index| &historical[*index]),
            complete_ranks.iter().map(|index| &complete[*index]),
            artifacts,
            enabled,
            owner,
            signal,
        )
    }
    /// Materializes only the actual media items, preserving their original ranks.
    pub(crate) fn materialize_selected_with_changes(
        &self,
        messages: &mut super::SelectedModelInput,
        manifest: Option<&super::SelectedInputManifest>,
    ) -> Result<Vec<usize>, String> {
        if manifest.is_some_and(|manifest| manifest.len() != messages.len()) {
            return Err("media input manifest length changed".into());
        }
        let ranks = messages.media_indices();
        if ranks.is_empty() {
            return Ok(Vec::new());
        }
        let mut media: Vec<_> = ranks.iter().map(|index| messages[*index].clone()).collect();
        #[cfg(test)]
        count_copy(&media);
        let changed = self.materialize_indexed(&mut media, manifest, Some(&ranks))?;
        #[cfg(test)]
        if !changed.is_empty() {
            count_projection();
        }
        for index in &changed {
            #[cfg(test)]
            count_item_copy(&media[*index]);
            messages.replace(ranks[*index], media[*index].clone());
        }
        Ok(changed.into_iter().map(|index| ranks[index]).collect())
    }
    pub(crate) fn selected_images_input(
        &self,
        messages: &super::SelectedModelInput,
    ) -> Result<super::images::ResolvedImages, String> {
        let ranks = messages.media_indices();
        let mut count = 0usize;
        for message in ranks.iter().map(|index| &messages[*index]) {
            count += match message.view() {
                ModelMessageView::User(content) => content
                    .iter()
                    .filter(|block| matches!(block, UserContentBlock::Image(_)))
                    .count(),
                ModelMessageView::Tool(_, result) => result
                    .content
                    .iter()
                    .filter(|block| matches!(block, ToolResultContent::Image(_)))
                    .count(),
                _ => 0,
            };
        }
        if count > 16 {
            return Err("request exceeds the 16-image limit".into());
        }
        image_ids(ranks.iter().map(|index| &messages[*index]))
            .into_iter()
            .map(|id| {
                self.images
                    .get(&id)
                    .cloned()
                    .map(|bytes| (id, bytes))
                    .ok_or_else(|| "image is outside frozen media resources".into())
            })
            .collect()
    }
    pub(crate) fn image_cost_input(&self, messages: &super::SelectedModelInput) -> u64 {
        if self.enabled.is_none() {
            return 0;
        }
        image_cost_input(messages, |id| self.costs.get(id).copied())
    }
    /// Same provider-neutral content and scoped provenance as actual staging.
    #[cfg(test)]
    pub(crate) fn materialize(&self, messages: &mut [ModelInputMessage]) -> Result<(), String> {
        self.materialize_with_changes(messages, None).map(|_| ())
    }

    /// Transform only the actual selected items and retain their native input
    /// provenance. Unchanged message bodies are never copied or compared.
    #[cfg(test)]
    pub(crate) fn materialize_with_changes(
        &self,
        messages: &mut [ModelInputMessage],
        manifest: Option<&super::SelectedInputManifest>,
    ) -> Result<Vec<usize>, String> {
        self.materialize_indexed(messages, manifest, None)
    }
    fn materialize_indexed(
        &self,
        messages: &mut [ModelInputMessage],
        manifest: Option<&super::SelectedInputManifest>,
        ranks: Option<&[usize]>,
    ) -> Result<Vec<usize>, String> {
        if ranks.is_none() && manifest.is_some_and(|manifest| manifest.len() != messages.len()) {
            return Err("media input manifest length changed".into());
        }
        self.validate_render_budget(messages.iter())?;
        let mut changed = BTreeSet::new();
        let mut originals = BTreeMap::new();
        let mut failure = None;
        self.uploads.apply_with_changes(messages, |index, before| {
            changed.insert(index);
            match self.original_source(ranks.map_or(index, |ranks| ranks[index]), before, manifest)
            {
                Ok(Some(source)) => {
                    originals.insert(index, source);
                }
                Ok(None) => {}
                Err(error) => failure = Some(error),
            }
        })?;
        if let Some(error) = failure.take() {
            return Err(error);
        }
        mark_transformed(messages, originals, SemanticTransformation::UploadRendering);
        let mut originals = BTreeMap::new();
        if let Some(enabled) = self.enabled {
            super::images::project_with_changes(messages, enabled, |index, before| {
                changed.insert(index);
                match self.original_source(
                    ranks.map_or(index, |ranks| ranks[index]),
                    before,
                    manifest,
                ) {
                    Ok(Some(source)) => {
                        originals.insert(index, source);
                    }
                    Ok(None) => {}
                    Err(error) => failure = Some(error),
                }
            });
        }
        if let Some(error) = failure {
            return Err(error);
        }
        mark_transformed(messages, originals, SemanticTransformation::ImageDowngrade);
        // Never price a newly introduced/unfrozen reference as zero cost.
        if self.enabled.is_some()
            && image_ids(messages.iter())
                .iter()
                .any(|id| !self.images.contains_key(id))
        {
            return Err("image is outside frozen media resources".into());
        }
        #[cfg(test)]
        WORK.with(|work| {
            let mut current = work.get();
            current.changed_items += changed.len();
            work.set(current);
        });
        Ok(changed.into_iter().collect())
    }

    fn original_source(
        &self,
        index: usize,
        message: &ModelInputMessage,
        manifest: Option<&super::SelectedInputManifest>,
    ) -> Result<Option<super::projected::CanonicalMessageRef>, String> {
        let (Some(conversation), Some(message)) = (&self.conversation, message.as_canonical())
        else {
            return Ok(None);
        };
        if let Some(manifest) = manifest {
            return match manifest.get(index) {
                Some(super::projected::FrozenModelInput::Original(source))
                    if &source.message_id == message.id() =>
                {
                    Ok(Some(source.clone()))
                }
                _ => Err("media input differs from its native Original selection".into()),
            };
        }
        #[cfg(test)]
        WORK.with(|work| {
            let mut current = work.get();
            current.original_hashes += 1;
            work.set(current);
        });
        Ok(Some(super::projected::CanonicalMessageRef::of(
            conversation,
            message,
        )))
    }
    #[cfg(test)]
    pub(crate) fn selected_images(
        &self,
        messages: &[ModelInputMessage],
    ) -> Result<super::images::ResolvedImages, String> {
        if super::images::image_count(messages) > 16 {
            return Err("request exceeds the 16-image limit".into());
        }
        image_ids(messages)
            .into_iter()
            .map(|id| {
                self.images
                    .get(&id)
                    .cloned()
                    .map(|bytes| (id, bytes))
                    .ok_or("image is outside frozen media resources".into())
            })
            .collect()
    }
    #[cfg(test)]
    pub(crate) fn image_cost(&self, messages: &[ModelInputMessage]) -> u64 {
        if self.enabled.is_none() {
            return 0;
        }
        image_cost(messages, |id| self.costs.get(id).copied())
    }
}
fn mark_transformed(
    messages: &mut [ModelInputMessage],
    originals: BTreeMap<usize, super::projected::CanonicalMessageRef>,
    transformation: SemanticTransformation,
) {
    #[cfg(test)]
    let mut counted_projection = false;
    for (index, source) in originals {
        if let ModelInputMessage::Canonical(message) = &messages[index] {
            #[cfg(test)]
            {
                if !counted_projection {
                    count_projection();
                    counted_projection = true;
                }
                count_item_copy(&messages[index]);
            }
            messages[index] = ModelInputMessage::Derived(super::projected::DerivedModelMessage {
                message: super::projected::ProjectedMessage::from(message),
                sources: vec![source],
                checkpoint_sources: Vec::new(),
                transformations: vec![transformation],
            });
        }
    }
}

#[cfg(test)]
pub(crate) fn image_cost(
    messages: &[ModelInputMessage],
    cost: impl FnMut(&ArtifactId) -> Option<u64>,
) -> u64 {
    image_cost_iter(messages, cost)
}
pub(crate) fn image_cost_input(
    messages: &super::SelectedModelInput,
    cost: impl FnMut(&ArtifactId) -> Option<u64>,
) -> u64 {
    let ranks = messages.media_indices();
    image_cost_iter(ranks.iter().map(|index| &messages[*index]), cost)
}
fn image_cost_iter<'a>(
    messages: impl IntoIterator<Item = &'a ModelInputMessage>,
    mut cost: impl FnMut(&ArtifactId) -> Option<u64>,
) -> u64 {
    let mut total = 0u64;
    for message in messages {
        match message.view() {
            ModelMessageView::User(content) => {
                for block in content {
                    if let UserContentBlock::Image(image) = block {
                        total = total.saturating_add(cost(&image.artifact_id).unwrap_or(u64::MAX));
                    }
                }
            }
            ModelMessageView::Tool(_, result) => {
                for block in &result.content {
                    if let ToolResultContent::Image(image) = block {
                        total = total.saturating_add(cost(&image.artifact_id).unwrap_or(u64::MAX));
                    }
                }
            }
            _ => {}
        }
    }
    total
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Work {
    pub reads: usize,
    pub resolutions: usize,
    /// Copy passes over a selected input, rather than the number of items.
    pub projections: usize,
    /// Actual values copied by those passes (including replacement leaves).
    pub copied_items: usize,
    /// Logical serialized source bytes copied, not allocator/RSS measurements.
    pub copied_bytes: usize,
    pub original_hashes: usize,
    pub changed_items: usize,
}
#[cfg(test)]
thread_local! { static WORK: std::cell::Cell<Work> = const { std::cell::Cell::new(Work { reads: 0, resolutions: 0, projections: 0, copied_items: 0, copied_bytes: 0, original_hashes: 0, changed_items: 0 }) }; }
#[cfg(test)]
pub(crate) fn take_work() -> Work {
    WORK.with(|work| work.replace(Work::default()))
}
#[cfg(test)]
pub(crate) fn count_copy(messages: &[ModelInputMessage]) {
    if !messages.is_empty() {
        count_projection();
    }
    for message in messages {
        count_item_copy(message);
    }
}
#[cfg(test)]
fn count_projection() {
    WORK.with(|work| {
        let mut current = work.get();
        current.projections += 1;
        work.set(current);
    });
}
#[cfg(test)]
fn count_item_copy(message: &ModelInputMessage) {
    WORK.with(|work| {
        let mut current = work.get();
        current.copied_items += 1;
        current.copied_bytes += serde_json::to_vec(message).unwrap().len();
        work.set(current);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    mod selected_tests {
        use super::*;
        include!("media/selected_tests.rs");
    }
    use crate::context::{ContextConfig, ContextEngine, DefaultTokenEstimator, TokenEstimator};
    use crate::durable::{ConversationStore, SqliteConversationStore};
    use crate::message::{
        content::{ImageReference, TextBlock, UploadedFileRef},
        types::{InboundKind, UserMessageBlock, UserSource},
    };
    use crate::runtime::identity::MessageId;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    fn png() -> Vec<u8> {
        let mut bytes = Vec::new();
        png::Encoder::new(&mut bytes, 1, 1)
            .write_header()
            .unwrap()
            .write_image_data(&[0])
            .unwrap();
        bytes
    }
    fn user(n: usize, content: Vec<UserContentBlock>) -> ModelInputMessage {
        ModelInputMessage::Canonical(MessageBlock::User(UserMessageBlock {
            id: MessageId::new(format!("M{n}")),
            content,
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        }))
    }
    fn image(id: ArtifactId) -> UserContentBlock {
        UserContentBlock::Image(ImageReference {
            artifact_id: id,
            alt: None,
        })
    }
    #[derive(Debug)]
    struct Uploads {
        calls: Arc<AtomicUsize>,
        signal: Option<CancellationSignal>,
        path: String,
    }
    impl UploadProjectionResolver for Uploads {
        fn resolve(&self, messages: &[ModelInputMessage]) -> Result<UploadProjection, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(signal) = &self.signal {
                signal.cancel();
            }
            let mut seen = BTreeSet::new();
            Ok(UploadProjection {
                files: messages
                    .iter()
                    .filter_map(|m| match m.view() {
                        ModelMessageView::User(c) => Some(c),
                        _ => None,
                    })
                    .flat_map(|c| c.iter())
                    .filter_map(|b| match b {
                        UserContentBlock::UploadedFile(file) if seen.insert(file.clone()) => {
                            Some(super::super::uploads::ResolvedUpload {
                                file: file.clone(),
                                path: self.path.clone(),
                            })
                        }
                        _ => None,
                    })
                    .collect(),
            })
        }
    }
    fn reference() -> UserContentBlock {
        UserContentBlock::UploadedFile(UploadedFileRef {
            batch_id: "batch".into(),
            name: "file&.txt".into(),
        })
    }
    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "One controlled suffix oracle covers both image policies and accounting views"
    )]
    async fn frozen_mixed_media_suffix_prices_equal_exact_materialization_with_seventeen_images() {
        let root = tempfile::tempdir().unwrap();
        let artifacts = ArtifactStore::new(ConversationId::generate(), root.path()).unwrap();
        let mut history = Vec::new();
        for n in 0..17 {
            history.push(user(
                n,
                vec![
                    image(artifacts.put_bounded(&png()).unwrap()),
                    reference(),
                    UserContentBlock::Text(TextBlock {
                        text: format!("history {n}"),
                    }),
                ],
            ));
        }
        // Repeated artifact occurrences count independently; primary fixed
        // Context consumes image slots absent from keep-recent accounting.
        let repeated = match history[16].view() {
            ModelMessageView::User(c) => c[0].clone(),
            _ => unreachable!(),
        };
        let fixed = user(100, vec![repeated.clone(), repeated]);
        let mut complete = history.clone();
        complete.push(fixed.clone());
        let calls = Arc::new(AtomicUsize::new(0));
        let owner = Uploads {
            calls: calls.clone(),
            signal: None,
            path: "/session/file&.txt".into(),
        };
        for enabled in [true, false] {
            take_work();
            let snapshot = MediaSnapshot::capture(
                None,
                &history,
                &complete,
                Some(&artifacts),
                Some(enabled),
                Some(&owner),
                &CancellationSignal::new(),
            )
            .unwrap();
            let captured = take_work();
            assert_eq!(captured.reads, if enabled { 16 } else { 0 });
            assert_eq!(captured.resolutions, 1);
            let frozen_calls = calls.load(Ordering::SeqCst);
            let mut previous = u64::MAX;
            for boundary in 0..=history.len() {
                for core in [false, true] {
                    let mut input = history[boundary..].to_vec();
                    if core {
                        input.push(fixed.clone());
                    }
                    let mut oracle = input.clone();
                    owner.resolve(&oracle).unwrap().apply(&mut oracle).unwrap();
                    super::super::images::project(&mut oracle, enabled);
                    let images = super::super::images::resolve(&oracle, |id| {
                        artifacts.read_bounded(id).map_err(|e| e.to_string())
                    })
                    .unwrap();
                    let costs: BTreeMap<_, _> = images
                        .iter()
                        .map(|(id, b)| {
                            (
                                id.clone(),
                                super::super::images::measure_png(b).unwrap().input_tokens(),
                            )
                        })
                        .collect();
                    let exact = DefaultTokenEstimator.estimate_input(&oracle, "system", &[])
                        + image_cost(&oracle, |id| costs.get(id).copied());
                    snapshot.materialize(&mut input).unwrap();
                    assert_eq!(
                        serde_json::to_value(
                            input
                                .iter()
                                .map(ModelInputMessage::view)
                                .collect::<Vec<_>>()
                        )
                        .unwrap(),
                        serde_json::to_value(
                            oracle
                                .iter()
                                .map(ModelInputMessage::view)
                                .collect::<Vec<_>>()
                        )
                        .unwrap()
                    );
                    assert_eq!(snapshot.selected_images(&input).unwrap(), images);
                    assert_eq!(
                        DefaultTokenEstimator.estimate_input(&input, "system", &[])
                            + snapshot.image_cost(&input),
                        exact
                    );
                    if core {
                        assert!(exact <= previous);
                        previous = exact;
                    }
                }
            }
            // Every additional resolver call above belongs only to independent
            // test oracle reads, never snapshot materialization.
            assert_eq!(calls.load(Ordering::SeqCst), frozen_calls + 36);
            assert_eq!(take_work().reads, 0);
            let renewed = MediaSnapshot::capture(
                Some(&snapshot),
                &history,
                &complete,
                Some(&artifacts),
                Some(enabled),
                Some(&owner),
                &CancellationSignal::new(),
            )
            .unwrap();
            assert_eq!(renewed.images, snapshot.images);
            assert_eq!(take_work().reads, 0);
            assert_eq!(calls.load(Ordering::SeqCst), frozen_calls + 36);
        }
    }
    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "One SQLite-backed projection proves provenance and estimator isolation end to end"
    )]
    async fn derived_media_and_arbitrary_estimator_do_not_acquire_a_false_monotone_contract() {
        let root = tempfile::tempdir().unwrap();
        let conversation = ConversationId::generate();
        let artifacts = ArtifactStore::new(conversation.clone(), root.path()).unwrap();
        let original = user(
            1,
            vec![image(artifacts.put_bounded(&png()).unwrap()), reference()],
        );
        let ModelInputMessage::Canonical(source) = original else {
            unreachable!()
        };
        let derived = ModelInputMessage::Derived(crate::model::projected::DerivedModelMessage {
            message: crate::model::projected::ProjectedMessage::from(&source),
            sources: vec![crate::model::projected::CanonicalMessageRef::of(
                &conversation,
                &source,
            )],
            checkpoint_sources: vec![],
            transformations: vec![SemanticTransformation::Summary],
        });
        let calls = Arc::new(AtomicUsize::new(0));
        let owner = Arc::new(Uploads {
            calls,
            signal: None,
            path: "/session/file.txt".into(),
        });
        let store = SqliteConversationStore::in_memory(conversation).unwrap();
        store.append_canonical(&source).unwrap();
        let ModelInputMessage::Derived(body) = derived else {
            unreachable!()
        };
        let cut = store.compaction_resource_cut().unwrap();
        let operation = crate::runtime::identity::CompactionOperationId::new("derived-media");
        store
            .commit_projection(crate::durable::projection::ProjectionCommitInput {
                grant: CancellationSignal::new()
                    .authorize_compaction_commit(operation.clone())
                    .unwrap(),
                operation_id: operation,
                candidate: crate::context::checkpoint::CheckpointCandidate {
                    checkpoint_id: crate::runtime::identity::CheckpointId::new("derived-media"),
                    expected_head: None,
                    base_frontier: cut.ledger,
                    read_cut: cut,
                    items: vec![crate::context::checkpoint::CheckpointItem {
                        message: crate::context::checkpoint::CheckpointMessage::Derived(Box::new(
                            body,
                        )),
                        call_sources: vec![],
                        result_call: None,
                    }],
                    summary_metadata: None,
                    summary_generation: None,
                },
                owner: store.projection_owner(None).unwrap(),
                tokens_before: crate::runtime::types::TokenMeasurement {
                    input_tokens: 100,
                    source: crate::runtime::types::TokenMeasurementSource::Estimated,
                },
                estimated_tokens_after: 10,
                occupancy: None,
                turn_id: None,
                timestamp: chrono::Utc::now(),
            })
            .unwrap();
        let input = store.projected_history().unwrap().messages;
        let mut engine = ContextEngine::new(
            ContextConfig {
                context_window_tokens: 100_000,
                reserve_tokens: 0,
                keep_recent_tokens: 0,
            },
            Arc::new(DefaultTokenEstimator),
        )
        .unwrap();
        engine.set_image_projection(artifacts.clone(), true);
        engine.set_upload_resolver(owner.clone());
        engine
            .prepare_media(&input, &input, &CancellationSignal::new())
            .await
            .unwrap();
        assert!(engine.supports_monotone_compaction(&input));
        let mut materialized = input.to_vec();
        let images = engine.materialize_media(&mut materialized).unwrap();
        let ModelInputMessage::Derived(d) = &materialized[0] else {
            panic!("Derived provenance lost")
        };
        assert!(!d.sources.is_empty());
        assert!(
            d.transformations
                .contains(&SemanticTransformation::UploadRendering)
        );
        assert_eq!(images.len(), 1);
        let mut arbitrary = ContextEngine::new(
            *engine.config(),
            Arc::new(crate::context::ClosureTokenEstimator::new(|m, _, _| {
                if m.len() % 2 == 0 { 0 } else { 100 }
            })),
        )
        .unwrap();
        arbitrary.set_image_projection(artifacts, true);
        arbitrary.set_upload_resolver(owner);
        arbitrary
            .prepare_media(&input, &input, &CancellationSignal::new())
            .await
            .unwrap();
        assert!(!arbitrary.supports_monotone_compaction(&input));
    }
    #[test]
    fn missing_corrupt_unresolved_relative_and_expanding_media_fail_before_materialization() {
        let root = tempfile::tempdir().unwrap();
        let artifacts = ArtifactStore::new(ConversationId::generate(), root.path()).unwrap();
        let missing = vec![user(1, vec![image(ArtifactId::new("artifact_999"))])];
        assert!(
            MediaSnapshot::capture(
                None,
                &missing,
                &missing,
                Some(&artifacts),
                Some(true),
                None,
                &CancellationSignal::new()
            )
            .is_err()
        );
        let corrupt = vec![user(
            1,
            vec![image(artifacts.put_bounded(b"not a PNG").unwrap())],
        )];
        assert!(
            MediaSnapshot::capture(
                None,
                &corrupt,
                &corrupt,
                Some(&artifacts),
                Some(true),
                None,
                &CancellationSignal::new()
            )
            .is_err()
        );
        let input = vec![user(1, vec![reference()])];
        assert!(
            MediaSnapshot::capture(
                None,
                &input,
                &input,
                Some(&artifacts),
                Some(true),
                None,
                &CancellationSignal::new()
            )
            .is_err()
        );
        for path in [
            "relative".into(),
            "/invalid\0path".into(),
            format!("/{}", "a".repeat(1_000_000)),
        ] {
            let owner = Uploads {
                calls: Arc::default(),
                signal: None,
                path,
            };
            let many = vec![input[0].clone(); 20];
            assert!(
                MediaSnapshot::capture(
                    None,
                    &many,
                    &many,
                    Some(&artifacts),
                    Some(true),
                    Some(&owner),
                    &CancellationSignal::new()
                )
                .is_err()
            );
        }
    }
    #[test]
    fn cancellation_at_upload_resolution_prevents_the_first_image_read() {
        let root = tempfile::tempdir().unwrap();
        let artifacts = ArtifactStore::new(ConversationId::generate(), root.path()).unwrap();
        let signal = CancellationSignal::new();
        let input = vec![user(
            1,
            vec![reference(), image(artifacts.put_bounded(&png()).unwrap())],
        )];
        let owner = Uploads {
            calls: Arc::default(),
            signal: Some(signal.clone()),
            path: "/session/file.txt".into(),
        };
        take_work();
        assert_eq!(
            MediaSnapshot::capture(
                None,
                &input,
                &input,
                Some(&artifacts),
                Some(true),
                Some(&owner),
                &signal
            )
            .unwrap_err(),
            "media preparation cancelled"
        );
        let work = take_work();
        assert_eq!(work.resolutions, 1);
        assert_eq!(work.reads, 0);
    }
    #[test]
    fn captured_image_bytes_survive_source_change_but_a_new_capture_revalidates() {
        let root = tempfile::tempdir().unwrap();
        let artifacts = ArtifactStore::new(ConversationId::generate(), root.path()).unwrap();
        let bytes = png();
        let id = artifacts.put_bounded(&bytes).unwrap();
        let input = vec![user(1, vec![image(id.clone())])];
        let snapshot = MediaSnapshot::capture(
            None,
            &input,
            &input,
            Some(&artifacts),
            Some(true),
            None,
            &CancellationSignal::new(),
        )
        .unwrap();
        std::fs::write(
            artifacts.root().join(format!("{}.bin", id.as_str())),
            b"changed, corrupt resource",
        )
        .unwrap();
        let mut rendered = input.clone();
        snapshot.materialize(&mut rendered).unwrap();
        assert_eq!(snapshot.selected_images(&rendered).unwrap()[&id], bytes);
        take_work();
        let extended = MediaSnapshot::capture(
            Some(&snapshot),
            &input,
            &input,
            Some(&artifacts),
            Some(true),
            None,
            &CancellationSignal::new(),
        )
        .unwrap();
        assert_eq!(extended.images, snapshot.images);
        assert_eq!(take_work().reads, 0);
        assert!(
            MediaSnapshot::capture(
                None,
                &input,
                &input,
                Some(&artifacts),
                Some(true),
                None,
                &CancellationSignal::new()
            )
            .is_err()
        );
        let foreign = ArtifactStore::new(ConversationId::generate(), root.path()).unwrap();
        take_work();
        assert_eq!(
            MediaSnapshot::capture(
                Some(&snapshot),
                &input,
                &input,
                Some(&foreign),
                Some(true),
                None,
                &CancellationSignal::new()
            )
            .unwrap_err(),
            "media snapshot ownership changed"
        );
        assert_eq!(take_work().reads, 0);
    }
    #[test]
    fn bounded_tool_media_suffix_accounting_matches_exact_projection_and_is_monotone() {
        use crate::context::tokens::{DefaultTokenEstimator, TokenEstimator};
        use crate::message::types::{ContentBlockIndex, ToolCallOccurrenceRef, ToolMessageBlock};
        use crate::runtime::identity::{ToolCallId, ToolId};
        use crate::tools::types::{ToolExecutionStatus, ToolResultContent};
        let root = tempfile::tempdir().unwrap();
        let artifacts = ArtifactStore::new(ConversationId::generate(), root.path()).unwrap();
        let id = artifacts.put_bounded(&png()).unwrap();
        let history: Vec<_> = (0..17)
            .map(|n| {
                let mut result =
                    crate::scripted_suites::support::fake::success_result(&"text".repeat(10_000));
                result
                    .content
                    .push(ToolResultContent::Image(ImageReference {
                        artifact_id: id.clone(),
                        alt: None,
                    }));
                result.status = ToolExecutionStatus::Failed {
                    error: "visible status".into(),
                };
                ModelInputMessage::Canonical(MessageBlock::Tool(ToolMessageBlock {
                    id: MessageId::new(format!("tool-{n}")),
                    tool_call_id: ToolCallId::new(format!("call-{n}")),
                    occurrence: ToolCallOccurrenceRef::new(
                        MessageId::new(format!("assistant-{n}")),
                        ContentBlockIndex::new(0),
                    ),
                    tool_id: ToolId::new("tool"),
                    result,
                }))
            })
            .collect();
        for enabled in [true, false] {
            take_work();
            let snapshot = MediaSnapshot::capture(
                None,
                &history,
                &history,
                Some(&artifacts),
                Some(enabled),
                None,
                &CancellationSignal::new(),
            )
            .unwrap();
            assert_eq!(take_work().reads, usize::from(enabled));
            let mut previous = u64::MAX;
            for boundary in 0..=history.len() {
                let mut oracle = history[boundary..].to_vec();
                crate::model::images::project(&mut oracle, enabled);
                let images = crate::model::images::resolve(&oracle, |id| {
                    artifacts.read_bounded(id).map_err(|e| e.to_string())
                })
                .unwrap();
                let costs: BTreeMap<_, _> = images
                    .iter()
                    .map(|(id, bytes)| {
                        (
                            id.clone(),
                            crate::model::images::measure_png(bytes)
                                .unwrap()
                                .input_tokens(),
                        )
                    })
                    .collect();
                let exact = DefaultTokenEstimator.estimate_input(&oracle, "system", &[])
                    + image_cost(&oracle, |id| costs.get(id).copied());
                let mut actual = history[boundary..].to_vec();
                snapshot.materialize(&mut actual).unwrap();
                assert_eq!(
                    DefaultTokenEstimator.estimate_input(&actual, "system", &[])
                        + snapshot.image_cost(&actual),
                    exact
                );
                assert_eq!(snapshot.selected_images(&actual).unwrap(), images);
                assert!(
                    exact <= previous,
                    "prefix deletion must not increase the complete price"
                );
                previous = exact;
            }
            assert_eq!(take_work().reads, 0, "candidate queries never read media");
        }
    }
}
