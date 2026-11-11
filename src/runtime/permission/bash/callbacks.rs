//! Command-specific danger callbacks ported from ZCode bash-readonly-policy-callbacks.

use super::types::DangerCallbackId;
use std::collections::HashSet;
use std::sync::LazyLock;

pub static XARGS_TARGET_COMMANDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "echo", "printf", "wc", "grep", "egrep", "fgrep", "head", "tail",
    ]
    .into_iter()
    .collect()
});

#[must_use]
pub fn run_danger_callback(id: DangerCallbackId, _command_text: &str, args: &[String]) -> bool {
    match id {
        DangerCallbackId::Date => date_dangerous(args),
        DangerCallbackId::Hostname => hostname_dangerous(args),
        DangerCallbackId::Jq => jq_dangerous(args),
        DangerCallbackId::Lsof => lsof_dangerous(args),
        DangerCallbackId::Man => man_dangerous(args),
        DangerCallbackId::Ps => args.iter().any(|a| {
            !a.starts_with('-') && a.contains('e') && a.chars().all(|c| c.is_ascii_alphabetic())
        }),
        DangerCallbackId::Pyright => args.iter().any(|a| a == "--watch" || a == "-w"),
        DangerCallbackId::Sed => sed_dangerous(args),
        DangerCallbackId::Ss => ss_dangerous(args),
        DangerCallbackId::Test => test_dangerous(args),
        DangerCallbackId::Tput => tput_dangerous(args),
        DangerCallbackId::Xargs => xargs_dangerous(args),
        DangerCallbackId::Docker => has_dangerous_docker_option(args),
        DangerCallbackId::Gh => gh_dangerous(args),
        DangerCallbackId::GitBranch => git_list_like(
            args,
            &["--contains", "--no-contains", "--points-at", "--sort"],
        ),
        DangerCallbackId::GitTag => git_list_like(
            args,
            &[
                "--contains",
                "--no-contains",
                "--merged",
                "--no-merged",
                "--points-at",
                "--sort",
                "--format",
                "-n",
            ],
        ),
        DangerCallbackId::GitRevisionFormat => git_revision_format(args),
        DangerCallbackId::GitReflog => {
            let allowed = ["show", "list"];
            let dangerous = ["expire", "delete", "exists", "drop", "write"];
            if let Some(first) = args.iter().find(|a| !a.is_empty() && !a.starts_with('-')) {
                if !allowed.contains(&first.as_str()) {
                    return true;
                }
            }
            args.iter().any(|a| dangerous.contains(&a.as_str()))
        }
        DangerCallbackId::GitLsRemote => {
            let mut after = false;
            let mut i = 0;
            while i < args.len() {
                let arg = &args[i];
                if !after && arg == "--" {
                    after = true;
                    i += 1;
                    continue;
                }
                if !after && (arg.starts_with('-') || arg.is_empty()) {
                    if arg == "--sort" {
                        i += 1;
                    }
                    i += 1;
                    continue;
                }
                return true;
            }
            false
        }
        DangerCallbackId::GitRemoteShow => {
            let has_n = args.iter().any(|a| a == "-n");
            if !has_n {
                return true;
            }
            let positional: Vec<_> = args
                .iter()
                .filter(|a| a.as_str() != "-n" && a.as_str() != "--")
                .cloned()
                .collect();
            if positional.len() != 1 {
                return true;
            }
            let name = &positional[0];
            let mut chars = name.chars();
            !matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric() || c == '_')
                || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        }
        DangerCallbackId::GitRemoteOnlyVerbose => {
            args.iter().any(|a| a != "-v" && a != "--verbose")
        }
    }
}

#[must_use]
pub fn is_sed_in_place_option(word: &str) -> bool {
    word.starts_with("-i") || word == "--in-place" || word.starts_with("--in-place=")
}

#[must_use]
pub fn has_dangerous_docker_option(args: &[String]) -> bool {
    const DANGEROUS: &[&str] = &[
        "-H",
        "-c",
        "--config",
        "--context",
        "--host",
        "--tlscacert",
        "--tlscert",
        "--tlskey",
    ];
    args.iter().any(|arg| {
        DANGEROUS.iter().any(|flag| {
            arg == flag
                || arg.starts_with(&format!("{flag}="))
                || (flag.len() == 2 && arg.len() > 2 && arg.starts_with(flag))
        }) || arg.strip_prefix('-').is_some_and(|rest| {
            !rest.starts_with('-')
                && rest.len() >= 2
                && rest.chars().any(|c| matches!(c, 'H' | 'c'))
        })
    })
}

fn sed_dangerous(args: &[String]) -> bool {
    let quiet_flags = ["-n", "--quiet", "--silent"];
    let zero_flags = ["-z", "--zero-terminated"];
    let mut index = 0;
    let mut quiet = false;
    let mut zero = false;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            index += 1;
            break;
        }
        if !arg.starts_with('-') {
            break;
        }
        let flags: Vec<String> = if arg.starts_with("--") {
            vec![arg.clone()]
        } else {
            arg[1..].chars().map(|c| format!("-{c}")).collect()
        };
        if flags.is_empty() || flags.iter().any(|f| !sed_safe(f)) {
            return true;
        }
        quiet |= flags.iter().any(|f| quiet_flags.contains(&f.as_str()));
        zero |= flags.iter().any(|f| zero_flags.contains(&f.as_str()));
        index += 1;
    }
    let Some(script) = args.get(index) else {
        return true;
    };
    if script.chars().any(|c| c.is_control()) {
        return true;
    }
    let files = &args[index + 1..];
    if files
        .iter()
        .any(|f| f.starts_with('-') && f.as_str() != "-")
    {
        return true;
    }
    if quiet && script.split(';').all(|c| is_sed_print(c.trim())) {
        return false;
    }
    quiet || zero || !files.is_empty() || !is_readonly_sub(script)
}

fn sed_safe(flag: &str) -> bool {
    matches!(
        flag,
        "-n" | "--quiet"
            | "--silent"
            | "-E"
            | "-r"
            | "--regexp-extended"
            | "--posix"
            | "-z"
            | "--zero-terminated"
    )
}

fn is_sed_print(command: &str) -> bool {
    if command == "p" {
        return true;
    }
    let Some(rest) = command.strip_suffix('p') else {
        return false;
    };
    if rest.is_empty() {
        return true;
    }
    if let Some((a, b)) = rest.split_once(',') {
        return is_pos_int(a) && is_pos_int(b);
    }
    is_pos_int(rest)
}

fn is_pos_int(v: &str) -> bool {
    let mut c = v.chars();
    matches!(c.next(), Some(ch) if ch.is_ascii_digit() && ch != '0')
        && c.all(|ch| ch.is_ascii_digit())
}

fn is_readonly_sub(script: &str) -> bool {
    if !script.starts_with("s/") || script.contains(';') {
        return false;
    }
    let mut delimiters = 0;
    let mut last = None;
    let bytes = script.as_bytes();
    let mut i = 2;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 1;
            let Some(esc) = script.get(i..=i) else {
                return false;
            };
            if !"/\\&123456789.^$*+?()[]{}|".contains(esc) {
                return false;
            }
        } else if bytes[i] == b'/' {
            delimiters += 1;
            last = Some(i);
        }
        i += 1;
    }
    if delimiters != 2 {
        return false;
    }
    let flags = &script[last.unwrap() + 1..];
    flags
        .chars()
        .all(|c| matches!(c, 'g' | 'p' | 'i' | 'm' | 'I' | 'M' | '1'..='9'))
        && flags.chars().filter(|c| c.is_ascii_digit()).count() <= 1
}

fn jq_dangerous(args: &[String]) -> bool {
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg.is_empty() {
            i += 1;
            continue;
        }
        if arg == "-f"
            || arg.starts_with("-f")
            || arg == "-L"
            || arg.starts_with("-L")
            || arg.starts_with("--argfile")
            || arg.starts_with("--from-file")
            || arg.starts_with("--library-path")
            || arg.starts_with("--rawfile")
            || arg.starts_with("--run-tests")
            || arg.starts_with("--slurpfile")
        {
            return true;
        }
        if arg == "--" {
            return jq_filter(args.get(i + 1).map(String::as_str).unwrap_or(""));
        }
        if arg == "--indent" {
            i += 2;
            continue;
        }
        if arg.starts_with('-') {
            i += 1;
            continue;
        }
        return jq_filter(arg);
    }
    false
}

fn jq_filter(filter: &str) -> bool {
    filter.contains("$ENV")
        || filter
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '$')
            .any(|t| t == "env")
        || filter
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .any(|t| t == "include" || t == "import")
}

fn date_dangerous(args: &[String]) -> bool {
    let value_flags = ["-d", "--date", "-r", "--reference", "--rfc-3339"];
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg.starts_with("--") && arg.contains('=') {
            i += 1;
            continue;
        }
        if arg.starts_with('-') {
            i += if value_flags.contains(&arg.as_str()) {
                2
            } else {
                1
            };
            continue;
        }
        if !arg.starts_with('+') {
            return true;
        }
        i += 1;
    }
    false
}

/// GNU/BSD `hostname NAME` sets the host name. Safe flags are query-only;
/// any positional operand (including after `--`, a lone `-`, or `""`) is a
/// mutation — those forms are still NAME operands to `sethostname`.
fn hostname_dangerous(args: &[String]) -> bool {
    let mut after_dd = false;
    for arg in args {
        if after_dd {
            return true;
        }
        if arg == "--" {
            after_dd = true;
            continue;
        }
        // Empty and lone `-` are not query flags; GNU/BSD treat them as NAME.
        if arg.is_empty() || arg == "-" || !arg.starts_with('-') {
            return true;
        }
    }
    after_dd
}

fn man_dangerous(args: &[String]) -> bool {
    let apropos = ["-k", "-f", "--apropos", "--whatis"];
    let value = ["-S", "-s"];
    let mut is_apropos = false;
    let mut after = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if !after && arg == "--" {
            after = true;
            i += 1;
            continue;
        }
        if !after && arg.starts_with('-') && arg != "-" {
            if apropos.contains(&arg.as_str()) {
                is_apropos = true;
            }
            if value.contains(&arg.as_str()) {
                i += 1;
            }
            i += 1;
            continue;
        }
        after = true;
        if arg.contains('/') {
            return !is_apropos;
        }
        i += 1;
    }
    false
}

fn lsof_dangerous(args: &[String]) -> bool {
    for (i, arg) in args.iter().enumerate() {
        if arg == "+m" || arg.starts_with("+m") {
            return true;
        }
        if arg.contains('@') && arg.contains('i') {
            if let Some(host) = arg.split('@').nth(1).and_then(|r| r.split(':').next()) {
                if host.chars().any(|c| c.is_ascii_alphabetic()) {
                    return true;
                }
            }
        }
        if arg.starts_with('-')
            && !arg.starts_with("--")
            && arg.ends_with('i')
            && arg[1..].chars().all(|c| c.is_ascii_alphabetic())
        {
            if let Some(next) = args.get(i + 1) {
                if next.contains('@') {
                    if let Some(host) = next.split('@').nth(1).and_then(|r| r.split(':').next()) {
                        if host.chars().any(|c| c.is_ascii_alphabetic()) {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

fn tput_dangerous(args: &[String]) -> bool {
    static DANGEROUS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
        [
            "clear", "flash", "if", "init", "iprog", "is1", "is2", "is3", "mc0", "mc4", "mc5",
            "mc5i", "mc5p", "pfkey", "pfloc", "pfx", "pfxl", "reset", "rf", "rmcup", "rs1", "rs2",
            "rs3", "smcup",
        ]
        .into_iter()
        .collect()
    });
    let mut after = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            after = true;
            i += 1;
            continue;
        }
        if !after && arg.starts_with('-') {
            if arg == "-S" || (!arg.starts_with("--") && arg.len() > 2 && arg.contains('S')) {
                return true;
            }
            i += if arg == "-T" { 2 } else { 1 };
            continue;
        }
        if DANGEROUS.contains(arg.as_str()) {
            return true;
        }
        i += 1;
    }
    false
}

fn ss_dangerous(args: &[String]) -> bool {
    let keywords = [
        "dst",
        "src",
        "dport",
        "sport",
        "and",
        "or",
        "not",
        "eq",
        "ne",
        "ge",
        "le",
        "gt",
        "lt",
        "autobound",
        "state",
        "exclude",
        "dev",
        "fwmark",
        "cgroup",
    ];
    let value_kw = [
        "state", "exclude", "dport", "sport", "dev", "fwmark", "cgroup",
    ];
    let value_flags = ["-f", "--family", "-A", "--query", "--socket"];
    let mut pos = Vec::new();
    let mut after = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if !after && arg == "--" {
            after = true;
            i += 1;
            continue;
        }
        if !after && arg.starts_with('-') {
            if value_flags.contains(&arg.as_str()) {
                i += 1;
            }
            i += 1;
            continue;
        }
        pos.push(arg.as_str());
        i += 1;
    }
    let joined = pos.join(" ");
    let tokens: Vec<_> = joined
        .split(|c: char| c.is_whitespace() || "()=!<>&|,".contains(c))
        .filter(|t| !t.is_empty())
        .collect();
    let mut skip = false;
    for token in tokens {
        if skip {
            skip = false;
            continue;
        }
        if keywords.contains(&token) {
            skip = value_kw.contains(&token);
            continue;
        }
        if token.chars().any(|c| matches!(c, 'g'..='z' | 'G'..='Z'))
            || (token.chars().any(|c| matches!(c, 'a'..='f' | 'A'..='F'))
                && (token.contains('.') || !token.contains(':')))
        {
            return true;
        }
    }
    false
}

fn test_dangerous(args: &[String]) -> bool {
    if args
        .iter()
        .any(|a| a == "-v" || a == "-R" || a == "-a" || a == "-o" || a.contains('['))
    {
        return true;
    }
    let numeric = ["-eq", "-ne", "-lt", "-le", "-gt", "-ge"];
    for (i, arg) in args.iter().enumerate() {
        if numeric.contains(&arg.as_str()) {
            for v in [args.get(i.wrapping_sub(1)), args.get(i + 1)]
                .into_iter()
                .flatten()
            {
                if !is_test_num(v) {
                    return true;
                }
            }
        }
        if arg == "-t" {
            if let Some(v) = args.get(i + 1) {
                if !is_test_num(v) {
                    return true;
                }
            }
        }
    }
    false
}

fn is_test_num(value: &str) -> bool {
    let value = value.strip_prefix('-').unwrap_or(value);
    if value.is_empty() {
        return false;
    }
    if let Some(rest) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        return !rest.is_empty() && rest.chars().all(|c| c.is_ascii_hexdigit());
    }
    if let Some((l, r)) = value.split_once('#') {
        return !l.is_empty()
            && l.chars().all(|c| c.is_ascii_digit())
            && !r.is_empty()
            && r.chars().all(|c| c.is_ascii_alphanumeric());
    }
    value.chars().all(|c| c.is_ascii_digit())
}

fn xargs_dangerous(args: &[String]) -> bool {
    let value_flags = ["-I", "-n", "-P", "-L", "-s", "-E", "-d"];
    let mut i = 0;
    while i < args.len() {
        let mut arg = args[i].as_str();
        if arg.is_empty() {
            i += 1;
            continue;
        }
        if arg == "--" && i + 1 < args.len() {
            i += 1;
            arg = args[i].as_str();
        }
        if arg.starts_with('-') && arg != "-" {
            if value_flags.contains(&arg) {
                i += 1;
            }
            i += 1;
            continue;
        }
        return !XARGS_TARGET_COMMANDS.contains(arg);
    }
    false
}

fn gh_dangerous(args: &[String]) -> bool {
    for arg in args {
        if arg.is_empty() {
            continue;
        }
        let mut value = arg.as_str();
        if arg.starts_with('-') {
            let Some((_, rest)) = arg.split_once('=') else {
                continue;
            };
            if rest.is_empty() {
                continue;
            }
            value = rest;
        }
        if !(value.contains('/') || value.contains("://") || value.contains('@')) {
            continue;
        }
        if value.contains("://") || value.contains('@') {
            return true;
        }
        if value.matches('/').count() >= 2 {
            return true;
        }
    }
    false
}

fn git_revision_format(args: &[String]) -> bool {
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let value = arg
            .split_once('=')
            .map(|(_, v)| v)
            .or_else(|| args.get(i + 1).map(String::as_str));
        if (arg == "--format"
            || arg == "--pretty"
            || arg.starts_with("--format=")
            || arg.starts_with("--pretty="))
            && value.is_some_and(|v| {
                v.contains("%G") || v.contains("%(*signature") || v.contains("%(signature")
            })
        {
            return true;
        }
        i += 1;
    }
    false
}

fn git_list_like(args: &[String], value_flags: &[&str]) -> bool {
    let mut has_list = false;
    let mut after = false;
    let mut prev = "";
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg.is_empty() {
            i += 1;
            continue;
        }
        if arg == "--" && !after {
            after = true;
            prev = "";
            i += 1;
            continue;
        }
        if !after && arg.starts_with('-') {
            if arg == "--list"
                || arg == "-l"
                || (arg.starts_with('-') && !arg.starts_with("--") && arg[1..].contains('l'))
            {
                has_list = true;
            }
            prev = arg.split_once('=').map_or(arg.as_str(), |(f, _)| f);
            if !arg.contains('=') && value_flags.contains(&prev) {
                i += 1;
            }
            i += 1;
            continue;
        }
        if !has_list && prev != "--merged" && prev != "--no-merged" {
            return true;
        }
        i += 1;
    }
    false
}
