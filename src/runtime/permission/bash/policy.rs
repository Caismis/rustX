//! Read-only policy evaluation (ZCode `evaluateBashReadonlyPolicy` / semantics).

use super::callbacks::{is_sed_in_place_option, run_danger_callback};
use super::commands::{
    GIT_READONLY_SUBCOMMAND_POLICIES, READONLY_ALLOW_ANY_ARG_COMMANDS, READONLY_COMMAND_POLICIES,
    sorted_multiword_policies,
};
use super::direct::{evaluate_direct_readonly_argv, is_find_write_option};
use super::flags_walk::is_argv_allowed_by_policy;
use super::git::{has_dangerous_git_global_option, is_git_read_only_command};
use super::io::{
    are_env_assignments_allowed, are_redirects_allowed, argv_contains_dev_tcp_udp,
    is_unsafe_windows_unc_path, strip_safe_command_wrappers,
};
use super::types::BashCommandInvocation;

#[must_use]
pub fn evaluate_bash_readonly_policy(command: &BashCommandInvocation) -> Option<bool> {
    if !are_env_assignments_allowed(command) || !are_redirects_allowed(command) {
        return Some(false);
    }
    let argv = strip_safe_command_wrappers(&command.argv);
    if argv.is_empty() {
        return Some(false);
    }
    if argv.iter().any(|a| is_unsafe_windows_unc_path(a)) || argv_contains_dev_tcp_udp(&argv) {
        return Some(false);
    }
    if argv[0].contains('/') {
        return None;
    }
    if argv[0] == "git" {
        return Some(is_git_read_only_command(
            &argv,
            &GIT_READONLY_SUBCOMMAND_POLICIES,
        ));
    }
    if let Some(direct) = evaluate_direct_readonly_argv(&argv) {
        return Some(direct);
    }
    if let Some(prefix) = evaluate_prefix(&argv) {
        return Some(prefix);
    }
    if READONLY_ALLOW_ANY_ARG_COMMANDS.contains(argv[0].as_str()) {
        return Some(true);
    }
    let policy = READONLY_COMMAND_POLICIES.get(argv[0].as_str())?;
    if argv[0] == "cd" && argv.len() > 2 {
        return Some(false);
    }
    if let Some(cb) = policy.danger_callback {
        if run_danger_callback(cb, &command.command_text, &argv[1..]) {
            return Some(false);
        }
    }
    if !is_argv_allowed_by_policy(&argv, policy, &argv[0], 1) {
        return Some(false);
    }
    Some(true)
}

#[must_use]
pub fn has_known_bash_write_option(command: &BashCommandInvocation) -> bool {
    let argv = strip_safe_command_wrappers(&command.argv);
    match argv.first().map(String::as_str) {
        Some("sed") => argv.iter().any(|w| is_sed_in_place_option(w)),
        Some("find") => argv.iter().any(|w| is_find_write_option(w)),
        Some("tree") => tree_output(&argv),
        Some("git") => has_dangerous_git_global_option(&argv),
        _ => false,
    }
}

#[must_use]
pub fn analysis_contains_git_and_directory_change(commands: &[BashCommandInvocation]) -> bool {
    let mut git = false;
    let mut dir = false;
    for command in commands {
        let argv = strip_safe_command_wrappers(&command.argv);
        let name = argv
            .first()
            .map(String::as_str)
            .unwrap_or(command.name.as_str());
        git |= name == "git";
        dir |= matches!(name, "cd" | "pushd" | "popd");
    }
    git && dir
}

fn evaluate_prefix(argv: &[String]) -> Option<bool> {
    for (prefix, policy) in sorted_multiword_policies() {
        let words: Vec<&str> = prefix.split_whitespace().collect();
        if !words
            .iter()
            .enumerate()
            .all(|(i, w)| argv.get(i).map(String::as_str) == Some(*w))
        {
            continue;
        }
        let args = &argv[words.len()..];
        if args
            .iter()
            .any(|a| a.contains('$') || (a.contains('{') && (a.contains(',') || a.contains(".."))))
        {
            return Some(false);
        }
        if let Some(cb) = policy.danger_callback {
            if run_danger_callback(cb, prefix, args) {
                return Some(false);
            }
        }
        if !is_argv_allowed_by_policy(argv, &policy, &argv[0], words.len()) {
            return Some(false);
        }
        return Some(true);
    }
    None
}

fn tree_output(argv: &[String]) -> bool {
    for word in argv.iter().skip(1) {
        if word == "--" {
            return false;
        }
        if word == "-o" || word == "--output" || word.starts_with("--output=") {
            return true;
        }
        if word.starts_with('-') && !word.starts_with("--") && word[1..].contains('o') {
            return true;
        }
    }
    false
}
