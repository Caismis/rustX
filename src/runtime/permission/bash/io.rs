//! Env assignment, redirect, and wrapper stripping rules (ZCode argv-io).

use super::types::BashCommandInvocation;
use std::collections::HashSet;
use std::sync::LazyLock;

static SAFE_ENV_ASSIGNMENTS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "ANTHROPIC_API_KEY",
        "BLOCK_SIZE",
        "BLOCKSIZE",
        "CGO_ENABLED",
        "CHARSET",
        "CI",
        "CLICOLOR",
        "CLICOLOR_FORCE",
        "COLORTERM",
        "COLUMNS",
        "DEBIAN_FRONTEND",
        "FORCE_COLOR",
        "GCC_COLORS",
        "GIT_TERMINAL_PROMPT",
        "GO111MODULE",
        "GOARCH",
        "GOEXPERIMENT",
        "GOOS",
        "GREP_COLOR",
        "GREP_COLORS",
        "LANG",
        "LANGUAGE",
        "LC_ALL",
        "LC_CTYPE",
        "LC_TIME",
        "LINES",
        "LSCOLORS",
        "LS_COLORS",
        "NO_COLOR",
        "NODE_ENV",
        "PYTEST_DEBUG",
        "PYTEST_DISABLE_PLUGIN_AUTOLOAD",
        "PYTHONDONTWRITEBYTECODE",
        "PYTHONUNBUFFERED",
        "RUST_BACKTRACE",
        "RUST_LOG",
        "TERM",
        "TIME_STYLE",
        "TZ",
    ]
    .into_iter()
    .collect()
});

fn is_dev_tcp_udp(target: &str) -> bool {
    target.starts_with("/dev/tcp/") || target.starts_with("/dev/udp/")
}

fn looks_like_unc(value: &str) -> bool {
    let rest = value
        .strip_prefix("//")
        .or_else(|| value.strip_prefix(r"\\"));
    rest.is_some_and(|r| r.chars().next().is_some_and(|c| c != '/' && c != '\\'))
}

#[must_use]
pub fn are_env_assignments_allowed(command: &BashCommandInvocation) -> bool {
    command.env_assignments.iter().all(|a| {
        a.name
            .as_deref()
            .is_some_and(|n| SAFE_ENV_ASSIGNMENTS.contains(n))
            && !a.value_dynamic
    })
}

#[must_use]
pub fn are_redirects_allowed(command: &BashCommandInvocation) -> bool {
    for redirect in &command.redirects {
        if redirect.dynamic || is_dev_tcp_udp(&redirect.target) {
            return false;
        }
        if redirect.operator == ">&" && redirect.target.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if redirect.target == "/dev/null" {
            continue;
        }
        if matches!(
            redirect.operator.as_str(),
            "<" | "<<" | "<<-" | "<&" | "<<<"
        ) {
            if looks_like_unc(&redirect.target) {
                return false;
            }
            continue;
        }
        return false;
    }
    true
}

#[must_use]
pub fn is_unsafe_windows_unc_path(value: &str) -> bool {
    looks_like_unc(value)
}

#[must_use]
pub fn strip_safe_command_wrappers(argv: &[String]) -> Vec<String> {
    let mut stripped = argv.to_vec();
    loop {
        match stripped.first().map(String::as_str) {
            Some("command") => {
                let mut index = 1;
                while stripped.get(index).is_some_and(|w| is_command_p_flag(w)) {
                    index += 1;
                }
                if stripped.get(index).map(String::as_str) == Some("--") {
                    index += 1;
                }
                if index >= stripped.len()
                    || stripped.get(index).is_some_and(|w| w.starts_with('-'))
                {
                    return stripped;
                }
                stripped = stripped[index..].to_vec();
            }
            Some("builtin") => {
                let index = if stripped.get(1).map(String::as_str) == Some("--") {
                    2
                } else {
                    1
                };
                if index >= stripped.len() {
                    return stripped;
                }
                stripped = stripped[index..].to_vec();
            }
            Some("noglob") => {
                if stripped.len() <= 1 {
                    return stripped;
                }
                stripped = stripped[1..].to_vec();
            }
            _ => return stripped,
        }
    }
}

#[must_use]
pub fn argv_contains_dev_tcp_udp(argv: &[String]) -> bool {
    argv.iter().any(|w| is_dev_tcp_udp(w))
}

fn is_command_p_flag(word: &str) -> bool {
    word.starts_with('-') && word.len() > 1 && word[1..].chars().all(|c| c == 'p')
}
