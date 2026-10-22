//! Bounded line comparisons; raw file captures never travel through the RPC.
use serde::{Deserialize, Serialize};
use std::time::Duration;

const MAX_DIFF_BYTES: usize = 128 * 1024;
const MAX_DIFF_LINES: usize = 2000;

/// Three lines of shared context around one group of edits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ComparisonHunk {
    pub old_start: usize,
    pub new_start: usize,
    pub old_text: String,
    pub new_text: String,
}

/// An immutable, bounded comparison of a recorded file's two endpoints.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileComparison {
    pub hunks: Vec<ComparisonHunk>,
    pub binary: bool,
    pub oversized: bool,
    pub truncated: bool,
}

pub(super) struct FileContents {
    old_text: Option<String>,
    new_text: Option<String>,
    pub binary: bool,
    pub oversized: bool,
}
impl FileContents {
    pub fn from_bytes(before: Option<Vec<u8>>, after: Option<Vec<u8>>) -> Self {
        let oversized = before.is_none() || after.is_none();
        let binary = [&before, &after]
            .iter()
            .filter_map(|bytes| bytes.as_ref())
            .any(|bytes| bytes.contains(&0) || std::str::from_utf8(bytes).is_err());
        let text = |bytes: Option<Vec<u8>>| {
            if binary || oversized {
                None
            } else {
                bytes.and_then(|bytes| String::from_utf8(bytes).ok())
            }
        };
        Self {
            old_text: text(before),
            new_text: text(after),
            binary,
            oversized,
        }
    }
    pub fn counts(&self) -> (usize, usize) {
        let (Some(before), Some(after)) = (&self.old_text, &self.new_text) else {
            return (0, 0);
        };
        let before = normalize(before);
        let after = normalize(after);
        let diff = similar::TextDiff::configure()
            .timeout(Duration::from_millis(100))
            .diff_lines(before, after);
        let mut added = 0;
        let mut deleted = 0;
        for change in diff.iter_all_changes() {
            match change.tag() {
                similar::ChangeTag::Insert => added += 1,
                similar::ChangeTag::Delete => deleted += 1,
                similar::ChangeTag::Equal => {}
            }
        }
        (added, deleted)
    }
    pub fn comparison(self) -> FileComparison {
        let mut result = FileComparison {
            hunks: Vec::new(),
            binary: self.binary,
            oversized: self.oversized,
            truncated: false,
        };
        let (Some(before), Some(after)) = (&self.old_text, &self.new_text) else {
            return result;
        };
        let diff = similar::TextDiff::configure()
            .timeout(Duration::from_millis(100))
            .diff_lines(before.as_str(), after.as_str());
        let mut bytes = 0;
        let mut lines = 0;
        for group in diff.grouped_ops(3) {
            let mut hunk = ComparisonHunk {
                old_start: group[0].old_range().start + 1,
                new_start: group[0].new_range().start + 1,
                old_text: String::new(),
                new_text: String::new(),
            };
            'group: for op in &group {
                for change in diff.iter_changes(op) {
                    let text = change.value();
                    let sides = if change.tag() == similar::ChangeTag::Equal {
                        2
                    } else {
                        1
                    };
                    if bytes + text.len() * sides > MAX_DIFF_BYTES || lines >= MAX_DIFF_LINES {
                        result.truncated = true;
                        break 'group;
                    }
                    bytes += text.len() * sides;
                    lines += 1;
                    if change.tag() != similar::ChangeTag::Insert {
                        hunk.old_text.push_str(text);
                    }
                    if change.tag() != similar::ChangeTag::Delete {
                        hunk.new_text.push_str(text);
                    }
                }
            }
            if !hunk.old_text.is_empty() || !hunk.new_text.is_empty() {
                result.hunks.push(hunk);
            }
            if result.truncated {
                break;
            }
        }
        result
    }
}

fn normalize(text: &str) -> &str {
    text.strip_suffix('\n').unwrap_or(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn large_files_send_only_changed_regions_and_replacements_remain_wire_bounded() {
        use std::fmt::Write;
        let mut before = String::new();
        for i in 0..50_000 {
            writeln!(&mut before, "line-{i}").unwrap();
        }
        let after = before.replace("line-25000\n", "changed\n");
        let diff = FileContents::from_bytes(Some(before.into_bytes()), Some(after.into_bytes()))
            .comparison();
        assert_eq!(diff.hunks.len(), 1);
        assert_eq!(diff.hunks[0].old_start, 24998);
        assert!(!diff.truncated);
        assert!(serde_json::to_vec(&diff).unwrap().len() < 1024);
        let diff = FileContents::from_bytes(Some(vec![b'\n'; 500_000]), Some(vec![b'\t'; 500_000]))
            .comparison();
        assert!(diff.truncated);
        assert!(
            serde_json::to_vec(&diff).unwrap().len()
                < crate::app_server::transport::MAX_MESSAGE_BYTES
        );
    }
}
