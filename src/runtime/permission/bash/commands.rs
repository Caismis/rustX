//! Command and multiword policy tables ported from ZCode.
//!
//! Upstream pin: zai-org/ZCode@b95ebaffeab641907b38652f45c1502bd762972d (Apache-2.0).

#![allow(dead_code)]

use super::flags::*;
use super::types::{BashReadonlyCommandPolicy, DangerCallbackId, SafeFlagValue};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

fn flags_arc(
    map: &LazyLock<HashMap<&'static str, SafeFlagValue>>,
) -> Arc<HashMap<&'static str, SafeFlagValue>> {
    Arc::new((**map).clone())
}

pub static READONLY_ALLOW_ANY_ARG_COMMANDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "basename", "cal", "cat", "cmp", "column", "comm", "cut", "df", "diff", "dirname", "du",
        "expand", "expr", "false", "fmt", "fold", "free", "groups", "head", "hexdump", "id",
        "locale", "nl", "nproc", "numfmt", "od", "paste", "pr", "readlink", "realpath", "rev",
        "seq", "sleep", "stat", "strings", "tac", "tail", "tr", "true", "tsort", "type", "uname",
        "unexpand", "uptime", "wc", "which",
    ]
    .into_iter()
    .collect()
});

pub static READONLY_COMMAND_POLICIES: LazyLock<HashMap<&'static str, BashReadonlyCommandPolicy>> =
    LazyLock::new(|| {
        let mut map = HashMap::new();
        map.insert(
            "alias",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: true,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: None,
            },
        );
        map.insert(
            "base64",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: false,
                safe_flags: Some(flags_arc(&BASE64_SAFE_FLAGS)),
                danger_callback: None,
            },
        );
        map.insert(
            "cat",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&CAT_SAFE_FLAGS)),
                danger_callback: None,
            },
        );
        map.insert(
            "cd",
            BashReadonlyCommandPolicy {
                allow_any_args: true,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: None,
            },
        );
        map.insert(
            "cut",
            BashReadonlyCommandPolicy {
                allow_any_args: true,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: None,
            },
        );
        map.insert(
            "date",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&DATE_SAFE_FLAGS)),
                danger_callback: Some(DangerCallbackId::Date),
            },
        );
        map.insert(
            "diff",
            BashReadonlyCommandPolicy {
                allow_any_args: true,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: None,
            },
        );
        map.insert(
            "dirname",
            BashReadonlyCommandPolicy {
                allow_any_args: true,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: None,
            },
        );
        map.insert(
            "du",
            BashReadonlyCommandPolicy {
                allow_any_args: true,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: None,
            },
        );
        map.insert(
            "echo",
            BashReadonlyCommandPolicy {
                allow_any_args: true,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: None,
            },
        );
        map.insert(
            "env",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: true,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: None,
            },
        );
        map.insert(
            "egrep",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&GREP_SAFE_FLAGS)),
                danger_callback: None,
            },
        );
        map.insert(
            "false",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: true,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: None,
            },
        );
        map.insert(
            "fd",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&FD_SAFE_FLAGS)),
                danger_callback: None,
            },
        );
        map.insert(
            "file",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&FILE_SAFE_FLAGS)),
                danger_callback: None,
            },
        );
        map.insert(
            "find",
            BashReadonlyCommandPolicy {
                allow_any_args: true,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: None,
            },
        );
        map.insert(
            "fgrep",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&GREP_SAFE_FLAGS)),
                danger_callback: None,
            },
        );
        map.insert(
            "fdfind",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&FD_SAFE_FLAGS)),
                danger_callback: None,
            },
        );
        map.insert(
            "grep",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&GREP_SAFE_FLAGS)),
                danger_callback: None,
            },
        );
        map.insert(
            "head",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&HEAD_TAIL_SAFE_FLAGS)),
                danger_callback: None,
            },
        );
        map.insert(
            "help",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(Arc::new(HashMap::from([
                    ("-d", SafeFlagValue::None),
                    ("-m", SafeFlagValue::None),
                    ("-s", SafeFlagValue::None),
                ]))),
                danger_callback: None,
            },
        );
        map.insert(
            "hexdump",
            BashReadonlyCommandPolicy {
                allow_any_args: true,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: None,
            },
        );
        map.insert(
            "hostname",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&HOSTNAME_SAFE_FLAGS)),
                danger_callback: Some(DangerCallbackId::Hostname),
            },
        );
        map.insert(
            "id",
            BashReadonlyCommandPolicy {
                allow_any_args: true,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: None,
            },
        );
        map.insert(
            "jq",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&JQ_SAFE_FLAGS)),
                danger_callback: Some(DangerCallbackId::Jq),
            },
        );
        map.insert(
            "lsof",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&LSOF_SAFE_FLAGS)),
                danger_callback: Some(DangerCallbackId::Lsof),
            },
        );
        map.insert(
            "ls",
            BashReadonlyCommandPolicy {
                allow_any_args: true,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: None,
            },
        );
        map.insert(
            "man",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&MAN_SAFE_FLAGS)),
                danger_callback: Some(DangerCallbackId::Man),
            },
        );
        map.insert(
            "md5sum",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&SHA_SUM_SAFE_FLAGS)),
                danger_callback: None,
            },
        );
        map.insert(
            "netstat",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&NETSTAT_SAFE_FLAGS)),
                danger_callback: None,
            },
        );
        map.insert(
            "nl",
            BashReadonlyCommandPolicy {
                allow_any_args: true,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: None,
            },
        );
        map.insert(
            "pgrep",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&PGREP_SAFE_FLAGS)),
                danger_callback: None,
            },
        );
        map.insert(
            "ps",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&PS_SAFE_FLAGS)),
                danger_callback: Some(DangerCallbackId::Ps),
            },
        );
        map.insert(
            "pyright",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: false,
                safe_flags: Some(flags_arc(&PYRIGHT_SAFE_FLAGS)),
                danger_callback: Some(DangerCallbackId::Pyright),
            },
        );
        map.insert(
            "printf",
            BashReadonlyCommandPolicy {
                allow_any_args: true,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: None,
            },
        );
        map.insert(
            "pwd",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: true,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: None,
            },
        );
        map.insert(
            "readlink",
            BashReadonlyCommandPolicy {
                allow_any_args: true,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: None,
            },
        );
        map.insert(
            "realpath",
            BashReadonlyCommandPolicy {
                allow_any_args: true,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: None,
            },
        );
        map.insert(
            "rev",
            BashReadonlyCommandPolicy {
                allow_any_args: true,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: None,
            },
        );
        map.insert(
            "rg",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&RG_SAFE_FLAGS)),
                danger_callback: None,
            },
        );
        map.insert(
            "sed",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&SED_SAFE_FLAGS)),
                danger_callback: Some(DangerCallbackId::Sed),
            },
        );
        map.insert(
            "sha1sum",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&SHA_SUM_SAFE_FLAGS)),
                danger_callback: None,
            },
        );
        map.insert(
            "sha256sum",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&SHA_SUM_SAFE_FLAGS)),
                danger_callback: None,
            },
        );
        map.insert(
            "sort",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&SORT_SAFE_FLAGS)),
                danger_callback: None,
            },
        );
        map.insert(
            "ss",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&SS_SAFE_FLAGS)),
                danger_callback: Some(DangerCallbackId::Ss),
            },
        );
        map.insert(
            "stat",
            BashReadonlyCommandPolicy {
                allow_any_args: true,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: None,
            },
        );
        map.insert(
            "strings",
            BashReadonlyCommandPolicy {
                allow_any_args: true,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: None,
            },
        );
        map.insert(
            "tail",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&HEAD_TAIL_SAFE_FLAGS)),
                danger_callback: None,
            },
        );
        map.insert(
            "test",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: false,
                safe_flags: Some(flags_arc(&TEST_SAFE_FLAGS)),
                danger_callback: Some(DangerCallbackId::Test),
            },
        );
        map.insert(
            "tput",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&TPUT_SAFE_FLAGS)),
                danger_callback: Some(DangerCallbackId::Tput),
            },
        );
        map.insert(
            "tr",
            BashReadonlyCommandPolicy {
                allow_any_args: true,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: None,
            },
        );
        map.insert(
            "tree",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&TREE_SAFE_FLAGS)),
                danger_callback: None,
            },
        );
        map.insert(
            "true",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: true,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: None,
            },
        );
        map.insert(
            "uniq",
            BashReadonlyCommandPolicy {
                // `uniq INPUT OUTPUT` writes the second positional; prove
                // absence of an output operand before allowing the readonly
                // fast path.
                allow_any_args: true,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: Some(DangerCallbackId::Uniq),
            },
        );
        map.insert(
            "uname",
            BashReadonlyCommandPolicy {
                allow_any_args: true,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: None,
            },
        );
        map.insert(
            "wc",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&WC_SAFE_FLAGS)),
                danger_callback: None,
            },
        );
        map.insert(
            "which",
            BashReadonlyCommandPolicy {
                allow_any_args: true,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: None,
            },
        );
        map.insert(
            "whoami",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: true,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: None,
                danger_callback: None,
            },
        );
        map.insert(
            "xargs",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&XARGS_SAFE_FLAGS)),
                danger_callback: Some(DangerCallbackId::Xargs),
            },
        );
        map.insert(
            "[",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: false,
                safe_flags: Some(flags_arc(&TEST_SAFE_FLAGS)),
                danger_callback: Some(DangerCallbackId::Test),
            },
        );
        map
    });

pub static READONLY_MULTIWORD_COMMAND_POLICIES: LazyLock<
    Vec<(&'static str, BashReadonlyCommandPolicy)>,
> = LazyLock::new(|| {
    vec![
        (
            "docker inspect",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&DOCKER_INSPECT_SAFE_FLAGS)),
                danger_callback: Some(DangerCallbackId::Docker),
            },
        ),
        (
            "docker logs",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(flags_arc(&DOCKER_LOGS_SAFE_FLAGS)),
                danger_callback: Some(DangerCallbackId::Docker),
            },
        ),
        (
            "gh auth status",
            BashReadonlyCommandPolicy {
                allow_any_args: false,
                command_only: false,
                allow_compact_numeric_count_flag: false,
                respects_double_dash: true,
                safe_flags: Some(Arc::new(HashMap::from([
                    ("-a", SafeFlagValue::None),
                    ("-h", SafeFlagValue::String),
                    ("--active", SafeFlagValue::None),
                    ("--hostname", SafeFlagValue::String),
                    ("--json", SafeFlagValue::String),
                ]))),
                danger_callback: Some(DangerCallbackId::Gh),
            },
        ),
    ]
});

pub fn sorted_multiword_policies() -> Vec<(&'static str, BashReadonlyCommandPolicy)> {
    let mut entries = READONLY_MULTIWORD_COMMAND_POLICIES.clone();
    entries.sort_by(|left, right| {
        right
            .0
            .split_whitespace()
            .count()
            .cmp(&left.0.split_whitespace().count())
    });
    entries
}
