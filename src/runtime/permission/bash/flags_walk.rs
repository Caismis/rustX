//! Argv flag walking against a command policy (ZCode argv-flags).

use super::callbacks::XARGS_TARGET_COMMANDS;
use super::flags::is_option_like;
use super::types::{BashReadonlyCommandPolicy, SafeFlagValue};
use std::collections::HashMap;

#[must_use]
pub fn is_argv_allowed_by_policy(
    argv: &[String],
    policy: &BashReadonlyCommandPolicy,
    command_name: &str,
    start_index: usize,
) -> bool {
    if argv.is_empty() {
        return false;
    }
    if policy.allow_any_args {
        return true;
    }
    if policy.command_only {
        return argv.len() == start_index;
    }
    let Some(safe_flags) = policy.safe_flags.as_ref() else {
        return false;
    };
    let mut index = start_index;
    while index < argv.len() {
        let word = &argv[index];
        if word.is_empty() {
            index += 1;
            continue;
        }
        if command_name == "xargs" && (!word.starts_with('-') || word == "--") {
            let target = if word == "--" {
                argv.get(index + 1).map(String::as_str)
            } else {
                Some(word.as_str())
            };
            if target.is_none_or(|name| !XARGS_TARGET_COMMANDS.contains(name)) {
                return false;
            }
            break;
        }
        if word == "--" {
            if !policy.respects_double_dash {
                index += 1;
                continue;
            }
            break;
        }
        if (command_name == "head" || command_name == "tail") && is_compact_num(word) {
            index += 1;
            continue;
        }
        if policy.allow_compact_numeric_count_flag && is_compact_num(word) {
            index += 1;
            continue;
        }
        if word.starts_with('-') && word.len() > 1 && is_option_like(word) {
            let (flag, inline) = match word.split_once('=') {
                Some((f, v)) => (f, Some(v)),
                None => (word.as_str(), None),
            };
            if let Some(kind) = safe_flags.get(flag) {
                match kind {
                    SafeFlagValue::None => {
                        if inline.is_some() {
                            return false;
                        }
                        index += 1;
                        continue;
                    }
                    SafeFlagValue::OptionalString => {
                        index += 1;
                        continue;
                    }
                    other => {
                        let value = inline.or_else(|| argv.get(index + 1).map(String::as_str));
                        let Some(value) = value else {
                            return false;
                        };
                        if *other == SafeFlagValue::String
                            && inline.is_none()
                            && value.starts_with('-')
                            && value.len() > 1
                            && is_option_like(value)
                        {
                            return false;
                        }
                        if !matches_kind(value, *other) {
                            return false;
                        }
                        index += if inline.is_some() { 1 } else { 2 };
                        continue;
                    }
                }
            }
            if let Some((_, kind, value)) = short_attached(word, safe_flags) {
                if value.starts_with('-') && value.len() > 1 && is_option_like(value) {
                    return false;
                }
                if !matches_kind(value, kind) {
                    return false;
                }
                index += 1;
                continue;
            }
            if is_short_cluster(flag, safe_flags) {
                index += 1;
                continue;
            }
            return false;
        }
        index += 1;
    }
    true
}

fn is_compact_num(word: &str) -> bool {
    word.starts_with('-') && word.len() > 1 && word[1..].chars().all(|c| c.is_ascii_digit())
}

fn short_attached<'a>(
    word: &'a str,
    safe_flags: &HashMap<&'static str, SafeFlagValue>,
) -> Option<(&'a str, SafeFlagValue, &'a str)> {
    if !word.starts_with('-') || word.starts_with("--") || word.len() <= 2 {
        return None;
    }
    let flag = &word[..2];
    let kind = *safe_flags.get(flag)?;
    if kind == SafeFlagValue::None {
        return None;
    }
    Some((flag, kind, &word[2..]))
}

fn is_short_cluster(flag: &str, safe_flags: &HashMap<&'static str, SafeFlagValue>) -> bool {
    if !flag.starts_with('-') || flag.starts_with("--") || flag.len() <= 2 {
        return false;
    }
    flag[1..].chars().all(|name| {
        let key = format!("-{name}");
        safe_flags.get(key.as_str()) == Some(&SafeFlagValue::None)
    })
}

fn matches_kind(value: &str, kind: SafeFlagValue) -> bool {
    match kind {
        SafeFlagValue::None => false,
        SafeFlagValue::Number => !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()),
        SafeFlagValue::OptionalString | SafeFlagValue::String => true,
        SafeFlagValue::Char => value.chars().count() == 1,
        SafeFlagValue::BracePair => value == "{}",
        SafeFlagValue::Eof => value == "EOF",
    }
}
