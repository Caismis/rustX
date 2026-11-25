//! Instance-owned logical environment authorization. The Client owns descriptors.
pub mod files;
use crate::runtime::CancellationSignal;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

#[derive(Default)]
pub(crate) struct Environment {
    roots: RwLock<BTreeMap<PathBuf, CancellationSignal>>,
}

impl Environment {
    /// Removed grants are cancelled before replacements are published.
    pub(crate) fn configure(&self, paths: &[PathBuf]) {
        let mut grants = self.roots.write().expect("environment roots");
        grants.retain(|path, signal| {
            let keep = paths.contains(path);
            if !keep {
                signal.cancel();
            }
            keep
        });
        for path in paths {
            grants.entry(path.clone()).or_default();
        }
    }

    /// The retirement signal for the longest configured logical grant.
    pub(crate) fn retirement(&self, path: &Path) -> Option<CancellationSignal> {
        self.roots
            .read()
            .expect("environment roots")
            .iter()
            .filter(|(root, _)| path.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
            .map(|(_, signal)| signal.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nonexistent_directory_still_has_a_logical_grant() {
        let parent = tempfile::tempdir().unwrap();
        let missing = parent.path().join("missing");
        let environment = Environment::default();
        environment.configure(std::slice::from_ref(&missing));

        let signal = environment
            .retirement(&missing.join("file"))
            .expect("the logical grant exists without a directory");
        assert!(!signal.is_cancelled());
        assert!(!missing.exists());
    }

    #[test]
    fn nested_grants_resolve_longest_even_if_child_is_missing() {
        let parent = tempfile::tempdir().unwrap();
        let child = parent.path().join("child");
        let environment = Environment::default();
        environment.configure(&[parent.path().to_path_buf(), child.clone()]);

        let inner = environment.retirement(&child.join("file")).unwrap();
        let outer = environment.retirement(&parent.path().join("file")).unwrap();
        assert!(!inner.is_cancelled() && !outer.is_cancelled());
        assert!(!child.exists());
        // Unchanged grants keep their family across publications.
        inner.cancel();
        assert!(
            environment
                .retirement(&child.join("file"))
                .unwrap()
                .is_cancelled()
        );
        environment.configure(&[parent.path().to_path_buf(), child.clone()]);
        assert!(
            environment
                .retirement(&child.join("file"))
                .unwrap()
                .is_cancelled()
        );
    }

    #[test]
    fn removing_a_nested_grant_cancels_only_it_and_falls_back_to_the_parent_grant() {
        let parent = tempfile::tempdir().unwrap();
        let child = parent.path().join("child");
        std::fs::create_dir(&child).unwrap();
        let environment = Environment::default();
        environment.configure(&[parent.path().to_path_buf(), child.clone()]);
        let inner = environment.retirement(&child.join("file")).unwrap();
        let outer = environment.retirement(&parent.path().join("file")).unwrap();

        environment.configure(std::slice::from_ref(&parent.path().to_path_buf()));
        assert!(inner.is_cancelled());
        assert!(!outer.is_cancelled());
        let fallback = environment.retirement(&child.join("file")).unwrap();
        assert!(!fallback.is_cancelled());
    }

    #[test]
    fn revocation_cancels_the_grant_and_regrant_issues_a_fresh_signal() {
        let root = tempfile::tempdir().unwrap();
        let environment = Environment::default();
        environment.configure(&[root.path().to_path_buf()]);
        let first = environment.retirement(&root.path().join("file")).unwrap();

        environment.configure(&[root.path().to_path_buf()]);
        let unchanged = environment.retirement(&root.path().join("file")).unwrap();
        first.cancel();
        assert!(
            unchanged.is_cancelled(),
            "an unchanged grant keeps its family"
        );

        environment.configure(&[]);
        assert!(environment.retirement(&root.path().join("file")).is_none());

        environment.configure(&[root.path().to_path_buf()]);
        let regranted = environment.retirement(&root.path().join("file")).unwrap();
        assert!(!regranted.is_cancelled(), "a regrant is a fresh grant");
    }
}
