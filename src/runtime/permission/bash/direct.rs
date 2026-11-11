//! Direct argv branches (find/printf/exact commands) from ZCode argv-direct.

use super::callbacks::has_dangerous_docker_option;
use std::collections::HashSet;
use std::sync::LazyLock;

static FIND_WRITE_OPTIONS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "-delete", "-exec", "-execdir", "-files0-from", "-fls", "-fprint", "-fprint0",
        "-fprintf", "-ok", "-okdir",
    ]
    .into_iter()
    .collect()
});

static FIND_VALUE_OPTIONS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "-Bmin", "-Bnewer", "-Btime", "-D", "-amin", "-anewer", "-atime", "-cmin", "-cnewer",
        "-context", "-ctime", "-f", "-flags", "-fstype", "-gid", "-group", "-ilname", "-iname",
        "-inum", "-ipath", "-iregex", "-iwholename", "-lname", "-links", "-maxdepth", "-mindepth",
        "-mmin", "-mnewer", "-mtime", "-name", "-newer", "-path", "-perm", "-printf", "-regex",
        "-regextype", "-samefile", "-size", "-type", "-used", "-user", "-wholename", "-xattrname",
        "-xtype", "-uid",
    ]
    .into_iter()
    .collect()
});

#[must_use]
pub fn is_find_write_option(word: &str) -> bool {
    FIND_WRITE_OPTIONS.contains(word)
}

#[must_use]
pub fn evaluate_direct_readonly_argv(argv: &[String]) -> Option<bool> {
    const EXACT: &[&[&str]] = &[
        &["ip", "addr"],
        &["node", "-v"],
        &["node", "--version"],
        &["python", "--version"],
        &["python3", "--version"],
    ];
    if EXACT.iter().any(|expected| {
        argv.len() == expected.len() && expected.iter().zip(argv).all(|(e, a)| *e == a.as_str())
    }) {
        return Some(true);
    }
    if argv.first().map(String::as_str) == Some("docker")
        && argv.len() >= 2
        && matches!(argv[1].as_str(), "images" | "ps")
    {
        return Some(!has_dangerous_docker_option(argv));
    }
    match argv.first().map(String::as_str) {
        Some("printf") => Some(is_safe_printf_argv(argv)),
        Some("find") => Some(is_safe_find_argv(argv)),
        Some("history") => Some(
            argv.len() == 1
                || (argv.len() == 2
                    && !argv[1].is_empty()
                    && argv[1].chars().all(|c| c.is_ascii_digit())),
        ),
        Some("arch") => {
            Some(argv.len() == 1 || (argv.len() == 2 && matches!(argv[1].as_str(), "-h" | "--help")))
        }
        Some("ifconfig") => Some(
            argv.len() == 1
                || (argv.len() == 2
                    && argv[1]
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphabetic())),
        ),
        _ => None,
    }
}

fn is_safe_printf_argv(argv: &[String]) -> bool {
    if argv
        .get(1)
        .is_some_and(|arg| arg.starts_with('-') && arg != "--")
    {
        return false;
    }
    let format_index = if argv.get(1).map(String::as_str) == Some("--") {
        2
    } else {
        1
    };
    let format = argv.get(format_index).map(String::as_str).unwrap_or("");
    !format.contains('$')
}

fn is_safe_find_argv(argv: &[String]) -> bool {
    let mut index = 1;
    while index < argv.len() {
        let word = &argv[index];
        if is_find_write_option(word) {
            return false;
        }
        if FIND_VALUE_OPTIONS.contains(word.as_str()) || is_newer_family(word) {
            index += 2;
            continue;
        }
        index += 1;
    }
    true
}

fn is_newer_family(word: &str) -> bool {
    let Some(rest) = word.strip_prefix("-newer") else {
        return false;
    };
    let mut chars = rest.chars();
    matches!(chars.next(), Some('a' | 'B' | 'c' | 'm'))
        && matches!(chars.next(), Some('a' | 'B' | 'c' | 'm' | 't'))
        && chars.next().is_none()
}
