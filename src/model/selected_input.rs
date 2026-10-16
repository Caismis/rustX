//! Persistent native ordered input. A leaf is an occurrence, not an identity set.
use std::ops::{Index, IndexMut};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

use super::input::ModelInputMessage;
use super::projected::{CanonicalMessageRef, FrozenModelInput};
use crate::runtime::identity::ConversationId;

#[cfg(test)]
mod request_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
thread_local! {
    static NODE_WORK: std::cell::Cell<(usize, usize, usize, usize)> = const { std::cell::Cell::new((0,0,0,0)) };
    static READ_SCOPE_DEPTH:std::cell::Cell<usize>=const{std::cell::Cell::new(0)};
    static ACTUAL_VISITS:std::cell::Cell<usize>=const{std::cell::Cell::new(0)};
    static PREPARATION_VISITS:std::cell::Cell<usize>=const{std::cell::Cell::new(0)};
    static MATERIALIZED_ITEMS:std::cell::Cell<usize>=const{std::cell::Cell::new(0)};
    static PREFIX_WORK: std::cell::Cell<(usize,usize)> = const {std::cell::Cell::new((0,0))};
    static HASH_WORK: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0,0)) };
}
#[cfg(test)]
pub(crate) fn take_node_work() -> (usize, usize, usize, usize) {
    NODE_WORK.with(|n| n.replace((0, 0, 0, 0)))
}
#[cfg(test)]
pub(crate) struct PreparationReads(Option<usize>);
#[cfg(test)]
impl PreparationReads {
    pub(crate) fn new() -> Self {
        let outermost = READ_SCOPE_DEPTH.with(|depth| {
            let previous = depth.get();
            depth.set(previous + 1);
            previous == 0
        });
        Self(outermost.then(|| ACTUAL_VISITS.with(std::cell::Cell::get)))
    }
}
#[cfg(test)]
impl Drop for PreparationReads {
    fn drop(&mut self) {
        READ_SCOPE_DEPTH.with(|depth| depth.set(depth.get() - 1));
        if let Some(start) = self.0 {
            let visits = ACTUAL_VISITS.with(std::cell::Cell::get) - start;
            PREPARATION_VISITS.with(|work| work.set(work.get() + visits));
            crate::agent::execution::attribution::record_native(
                crate::agent::execution::attribution::NativeWork {
                    preparation_items: visits as u64,
                    ..Default::default()
                },
            );
        }
    }
}
#[cfg(test)]
pub(crate) fn take_preparation_visits() -> usize {
    PREPARATION_VISITS.with(|work| work.replace(0))
}
#[cfg(any(test, feature = "issue-387-profile"))]
fn record_actual_visit() {
    #[cfg(test)]
    ACTUAL_VISITS.with(|work| work.set(work.get() + 1));
    #[cfg(feature = "issue-387-profile")]
    crate::model::adapter::profile::borrowed_visit();
}
#[cfg(test)]
pub(crate) fn take_materialized_items() -> usize {
    MATERIALIZED_ITEMS.with(|work| work.replace(0))
}
#[cfg(test)]
fn record_value(_value: &ModelInputMessage, old: bool) {
    if old {
        MATERIALIZED_ITEMS.with(|work| work.set(work.get() + 1));
    }
    crate::agent::execution::attribution::record_native(
        crate::agent::execution::attribution::NativeWork {
            old_items: u64::from(old),
            new_items: u64::from(!old),
            ..Default::default()
        },
    );
}
#[cfg(test)]
pub(crate) fn take_prefix_work() -> (usize, usize) {
    PREFIX_WORK.with(|work| work.replace((0, 0)))
}
#[cfg(test)]
pub(crate) fn take_hash_work() -> (usize, usize) {
    HASH_WORK.with(|n| n.replace((0, 0)))
}
#[cfg(test)]
pub(crate) fn record_hash_work(original: usize, semantic: usize) {
    HASH_WORK.with(|n| {
        let (a, b) = n.get();
        n.set((a + original, b + semantic));
    });
    crate::agent::execution::attribution::record_native(
        crate::agent::execution::attribution::NativeWork {
            hash_bytes: (original + semantic) as u64,
            ..Default::default()
        },
    );
}
#[cfg(test)]
fn record_node_work(kind: usize) {
    NODE_WORK.with(|n| {
        let mut a = n.get();
        match kind {
            0 => a.0 += 1,
            1 => a.1 += 1,
            2 => a.2 += 1,
            _ => a.3 += 1,
        }
        n.set(a);
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StoredInputNode {
    pub owner: u64,
    pub epoch: u64,
    pub id: u64,
    pub digest: [u8; 32],
}

#[derive(Debug)]
pub(crate) struct InputNode {
    pub kind: InputNodeKind,
    pub actual: Option<Arc<ModelInputMessage>>,
    pub count: usize,
    pub height: u8,
    pub frozen_count: usize,
    metrics: OnceLock<InputMetrics>,
    pub digest: OnceLock<[u8; 32]>,
    budget: OnceLock<Result<(usize, usize), ()>>,
    pub stored: Mutex<Option<StoredInputNode>>,
}
#[derive(Debug, Clone)]
pub(crate) enum InputNodeKind {
    Leaf(Option<Box<FrozenModelInput>>),
    Branch {
        left: Arc<InputNode>,
        right: Arc<InputNode>,
    },
}
impl Clone for InputNode {
    fn clone(&self) -> Self {
        #[cfg(test)]
        record_node_work(if matches!(self.kind, InputNodeKind::Leaf(_)) {
            2
        } else {
            3
        });
        Self {
            kind: self.kind.clone(),
            actual: self.actual.clone(),
            count: self.count,
            height: self.height,
            frozen_count: self.frozen_count,
            metrics: OnceLock::new(),
            digest: OnceLock::new(),
            budget: OnceLock::new(),
            stored: Mutex::new(None),
        }
    }
}
#[derive(Debug, Clone, Copy, Default)]
struct InputMetrics {
    visible: Option<u64>,
    request_only: usize,
    media: usize,
    tool_relationships: usize,
}
impl InputMetrics {
    fn empty() -> Self {
        Self {
            visible: Some(0),
            request_only: 0,
            media: 0,
            tool_relationships: 0,
        }
    }
    fn add(self, other: Self) -> Option<Self> {
        Some(Self {
            visible: self
                .visible
                .zip(other.visible)
                .and_then(|(a, b)| a.checked_add(b)),
            request_only: self.request_only.checked_add(other.request_only)?,
            media: self.media.checked_add(other.media)?,
            tool_relationships: self
                .tool_relationships
                .checked_add(other.tool_relationships)?,
        })
    }
}
fn node_metrics(node: &InputNode) -> InputMetrics {
    *node.metrics.get_or_init(|| match &node.kind {
        InputNodeKind::Branch { left, right } => node_metrics(left)
            .add(node_metrics(right))
            .expect("bounded native input metrics"),
        InputNodeKind::Leaf(_) => {
            #[cfg(any(test, feature = "issue-387-profile"))]
            record_actual_visit();
            let message = node.actual.as_deref().expect("materialized model input");
            let visible = crate::context::tokens::model_visible_item_bytes(message);
            let media = match message.view() {
                super::projected::ModelMessageView::User(content) => {
                    usize::from(content.iter().any(|block| {
                        matches!(
                            block,
                            crate::message::types::UserContentBlock::Image(_)
                                | crate::message::types::UserContentBlock::UploadedFile(_)
                        )
                    }))
                }
                super::projected::ModelMessageView::Assistant(content) => {
                    usize::from(content.iter().any(|block| {
                        matches!(
                            block,
                            crate::message::types::AssistantContentBlock::Image(_)
                        )
                    }))
                }
                super::projected::ModelMessageView::Tool(_, result) => {
                    usize::from(result.content.iter().any(|block| {
                        matches!(block, crate::tools::types::ToolResultContent::Image(_))
                    }))
                }
                super::projected::ModelMessageView::RequestOnly(_) => 0,
            };
            let tool_relationships = match message.view() {
                super::projected::ModelMessageView::Assistant(content) => content
                    .iter()
                    .filter(|block| {
                        matches!(
                            block,
                            crate::message::types::AssistantContentBlock::ToolCall(_)
                        )
                    })
                    .count(),
                super::projected::ModelMessageView::Tool(_, _) => 1,
                _ => 0,
            };
            InputMetrics {
                visible,
                request_only: usize::from(matches!(message, ModelInputMessage::RequestOnly(_))),
                media,
                tool_relationships,
            }
        }
    })
}
fn range_metrics(node: Option<&Arc<InputNode>>, start: usize, end: usize) -> Option<InputMetrics> {
    if start == end {
        return Some(InputMetrics::empty());
    }
    let node = node?;
    if start == 0 && end == node.count {
        return Some(node_metrics(node));
    }
    let InputNodeKind::Branch { left, right } = &node.kind else {
        return None;
    };
    let a = range_metrics(Some(left), start.min(left.count), end.min(left.count))?;
    let b = range_metrics(
        Some(right),
        start.saturating_sub(left.count),
        end.saturating_sub(left.count),
    )?;
    a.add(b)
}
fn select_range(node: &Arc<InputNode>, start: usize, end: usize) -> Option<Arc<InputNode>> {
    if start == end {
        return None;
    }
    if start == 0 && end == node.count {
        return Some(node.clone());
    }
    let InputNodeKind::Branch { left, right } = &node.kind else {
        unreachable!()
    };
    match (
        select_range(left, start.min(left.count), end.min(left.count)),
        select_range(
            right,
            start.saturating_sub(left.count),
            end.saturating_sub(left.count),
        ),
    ) {
        (Some(a), Some(b)) => Some(InputNode::join(a, b)),
        (a, None) => a,
        (None, b) => b,
    }
}
fn prefix_equal(
    current: Option<&Arc<InputNode>>,
    measured: Option<&Arc<InputNode>>,
    count: usize,
) -> bool {
    if count == 0 {
        return true;
    }
    let (Some(current), Some(measured)) = (current, measured) else {
        return false;
    };
    let mut a = vec![current.as_ref()];
    let mut b = vec![measured.as_ref()];
    let mut remaining = count;
    while remaining > 0 {
        #[cfg(test)]
        PREFIX_WORK.with(|work| {
            let (nodes, leaves) = work.get();
            work.set((nodes + 1, leaves));
        });
        let (Some(x), Some(y)) = (a.pop(), b.pop()) else {
            return false;
        };
        if std::ptr::eq(x, y) && x.count <= remaining {
            remaining -= x.count;
            continue;
        }
        if x.count == 1 && y.count == 1 {
            #[cfg(any(test, feature = "issue-387-profile"))]
            {
                record_actual_visit();
                record_actual_visit();
            }
            #[cfg(test)]
            PREFIX_WORK.with(|work| {
                let (nodes, leaves) = work.get();
                work.set((nodes, leaves + 1));
            });
            if !matches!((&x.actual,&y.actual),(Some(a),Some(b)) if a==b) {
                return false;
            }
            remaining -= 1;
            continue;
        }
        if x.count >= y.count {
            match &x.kind {
                InputNodeKind::Branch { left, right } => {
                    a.push(right);
                    a.push(left);
                }
                InputNodeKind::Leaf(_) => a.push(x),
            }
        } else {
            a.push(x);
        }
        if y.count >= x.count {
            match &y.kind {
                InputNodeKind::Branch { left, right } => {
                    b.push(right);
                    b.push(left);
                }
                InputNodeKind::Leaf(_) => b.push(y),
            }
        } else {
            b.push(y);
        }
    }
    true
}
impl InputNode {
    fn actual_leaf(value: ModelInputMessage) -> Arc<Self> {
        #[cfg(test)]
        record_value(&value, false);
        #[cfg(test)]
        record_node_work(0);
        Arc::new(Self {
            kind: InputNodeKind::Leaf(None),
            actual: Some(Arc::new(value)),
            count: 1,
            height: 1,
            frozen_count: 0,
            metrics: OnceLock::new(),
            digest: OnceLock::new(),
            budget: OnceLock::new(),
            stored: Mutex::new(None),
        })
    }
    fn leaf(value: FrozenModelInput, actual: Option<Arc<ModelInputMessage>>) -> Arc<Self> {
        #[cfg(test)]
        if let Some(actual) = &actual {
            record_value(actual, false);
        }
        #[cfg(test)]
        record_node_work(0);
        Arc::new(Self {
            kind: InputNodeKind::Leaf(Some(Box::new(value))),
            actual,
            count: 1,
            height: 1,
            frozen_count: 1,
            metrics: OnceLock::new(),
            digest: OnceLock::new(),
            budget: OnceLock::new(),
            stored: Mutex::new(None),
        })
    }
    fn branch(left: Arc<Self>, right: Arc<Self>) -> Arc<Self> {
        #[cfg(test)]
        record_node_work(1);
        Arc::new(Self {
            count: left.count + right.count,
            height: 1 + left.height.max(right.height),
            frozen_count: left.frozen_count + right.frozen_count,
            metrics: OnceLock::new(),
            kind: InputNodeKind::Branch { left, right },
            actual: None,
            digest: OnceLock::new(),
            budget: OnceLock::new(),
            stored: Mutex::new(None),
        })
    }
    fn join(left: Arc<Self>, right: Arc<Self>) -> Arc<Self> {
        if left.height > right.height + 1 {
            let InputNodeKind::Branch { left: a, right: b } = &left.kind else {
                unreachable!()
            };
            let joined = Self::join(b.clone(), right);
            return Self::balance(a.clone(), joined);
        }
        if right.height > left.height + 1 {
            let InputNodeKind::Branch { left: a, right: b } = &right.kind else {
                unreachable!()
            };
            let joined = Self::join(left, a.clone());
            return Self::balance(joined, b.clone());
        }
        Self::branch(left, right)
    }
    fn balance(left: Arc<Self>, right: Arc<Self>) -> Arc<Self> {
        if left.height > right.height + 1 {
            let InputNodeKind::Branch { left: a, right: b } = &left.kind else {
                unreachable!()
            };
            if a.height >= b.height {
                return Self::branch(a.clone(), Self::branch(b.clone(), right));
            }
            let InputNodeKind::Branch { left: c, right: d } = &b.kind else {
                unreachable!()
            };
            return Self::branch(
                Self::branch(a.clone(), c.clone()),
                Self::branch(d.clone(), right),
            );
        }
        if right.height > left.height + 1 {
            let InputNodeKind::Branch { left: a, right: b } = &right.kind else {
                unreachable!()
            };
            if b.height >= a.height {
                return Self::branch(Self::branch(left, a.clone()), b.clone());
            }
            let InputNodeKind::Branch { left: c, right: d } = &a.kind else {
                unreachable!()
            };
            return Self::branch(
                Self::branch(left, c.clone()),
                Self::branch(d.clone(), b.clone()),
            );
        }
        Self::branch(left, right)
    }
    fn insert(node: &Arc<Self>, index: usize, value: FrozenModelInput) -> Arc<Self> {
        match &node.kind {
            InputNodeKind::Leaf(_) => {
                if index == 0 {
                    Self::join(Self::leaf(value, None), node.clone())
                } else {
                    Self::join(node.clone(), Self::leaf(value, None))
                }
            }
            InputNodeKind::Branch { left, right } => {
                if index < left.count {
                    Self::join(Self::insert(left, index, value), right.clone())
                } else {
                    Self::join(left.clone(), Self::insert(right, index - left.count, value))
                }
            }
        }
    }
    fn insert_leaf(node: &Arc<Self>, index: usize, leaf: Arc<Self>) -> Arc<Self> {
        match &node.kind {
            InputNodeKind::Leaf(_) => {
                if index == 0 {
                    Self::join(leaf, node.clone())
                } else {
                    Self::join(node.clone(), leaf)
                }
            }
            InputNodeKind::Branch { left, right } => {
                if index < left.count {
                    Self::join(Self::insert_leaf(left, index, leaf), right.clone())
                } else {
                    Self::join(
                        left.clone(),
                        Self::insert_leaf(right, index - left.count, leaf),
                    )
                }
            }
        }
    }
    fn replace(node: &Arc<Self>, index: usize, leaf: Arc<Self>) -> Arc<Self> {
        match &node.kind {
            InputNodeKind::Leaf(_) => leaf,
            InputNodeKind::Branch { left, right } => {
                if index < left.count {
                    Self::branch(Self::replace(left, index, leaf), right.clone())
                } else {
                    Self::branch(left.clone(), Self::replace(right, index - left.count, leaf))
                }
            }
        }
    }
    fn get(&self, index: usize) -> &FrozenModelInput {
        match &self.kind {
            InputNodeKind::Leaf(value) => value.as_deref().expect("native frozen input evidence"),
            InputNodeKind::Branch { left, right } => {
                if index < left.count {
                    left.get(index)
                } else {
                    right.get(index - left.count)
                }
            }
        }
    }
    fn get_mut(node: &mut Arc<Self>, index: usize) -> &mut FrozenModelInput {
        let node = Arc::make_mut(node);
        node.digest.take();
        node.budget.take();
        node.metrics.take();
        node.actual = None;
        *node.stored.get_mut().expect("native input node") = None;
        match &mut node.kind {
            InputNodeKind::Leaf(value) => {
                value.as_deref_mut().expect("native frozen input evidence")
            }
            InputNodeKind::Branch { left, right } => {
                if index < left.count {
                    Self::get_mut(left, index)
                } else {
                    Self::get_mut(right, index - left.count)
                }
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct SelectionOrigin {
    pub owner: u64,
    pub epoch: u64,
    pub cut: crate::durable::projection::RequestHistoryCut,
    pub baseline: Option<Arc<InputNode>>,
}
#[derive(Debug, Clone)]
pub(crate) enum SelectionEdit {
    Append { index: usize, node: Arc<InputNode> },
}
/// Exact ordered frozen native input with path-copying append/edit operations.
/// Serialization retains the original logical flat-array representation.
#[derive(Debug, Clone, Default)]
pub struct SelectedInputManifest {
    pub(crate) root: Option<Arc<InputNode>>,
    semantic: Option<Sha256>,
    invalid_budget: bool,
    pub(crate) origin: Option<Arc<SelectionOrigin>>,
    pub(crate) edits: imbl::Vector<SelectionEdit>,
}
impl PartialEq for SelectedInputManifest {
    fn eq(&self, other: &Self) -> bool {
        match (&self.root, &other.root) {
            (Some(a), Some(b)) if Arc::ptr_eq(a, b) => true,
            _ => self.len() == other.len() && self.iter().eq(other.iter()),
        }
    }
}
impl SelectedInputManifest {
    /// Constructs a cold exact sequence from actual model input.
    #[must_use]
    pub fn from_messages<'a>(
        scope: &ConversationId,
        messages: impl IntoIterator<Item = &'a ModelInputMessage>,
    ) -> Self {
        let mut selected = Self {
            root: None,
            semantic: Some(Sha256::new()),
            invalid_budget: false,
            origin: None,
            edits: imbl::Vector::new(),
        };
        for message in messages {
            selected = selected.append(scope, message);
        }
        selected
    }
    /// Appends one actual occurrence, retaining the certified unfinished SHA prefix.
    #[must_use]
    pub fn append(&self, scope: &ConversationId, message: &ModelInputMessage) -> Self {
        let mut selected = self.clone();
        if !admit_change(self, message, None) || self.len() >= 8192 {
            selected.invalid_budget = true;
            return selected;
        }
        let leaf = InputNode::leaf(freeze(scope, message), Some(Arc::new(message.clone())));
        selected.root = Some(self.root.as_ref().map_or_else(
            || leaf.clone(),
            |root| InputNode::join(root.clone(), leaf.clone()),
        ));
        if selected.origin.is_some() {
            selected.edits.push_back(SelectionEdit::Append {
                index: self.len(),
                node: leaf,
            });
        }
        selected.semantic.clone_from(&self.semantic);
        if let Some(sha) = &mut selected.semantic {
            super::types::append_semantic_message(sha, message);
        }
        selected
    }
    /// Replaces one actual occurrence; semantic SHA is recomputed at final freeze.
    #[must_use]
    /// Replaces one selected occurrence.
    ///
    /// # Panics
    ///
    /// Panics if `index` is outside the selected sequence.
    pub fn replace(
        &self,
        scope: &ConversationId,
        index: usize,
        message: &ModelInputMessage,
    ) -> Self {
        let mut selected = self.clone();
        if !admit_change(self, message, Some(index)) {
            selected.invalid_budget = true;
            return selected;
        }
        assert!(index < self.len(), "native input replacement bounds");
        let leaf = InputNode::leaf(freeze(scope, message), Some(Arc::new(message.clone())));
        selected.root = Some(InputNode::replace(self.root.as_ref().unwrap(), index, leaf));
        selected.semantic = None;
        selected.origin = None;
        selected.edits.clear();
        selected
    }
    /// Inserts one actual occurrence at the producer's explicit insertion point.
    #[must_use]
    /// Inserts one selected occurrence.
    ///
    /// # Panics
    ///
    /// Panics if `index` is greater than the selected sequence length.
    pub fn insert(
        &self,
        scope: &ConversationId,
        index: usize,
        message: &ModelInputMessage,
    ) -> Self {
        assert!(index <= self.len(), "native input insertion bounds");
        if index == self.len() {
            return self.append(scope, message);
        }
        let mut selected = self.clone();
        if !admit_change(self, message, None) || self.len() >= 8192 {
            selected.invalid_budget = true;
            return selected;
        }
        selected.root = Some(InputNode::insert(
            self.root.as_ref().unwrap(),
            index,
            freeze(scope, message),
        ));
        set_actual(
            selected.root.as_mut().unwrap(),
            index,
            Arc::new(message.clone()),
        );
        selected.origin = None;
        selected.edits.clear();
        selected.semantic = None;
        selected
    }
    /// Number of ordered occurrences.
    #[must_use]
    pub fn len(&self) -> usize {
        self.root.as_ref().map_or(0, |node| node.count)
    }
    /// Whether the selection is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }
    /// Borrows the exact frozen occurrence at its original rank.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&FrozenModelInput> {
        self.root
            .as_ref()
            .filter(|root| index < root.count)
            .map(|root| root.get(index))
    }
    /// Iterates the original occurrence order with depth-bounded traversal state.
    pub fn iter(&self) -> ManifestIter<'_> {
        ManifestIter {
            pending: self.root.iter().map(AsRef::as_ref).collect(),
            remaining: self.len(),
        }
    }
    /// Appends a frozen occurrence without a producer semantic certificate.
    pub fn push(&mut self, item: FrozenModelInput) {
        let leaf = InputNode::leaf(item, None);
        self.root = Some(
            self.root
                .take()
                .map_or_else(|| leaf.clone(), |root| InputNode::join(root, leaf.clone())),
        );
        self.semantic = None;
        self.origin = None;
        self.edits.clear();
    }
    /// Clears all occurrences and certificates.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    pub(crate) fn semantic_digest(&self) -> Option<String> {
        self.semantic
            .as_ref()
            .map(|sha| format!("{:x}", sha.clone().finalize()))
    }
}
fn freeze(scope: &ConversationId, message: &ModelInputMessage) -> FrozenModelInput {
    match message {
        ModelInputMessage::Canonical(message) => {
            FrozenModelInput::Original(CanonicalMessageRef::of(scope, message))
        }
        ModelInputMessage::Derived(message) => FrozenModelInput::Derived(message.clone()),
        ModelInputMessage::RequestOnly(message) => FrozenModelInput::RequestOnly(message.clone()),
    }
}
impl Index<usize> for SelectedInputManifest {
    type Output = FrozenModelInput;
    fn index(&self, index: usize) -> &Self::Output {
        self.get(index).expect("native input index")
    }
}
impl IndexMut<usize> for SelectedInputManifest {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        assert!(index < self.len(), "native input index");
        self.semantic = None;
        self.origin = None;
        self.edits.clear();
        InputNode::get_mut(self.root.as_mut().unwrap(), index)
    }
}
/// Ordered iterator over an immutable native selection.
pub struct ManifestIter<'a> {
    pending: Vec<&'a InputNode>,
    remaining: usize,
}
impl<'a> Iterator for ManifestIter<'a> {
    type Item = &'a FrozenModelInput;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match &self.pending.pop()?.kind {
                InputNodeKind::Leaf(value) => {
                    self.remaining -= 1;
                    return Some(value.as_deref().expect("native frozen input evidence"));
                }
                InputNodeKind::Branch { left, right } => {
                    self.pending.push(right);
                    self.pending.push(left);
                }
            }
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}
impl ExactSizeIterator for ManifestIter<'_> {}
impl<'a> IntoIterator for &'a SelectedInputManifest {
    type Item = &'a FrozenModelInput;
    type IntoIter = ManifestIter<'a>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
impl FromIterator<FrozenModelInput> for SelectedInputManifest {
    fn from_iter<T: IntoIterator<Item = FrozenModelInput>>(iter: T) -> Self {
        let mut selected = Self::default();
        for item in iter {
            selected.push(item);
        }
        selected
    }
}
impl Serialize for SelectedInputManifest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter())
    }
}
impl<'de> Deserialize<'de> for SelectedInputManifest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Vec::<FrozenModelInput>::deserialize(deserializer)?
            .into_iter()
            .collect())
    }
}

fn set_actual(node: &mut Arc<InputNode>, index: usize, value: Arc<ModelInputMessage>) {
    let node = Arc::make_mut(node);
    match &mut node.kind {
        InputNodeKind::Leaf(_) => node.actual = Some(value),
        InputNodeKind::Branch { left, right } => {
            if index < left.count {
                set_actual(left, index, value);
            } else {
                set_actual(right, index - left.count, value);
            }
        }
    }
}
fn actual(node: &InputNode, index: usize) -> &ModelInputMessage {
    match &node.kind {
        InputNodeKind::Leaf(_) => {
            #[cfg(any(test, feature = "issue-387-profile"))]
            record_actual_visit();
            node.actual
                .as_deref()
                .expect("materialized native selected input")
        }
        InputNodeKind::Branch { left, right } => {
            if index < left.count {
                actual(left, index)
            } else {
                actual(right, index - left.count)
            }
        }
    }
}
/// Shared materialized view of the same ordered occurrence nodes.
/// Cold provider requests may own actual-only leaves; native persistence requires
/// their explicit scoped freeze before accepting them as immutable evidence.
#[derive(Debug, Clone, Default)]
pub struct SelectedModelInput(pub(crate) SelectedInputManifest);
impl PartialEq for SelectedModelInput {
    fn eq(&self, other: &Self) -> bool {
        self.same_input(other)
    }
}
impl SelectedModelInput {
    /// Original ranks that contain media, skipping immutable media-free subtrees.
    #[must_use]
    pub(crate) fn media_indices(&self) -> Vec<usize> {
        fn visit(node: &InputNode, base: usize, output: &mut Vec<usize>) {
            if node_metrics(node).media == 0 {
                return;
            }
            match &node.kind {
                InputNodeKind::Leaf(_) => output.push(base),
                InputNodeKind::Branch { left, right } => {
                    visit(left, base, output);
                    visit(right, base + left.count, output);
                }
            }
        }
        let mut output = Vec::new();
        if let Some(root) = &self.0.root {
            visit(root, 0, &mut output);
        }
        output
    }
    #[must_use]
    pub(crate) fn has_tool_relationships(&self) -> bool {
        self.0
            .root
            .as_ref()
            .is_some_and(|node| node_metrics(node).tool_relationships > 0)
    }
    pub(crate) fn tool_indices(&self) -> Vec<usize> {
        fn visit(node: &InputNode, base: usize, output: &mut Vec<usize>) {
            if node_metrics(node).tool_relationships == 0 {
                return;
            }
            match &node.kind {
                InputNodeKind::Leaf(_) => output.push(base),
                InputNodeKind::Branch { left, right } => {
                    visit(left, base, output);
                    visit(right, base + left.count, output);
                }
            }
        }
        let mut output = Vec::new();
        if let Some(root) = &self.0.root {
            visit(root, 0, &mut output);
        }
        output
    }
    /// Native relationship validation survives only a certified unchanged prefix
    /// followed by the actual producer's new canonical Context messages.
    pub(crate) fn follows_native_history(&self, history: &SelectedInputManifest) -> bool {
        let (Some(selected), Some(baseline)) = (&self.0.origin, &history.origin) else {
            return false;
        };
        if !Arc::ptr_eq(selected, baseline) || !history.edits.is_empty() {
            return false;
        }
        let mut expected = history.len();
        for edit in &self.0.edits {
            let SelectionEdit::Append { index, node } = edit;
            if *index != expected
                || !matches!(node.actual.as_deref(),Some(ModelInputMessage::Canonical(crate::message::types::MessageBlock::User(user))) if matches!(user.kind,crate::message::types::InboundKind::Context(_)))
            {
                return false;
            }
            expected += 1;
        }
        expected == self.len()
    }
    /// Adds genuine scoped immutable evidence to a cold actual-only input.
    #[must_use]
    pub(crate) fn freeze(&self, scope: &ConversationId) -> SelectedInputManifest {
        fn bind(node: &Arc<InputNode>, scope: &ConversationId) -> Arc<InputNode> {
            if node.frozen_count == node.count {
                return node.clone();
            }
            match &node.kind {
                InputNodeKind::Leaf(_) => InputNode::leaf(
                    freeze(scope, node.actual.as_deref().expect("actual input")),
                    node.actual.clone(),
                ),
                InputNodeKind::Branch { left, right } => {
                    InputNode::branch(bind(left, scope), bind(right, scope))
                }
            }
        }
        SelectedInputManifest {
            root: self.0.root.as_ref().map(|node| bind(node, scope)),
            ..SelectedInputManifest::default()
        }
    }
    /// Borrows ordered model-visible values without copying bodies.
    pub fn iter(&self) -> ModelManifestIter<'_> {
        ModelManifestIter {
            pending: self.0.root.iter().map(AsRef::as_ref).collect(),
            back_pending: self.0.root.iter().map(AsRef::as_ref).collect(),
            remaining: self.len(),
            reverse: false,
        }
    }
    /// Borrows the newest occurrences first with depth-bounded traversal state.
    pub(crate) fn iter_rev(&self) -> ModelManifestIter<'_> {
        ModelManifestIter {
            pending: self.0.root.iter().map(AsRef::as_ref).collect(),
            back_pending: self.0.root.iter().map(AsRef::as_ref).collect(),
            remaining: self.len(),
            reverse: true,
        }
    }
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&ModelInputMessage> {
        self.0
            .root
            .as_ref()
            .filter(|root| index < root.count)
            .map(|root| actual(root, index))
    }
    /// Borrows mutable actual values, revoking affected frozen evidence.
    pub fn iter_mut(&mut self) -> ModelManifestIterMut<'_> {
        self.0.semantic = None;
        self.0.origin = None;
        self.0.edits.clear();
        let remaining = self.len();
        ModelManifestIterMut {
            remaining,
            pending: self
                .0
                .root
                .as_mut()
                .map(Arc::make_mut)
                .into_iter()
                .collect(),
        }
    }
    /// Explicit complete disclosure for slice-based external consumers.
    #[must_use]
    pub fn to_vec(&self) -> Vec<ModelInputMessage> {
        let values = self.iter();
        #[cfg(test)]
        let values = values.inspect(|value| record_value(value, true));
        values.cloned().collect()
    }
    /// Appends an actual value, without manufacturing frozen provenance.
    pub fn push(&mut self, value: ModelInputMessage) {
        let leaf = InputNode::actual_leaf(value);
        self.0.root = Some(
            self.0
                .root
                .take()
                .map_or_else(|| leaf.clone(), |root| InputNode::join(root, leaf.clone())),
        );
        self.0.semantic = None;
        self.0.origin = None;
        self.0.edits.clear();
    }
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    /// Replaces one actual item; native evidence is reacquired only by scoped freeze.
    /// # Panics
    /// Panics when `index` is outside this input.
    pub fn replace(&mut self, index: usize, value: ModelInputMessage) {
        assert!(index < self.len(), "model input replacement bounds");
        self.0.root = Some(InputNode::replace(
            self.0.root.as_ref().unwrap(),
            index,
            InputNode::actual_leaf(value),
        ));
        self.0.semantic = None;
        self.0.origin = None;
        self.0.edits.clear();
    }
    #[must_use]
    pub fn first(&self) -> Option<&ModelInputMessage> {
        self.get(0)
    }
    #[must_use]
    pub fn last(&self) -> Option<&ModelInputMessage> {
        self.len().checked_sub(1).and_then(|index| self.get(index))
    }
    /// Inserts a cold actual occurrence without inventing immutable evidence.
    /// # Panics
    /// Panics when `index` exceeds this input's length.
    pub fn insert(&mut self, index: usize, value: ModelInputMessage) {
        assert!(index <= self.len(), "model input insertion bounds");
        if index == self.len() {
            self.push(value);
            return;
        }
        self.0.root = Some(InputNode::insert_leaf(
            self.0.root.as_ref().unwrap(),
            index,
            InputNode::actual_leaf(value),
        ));
        self.0.semantic = None;
        self.0.origin = None;
        self.0.edits.clear();
    }
    /// Exact established flat semantic SHA; immutable native prefixes retain its state.
    #[must_use]
    pub fn semantic_fingerprint(&self) -> String {
        self.0
            .semantic_digest()
            .unwrap_or_else(|| super::types::semantic_messages_fingerprint(self))
    }
    /// Exact same ordered actual input, skipping shared immutable subtrees.
    #[must_use]
    pub fn same_input(&self, other: &Self) -> bool {
        self.len() == other.len() && self.covers_prefix(other)
    }
    /// True when `other` is an exact ordered prefix of this input.
    #[must_use]
    pub fn covers_prefix(&self, other: &Self) -> bool {
        other.len() <= self.len()
            && prefix_equal(self.0.root.as_ref(), other.0.root.as_ref(), other.len())
    }
    /// Borrows a persistent selected interval.
    /// # Panics
    /// Panics for invalid interval bounds.
    #[must_use]
    pub fn range(&self, range: std::ops::Range<usize>) -> Self {
        assert!(
            range.start <= range.end && range.end <= self.len(),
            "model input range bounds"
        );
        Self(SelectedInputManifest {
            root: self
                .0
                .root
                .as_ref()
                .and_then(|node| select_range(node, range.start, range.end)),
            ..SelectedInputManifest::default()
        })
    }
    /// Exact model-visible JSON array bytes, without constructing the array.
    #[must_use]
    pub fn visible_bytes(&self, range: std::ops::Range<usize>) -> Option<u64> {
        let count = range.end.checked_sub(range.start)?;
        if range.end > self.len() {
            return None;
        }
        let total = range_metrics(self.0.root.as_ref(), range.start, range.end)?.visible?;
        total
            .checked_add(2)?
            .checked_add(u64::try_from(count.saturating_sub(1)).ok()?)
    }
    #[must_use]
    pub fn has_request_only(&self) -> bool {
        self.0
            .root
            .as_ref()
            .is_some_and(|node| node_metrics(node).request_only > 0)
    }
    #[must_use]
    pub fn has_media(&self) -> bool {
        self.0
            .root
            .as_ref()
            .is_some_and(|node| node_metrics(node).media > 0)
    }
}
impl From<Vec<ModelInputMessage>> for SelectedModelInput {
    fn from(values: Vec<ModelInputMessage>) -> Self {
        values.into_iter().collect()
    }
}
impl FromIterator<ModelInputMessage> for SelectedModelInput {
    fn from_iter<T: IntoIterator<Item = ModelInputMessage>>(values: T) -> Self {
        let mut selected = Self::default();
        for value in values {
            selected.push(value);
        }
        selected
    }
}
impl Extend<ModelInputMessage> for SelectedModelInput {
    fn extend<T: IntoIterator<Item = ModelInputMessage>>(&mut self, values: T) {
        for value in values {
            self.push(value);
        }
    }
}
impl Serialize for SelectedModelInput {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter())
    }
}
impl<'de> Deserialize<'de> for SelectedModelInput {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Vec::<ModelInputMessage>::deserialize(deserializer)?.into())
    }
}
impl SelectedInputManifest {
    /// Shares the actual selected values with their exact frozen provenance.
    #[must_use]
    pub(crate) fn messages(&self) -> SelectedModelInput {
        SelectedModelInput(self.clone())
    }
}
impl IndexMut<usize> for SelectedModelInput {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        assert!(index < self.len(), "model input index");
        self.0.semantic = None;
        self.0.origin = None;
        self.0.edits.clear();
        actual_mut(self.0.root.as_mut().unwrap(), index)
    }
}
fn actual_mut(node: &mut Arc<InputNode>, index: usize) -> &mut ModelInputMessage {
    let node = Arc::make_mut(node);
    node.metrics.take();
    node.digest.take();
    node.budget.take();
    *node.stored.get_mut().expect("native input node") = None;
    match &mut node.kind {
        InputNodeKind::Leaf(evidence) => {
            *evidence = None;
            node.frozen_count = 0;
            Arc::make_mut(node.actual.as_mut().expect("actual input"))
        }
        InputNodeKind::Branch { left, right } => {
            node.frozen_count = 0;
            if index < left.count {
                actual_mut(left, index)
            } else {
                actual_mut(right, index - left.count)
            }
        }
    }
}
impl Index<usize> for SelectedModelInput {
    type Output = ModelInputMessage;
    fn index(&self, index: usize) -> &Self::Output {
        assert!(index < self.len(), "native selected model index");
        actual(self.0.root.as_ref().unwrap(), index)
    }
}

/// Borrowed ordered model values from the same native selected root.
#[derive(Clone)]
pub struct ModelManifestIter<'a> {
    pending: Vec<&'a InputNode>,
    back_pending: Vec<&'a InputNode>,
    remaining: usize,
    reverse: bool,
}
impl<'a> Iterator for ModelManifestIter<'a> {
    type Item = &'a ModelInputMessage;
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let value = next_model_leaf(&mut self.pending, self.reverse)?;
        self.remaining -= 1;
        Some(value)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}
fn next_model_leaf<'a>(
    pending: &mut Vec<&'a InputNode>,
    reverse: bool,
) -> Option<&'a ModelInputMessage> {
    loop {
        let node = pending.pop()?;
        match &node.kind {
            InputNodeKind::Leaf(_) => {
                #[cfg(any(test, feature = "issue-387-profile"))]
                record_actual_visit();
                return Some(
                    node.actual
                        .as_deref()
                        .expect("materialized native selected input"),
                );
            }
            InputNodeKind::Branch { left, right } => {
                if reverse {
                    pending.push(left);
                    pending.push(right);
                } else {
                    pending.push(right);
                    pending.push(left);
                }
            }
        }
    }
}
impl DoubleEndedIterator for ModelManifestIter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let value = next_model_leaf(&mut self.back_pending, !self.reverse)?;
        self.remaining -= 1;
        Some(value)
    }
}
/// Explicit mutable traversal revokes all affected native evidence before exposing values.
pub struct ModelManifestIterMut<'a> {
    pending: Vec<&'a mut InputNode>,
    remaining: usize,
}
impl<'a> Iterator for ModelManifestIterMut<'a> {
    type Item = &'a mut ModelInputMessage;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let node = self.pending.pop()?;
            node.metrics.take();
            node.digest.take();
            node.budget.take();
            *node.stored.get_mut().expect("native input node") = None;
            node.frozen_count = 0;
            match &mut node.kind {
                InputNodeKind::Leaf(evidence) => {
                    *evidence = None;
                    self.remaining -= 1;
                    return Some(Arc::make_mut(
                        node.actual.as_mut().expect("actual model input"),
                    ));
                }
                InputNodeKind::Branch { left, right } => {
                    self.pending.push(Arc::make_mut(right));
                    self.pending.push(Arc::make_mut(left));
                }
            }
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}
impl ExactSizeIterator for ModelManifestIterMut<'_> {}
impl<'a> IntoIterator for &'a mut SelectedModelInput {
    type Item = &'a mut ModelInputMessage;
    type IntoIter = ModelManifestIterMut<'a>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}
pub struct OwnedModelManifestIter {
    pending: Vec<Arc<InputNode>>,
    remaining: usize,
}
impl Iterator for OwnedModelManifestIter {
    type Item = ModelInputMessage;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let node = self.pending.pop()?;
            match &node.kind {
                InputNodeKind::Leaf(_) => {
                    self.remaining -= 1;
                    let value = node.actual.as_deref().expect("actual input");
                    #[cfg(test)]
                    record_value(value, true);
                    return Some(value.clone());
                }
                InputNodeKind::Branch { left, right } => {
                    self.pending.push(right.clone());
                    self.pending.push(left.clone());
                }
            }
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}
impl ExactSizeIterator for OwnedModelManifestIter {}
impl IntoIterator for SelectedModelInput {
    type Item = ModelInputMessage;
    type IntoIter = OwnedModelManifestIter;
    fn into_iter(self) -> Self::IntoIter {
        OwnedModelManifestIter {
            remaining: self.len(),
            pending: self.0.root.into_iter().collect(),
        }
    }
}
impl ExactSizeIterator for ModelManifestIter<'_> {}
impl<'a> IntoIterator for &'a SelectedModelInput {
    type Item = &'a ModelInputMessage;
    type IntoIter = ModelManifestIter<'a>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

struct BudgetCounter(usize);
impl std::io::Write for BudgetCounter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .filter(|n| *n <= 8 * 1024 * 1024)
            .ok_or_else(|| std::io::Error::other("frozen Derived input byte bound"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn derived_budget(value: &super::projected::DerivedModelMessage) -> Result<(usize, usize), ()> {
    let refs = value
        .sources
        .len()
        .checked_add(value.checkpoint_sources.len())
        .ok_or(())?;
    if refs > 1024 {
        return Err(());
    }
    let mut bytes = BudgetCounter(0);
    serde_json::to_writer(&mut bytes, value).map_err(|_| ())?;
    Ok((bytes.0, refs))
}
fn admit_change(
    selected: &SelectedInputManifest,
    message: &ModelInputMessage,
    replaced: Option<usize>,
) -> bool {
    let added = match message {
        ModelInputMessage::Derived(value) => match derived_budget(value) {
            Ok(n) => n,
            Err(()) => return false,
        },
        _ => (0, 0),
    };
    let prior = match selected.root.as_ref().map(|n| node_budget(n)).transpose() {
        Ok(n) => n.unwrap_or((0, 0)),
        Err(()) => return false,
    };
    let removed = match replaced.and_then(|i| selected.get(i)) {
        Some(FrozenModelInput::Derived(value)) => match derived_budget(value) {
            Ok(n) => n,
            Err(()) => return false,
        },
        _ => (0, 0),
    };
    prior
        .0
        .checked_sub(removed.0)
        .and_then(|n| n.checked_add(added.0))
        .is_some_and(|n| n <= 8 * 1024 * 1024)
        && prior
            .1
            .checked_sub(removed.1)
            .and_then(|n| n.checked_add(added.1))
            .is_some_and(|n| n <= 1024)
}
fn node_budget(node: &InputNode) -> Result<(usize, usize), ()> {
    *node.budget.get_or_init(|| match &node.kind {
        InputNodeKind::Leaf(value) => match value.as_deref().ok_or(())? {
            FrozenModelInput::Derived(value) => derived_budget(value),
            FrozenModelInput::Original(_) | FrozenModelInput::RequestOnly(_) => Ok((0, 0)),
        },
        InputNodeKind::Branch { left, right } => {
            let a = node_budget(left)?;
            let b = node_budget(right)?;
            let bytes = a.0.checked_add(b.0).ok_or(())?;
            let refs = a.1.checked_add(b.1).ok_or(())?;
            if bytes > 8 * 1024 * 1024 || refs > 1024 {
                Err(())
            } else {
                Ok((bytes, refs))
            }
        }
    })
}
impl SelectedInputManifest {
    pub(crate) fn validate_budget(&self) -> Result<(), String> {
        if self.invalid_budget || self.len() > 8192 {
            return Err("frozen input exceeds item or Derived budget".into());
        }
        if let Some(root) = &self.root {
            node_budget(root).map_err(|()| {
                "frozen input exceeds Derived byte or provenance budget".to_owned()
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
fn node_at(node: &Arc<InputNode>, index: usize) -> &Arc<InputNode> {
    match &node.kind {
        InputNodeKind::Leaf(_) => node,
        InputNodeKind::Branch { left, right } => {
            if index < left.count {
                node_at(left, index)
            } else {
                node_at(right, index - left.count)
            }
        }
    }
}
impl SelectedInputManifest {
    pub(crate) fn certify_selection(
        &mut self,
        owner: u64,
        epoch: u64,
        cut: crate::durable::projection::RequestHistoryCut,
    ) {
        self.origin = Some(Arc::new(SelectionOrigin {
            owner,
            epoch,
            cut,
            baseline: self.root.clone(),
        }));
        self.edits.clear();
    }
}
