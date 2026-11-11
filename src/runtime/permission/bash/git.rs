//! Git-specific read-only argv policy (ZCode bash-readonly-policy-argv-git).

use super::callbacks::run_danger_callback;
use super::flags_walk::is_argv_allowed_by_policy;
use super::types::BashReadonlyCommandPolicy;
use std::collections::HashSet;
use std::sync::LazyLock;

static GIT_GLOBAL_NO_VALUE_FLAGS: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| ["--no-pager", "--paginate"].into_iter().collect());

static GIT_GLOBAL_DANGEROUS_FLAGS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "-c", "-C", "--attr-source", "--bare", "--config-env", "--exec-path", "--git-dir",
        "--namespace", "--shallow-file", "--super-prefix", "--work-tree",
    ]
    .into_iter()
    .collect()
});

#[must_use]
pub fn is_git_read_only_command(
    argv: &[String],
    policies: &[(String, BashReadonlyCommandPolicy)],
) -> bool {
    let Some(normalized) = normalize_git_argv(argv) else {
        return false;
    };
    for (prefix, policy) in policies {
        let prefix_words: Vec<&str> = prefix.split_whitespace().collect();
        if prefix_words
            .iter()
            .enumerate()
            .all(|(index, word)| normalized.get(index).map(String::as_str) == Some(*word))
        {
            if let Some(callback) = policy.danger_callback {
                if run_danger_callback(callback, prefix, &normalized[prefix_words.len()..]) {
                    return false;
                }
            }
            return is_argv_allowed_by_policy(&normalized, policy, "git", prefix_words.len());
        }
    }
    false
}

#[must_use]
pub fn has_dangerous_git_global_option(argv: &[String]) -> bool {
    argv.iter().any(|word| has_dangerous_git_global_option_word(word))
}

fn normalize_git_argv(argv: &[String]) -> Option<Vec<String>> {
    let mut normalized = vec!["git".to_owned()];
    let mut index = 1;
    while index < argv.len() {
        let word = &argv[index];
        if word.is_empty() {
            index += 1;
            continue;
        }
        if GIT_GLOBAL_NO_VALUE_FLAGS.contains(word.as_str()) {
            index += 1;
            continue;
        }
        if has_dangerous_git_global_option_word(word) {
            return None;
        }
        if word.starts_with('-') {
            return None;
        }
        normalized.extend(argv[index..].iter().cloned());
        return Some(normalized);
    }
    None
}

fn has_dangerous_git_global_option_word(word: &str) -> bool {
    for flag in ["-c", "-C"] {
        if word.len() > flag.len()
            && word.starts_with(flag)
            && (*flag == "-C" || word.as_bytes().get(flag.len()) != Some(&b'-'))
        {
            return true;
        }
    }
    if GIT_GLOBAL_DANGEROUS_FLAGS.contains(word) {
        return true;
    }
    GIT_GLOBAL_DANGEROUS_FLAGS
        .iter()
        .any(|flag| word.starts_with(&format!("{flag}=")))
}
