//! Safe-flag tables ported from ZCode bash-readonly-policy-flags-*.ts.
//!
//! Upstream pin: zai-org/ZCode@b95ebaffeab641907b38652f45c1502bd762972d (Apache-2.0).

#![allow(dead_code)]

use super::types::SafeFlagValue;
use std::collections::HashMap;
use std::sync::LazyLock;

/// Option-like argv token pattern used by the flag walker.
pub fn is_option_like(word: &str) -> bool {
    let mut chars = word.chars();
    match (chars.next(), chars.next()) {
        (Some('-'), Some(c)) => c.is_ascii_alphanumeric() || c == '_' || c == '-',
        _ => false,
    }
}

/// ZCode `TREE_SAFE_FLAGS`.
pub static TREE_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> = LazyLock::new(|| {
    HashMap::from([
        ("-A", SafeFlagValue::None),
        ("-C", SafeFlagValue::None),
        ("-D", SafeFlagValue::None),
        ("-F", SafeFlagValue::None),
        ("-H", SafeFlagValue::String),
        ("-I", SafeFlagValue::String),
        ("-J", SafeFlagValue::None),
        ("-L", SafeFlagValue::Number),
        ("-N", SafeFlagValue::None),
        ("-P", SafeFlagValue::String),
        ("-Q", SafeFlagValue::None),
        ("-S", SafeFlagValue::None),
        ("-T", SafeFlagValue::String),
        ("-U", SafeFlagValue::None),
        ("-X", SafeFlagValue::None),
        ("-a", SafeFlagValue::None),
        ("-c", SafeFlagValue::None),
        ("-d", SafeFlagValue::None),
        ("-f", SafeFlagValue::None),
        ("-g", SafeFlagValue::None),
        ("-h", SafeFlagValue::None),
        ("-i", SafeFlagValue::None),
        ("-l", SafeFlagValue::None),
        ("-n", SafeFlagValue::None),
        ("-p", SafeFlagValue::None),
        ("-q", SafeFlagValue::None),
        ("-r", SafeFlagValue::None),
        ("-s", SafeFlagValue::None),
        ("-t", SafeFlagValue::None),
        ("-u", SafeFlagValue::None),
        ("-v", SafeFlagValue::None),
        ("-x", SafeFlagValue::None),
        ("--authority", SafeFlagValue::String),
        ("--charset", SafeFlagValue::String),
        ("--device", SafeFlagValue::None),
        ("--dirsfirst", SafeFlagValue::None),
        ("--du", SafeFlagValue::None),
        ("--fflinks", SafeFlagValue::None),
        ("--filelimit", SafeFlagValue::Number),
        ("--filesfirst", SafeFlagValue::None),
        ("--fromfile", SafeFlagValue::None),
        ("--fromtabfile", SafeFlagValue::None),
        ("--gitfile", SafeFlagValue::String),
        ("--gitignore", SafeFlagValue::None),
        ("--help", SafeFlagValue::None),
        ("--hintro", SafeFlagValue::String),
        ("--houtro", SafeFlagValue::String),
        ("--hyperlink", SafeFlagValue::None),
        ("--ignore-case", SafeFlagValue::None),
        ("--inodes", SafeFlagValue::None),
        ("--info", SafeFlagValue::None),
        ("--infofile", SafeFlagValue::String),
        ("--matchdirs", SafeFlagValue::None),
        ("--metafirst", SafeFlagValue::None),
        ("--nolinks", SafeFlagValue::None),
        ("--noreport", SafeFlagValue::None),
        ("--prune", SafeFlagValue::None),
        ("--scheme", SafeFlagValue::String),
        ("--si", SafeFlagValue::None),
        ("--sort", SafeFlagValue::String),
        ("--timefmt", SafeFlagValue::String),
        ("--version", SafeFlagValue::None),
    ])
});

/// ZCode `CAT_SAFE_FLAGS`.
pub static CAT_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> = LazyLock::new(|| {
    HashMap::from([
        ("-A", SafeFlagValue::None),
        ("-E", SafeFlagValue::None),
        ("-T", SafeFlagValue::None),
        ("-b", SafeFlagValue::None),
        ("-e", SafeFlagValue::None),
        ("-n", SafeFlagValue::None),
        ("-s", SafeFlagValue::None),
        ("-t", SafeFlagValue::None),
        ("-u", SafeFlagValue::None),
        ("-v", SafeFlagValue::None),
        ("--number", SafeFlagValue::None),
        ("--number-nonblank", SafeFlagValue::None),
        ("--show-all", SafeFlagValue::None),
        ("--show-ends", SafeFlagValue::None),
        ("--show-nonprinting", SafeFlagValue::None),
        ("--show-tabs", SafeFlagValue::None),
        ("--squeeze-blank", SafeFlagValue::None),
    ])
});

/// ZCode `HEAD_TAIL_SAFE_FLAGS`.
pub static HEAD_TAIL_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> =
    LazyLock::new(|| {
        HashMap::from([
            ("-c", SafeFlagValue::Number),
            ("-n", SafeFlagValue::Number),
            ("-q", SafeFlagValue::None),
            ("-v", SafeFlagValue::None),
            ("--bytes", SafeFlagValue::Number),
            ("--lines", SafeFlagValue::Number),
            ("--quiet", SafeFlagValue::None),
            ("--silent", SafeFlagValue::None),
            ("--verbose", SafeFlagValue::None),
        ])
    });

/// ZCode `WC_SAFE_FLAGS`.
pub static WC_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> = LazyLock::new(|| {
    HashMap::from([
        ("-L", SafeFlagValue::None),
        ("-c", SafeFlagValue::None),
        ("-l", SafeFlagValue::None),
        ("-m", SafeFlagValue::None),
        ("-w", SafeFlagValue::None),
        ("--bytes", SafeFlagValue::None),
        ("--chars", SafeFlagValue::None),
        ("--lines", SafeFlagValue::None),
        ("--max-line-length", SafeFlagValue::None),
        ("--words", SafeFlagValue::None),
    ])
});

/// ZCode `RG_SAFE_FLAGS`.
pub static RG_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> = LazyLock::new(|| {
    HashMap::from([
        ("-A", SafeFlagValue::Number),
        ("-B", SafeFlagValue::Number),
        ("-C", SafeFlagValue::Number),
        ("-F", SafeFlagValue::None),
        ("-H", SafeFlagValue::None),
        ("-L", SafeFlagValue::None),
        ("-N", SafeFlagValue::None),
        ("-S", SafeFlagValue::None),
        ("-T", SafeFlagValue::String),
        ("-c", SafeFlagValue::None),
        ("-e", SafeFlagValue::String),
        ("-g", SafeFlagValue::String),
        ("-h", SafeFlagValue::None),
        ("-i", SafeFlagValue::None),
        ("-l", SafeFlagValue::None),
        ("-m", SafeFlagValue::Number),
        ("-n", SafeFlagValue::None),
        ("-s", SafeFlagValue::None),
        ("-t", SafeFlagValue::String),
        ("-u", SafeFlagValue::None),
        ("-v", SafeFlagValue::None),
        ("-w", SafeFlagValue::None),
        ("-x", SafeFlagValue::None),
        ("--after-context", SafeFlagValue::Number),
        ("--before-context", SafeFlagValue::Number),
        ("--case-sensitive", SafeFlagValue::None),
        ("--context", SafeFlagValue::Number),
        ("--count", SafeFlagValue::None),
        ("--count-matches", SafeFlagValue::None),
        ("--files", SafeFlagValue::None),
        ("--files-with-matches", SafeFlagValue::None),
        ("--fixed-strings", SafeFlagValue::None),
        ("--follow", SafeFlagValue::None),
        ("--glob", SafeFlagValue::String),
        ("--hidden", SafeFlagValue::None),
        ("--ignore-case", SafeFlagValue::None),
        ("--ignore-file", SafeFlagValue::String),
        ("--iglob", SafeFlagValue::String),
        ("--invert-match", SafeFlagValue::None),
        ("--json", SafeFlagValue::None),
        ("--line-number", SafeFlagValue::None),
        ("--line-regexp", SafeFlagValue::None),
        ("--max-count", SafeFlagValue::Number),
        ("--max-depth", SafeFlagValue::Number),
        ("--max-filesize", SafeFlagValue::String),
        ("--no-filename", SafeFlagValue::None),
        ("--no-ignore", SafeFlagValue::None),
        ("--no-ignore-parent", SafeFlagValue::None),
        ("--no-ignore-vcs", SafeFlagValue::None),
        ("--no-line-number", SafeFlagValue::None),
        ("--pcre2", SafeFlagValue::None),
        ("--regexp", SafeFlagValue::String),
        ("--smart-case", SafeFlagValue::None),
        ("--sort", SafeFlagValue::String),
        ("--sortr", SafeFlagValue::String),
        ("--type", SafeFlagValue::String),
        ("--type-not", SafeFlagValue::String),
        ("--unrestricted", SafeFlagValue::None),
        ("--with-filename", SafeFlagValue::None),
        ("--word-regexp", SafeFlagValue::None),
    ])
});

/// ZCode `GREP_SAFE_FLAGS`.
pub static GREP_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> = LazyLock::new(|| {
    HashMap::from([
        ("-A", SafeFlagValue::Number),
        ("-B", SafeFlagValue::Number),
        ("-C", SafeFlagValue::Number),
        ("-E", SafeFlagValue::None),
        ("-F", SafeFlagValue::None),
        ("-G", SafeFlagValue::None),
        ("-H", SafeFlagValue::None),
        ("-I", SafeFlagValue::None),
        ("-L", SafeFlagValue::None),
        ("-P", SafeFlagValue::None),
        ("-R", SafeFlagValue::None),
        ("-a", SafeFlagValue::None),
        ("-b", SafeFlagValue::None),
        ("-c", SafeFlagValue::None),
        ("-D", SafeFlagValue::String),
        ("-d", SafeFlagValue::String),
        ("-e", SafeFlagValue::String),
        ("-f", SafeFlagValue::String),
        ("-h", SafeFlagValue::None),
        ("-i", SafeFlagValue::None),
        ("-l", SafeFlagValue::None),
        ("-m", SafeFlagValue::Number),
        ("-n", SafeFlagValue::None),
        ("-o", SafeFlagValue::None),
        ("-q", SafeFlagValue::None),
        ("-r", SafeFlagValue::None),
        ("-s", SafeFlagValue::None),
        ("-T", SafeFlagValue::None),
        ("-u", SafeFlagValue::None),
        ("-U", SafeFlagValue::None),
        ("-v", SafeFlagValue::None),
        ("-V", SafeFlagValue::None),
        ("-w", SafeFlagValue::None),
        ("-x", SafeFlagValue::None),
        ("-Z", SafeFlagValue::None),
        ("-z", SafeFlagValue::None),
        ("--after-context", SafeFlagValue::Number),
        ("--basic-regexp", SafeFlagValue::None),
        ("--before-context", SafeFlagValue::Number),
        ("--binary", SafeFlagValue::None),
        ("--binary-files", SafeFlagValue::String),
        ("--byte-offset", SafeFlagValue::None),
        ("--color", SafeFlagValue::String),
        ("--colour", SafeFlagValue::String),
        ("--context", SafeFlagValue::Number),
        ("--count", SafeFlagValue::None),
        ("--dereference-recursive", SafeFlagValue::None),
        ("--directories", SafeFlagValue::String),
        ("--devices", SafeFlagValue::String),
        ("--exclude", SafeFlagValue::String),
        ("--exclude-dir", SafeFlagValue::String),
        ("--exclude-from", SafeFlagValue::String),
        ("--extended-regexp", SafeFlagValue::None),
        ("--file", SafeFlagValue::String),
        ("--files-with-matches", SafeFlagValue::None),
        ("--files-without-match", SafeFlagValue::None),
        ("--fixed-strings", SafeFlagValue::None),
        ("--group-separator", SafeFlagValue::String),
        ("--help", SafeFlagValue::None),
        ("--ignore-case", SafeFlagValue::None),
        ("--include", SafeFlagValue::String),
        ("--initial-tab", SafeFlagValue::None),
        ("--invert-match", SafeFlagValue::None),
        ("--label", SafeFlagValue::String),
        ("--line-buffered", SafeFlagValue::None),
        ("--line-number", SafeFlagValue::None),
        ("--line-regexp", SafeFlagValue::None),
        ("--max-count", SafeFlagValue::Number),
        ("--no-group-separator", SafeFlagValue::None),
        ("--no-filename", SafeFlagValue::None),
        ("--no-ignore-case", SafeFlagValue::None),
        ("--no-messages", SafeFlagValue::None),
        ("--null", SafeFlagValue::None),
        ("--null-data", SafeFlagValue::None),
        ("--only-matching", SafeFlagValue::None),
        ("--perl-regexp", SafeFlagValue::None),
        ("--quiet", SafeFlagValue::None),
        ("--recursive", SafeFlagValue::None),
        ("--regexp", SafeFlagValue::String),
        ("--silent", SafeFlagValue::None),
        ("--text", SafeFlagValue::None),
        ("--unix-byte-offsets", SafeFlagValue::None),
        ("--version", SafeFlagValue::None),
        ("--with-filename", SafeFlagValue::None),
        ("--word-regexp", SafeFlagValue::None),
    ])
});

/// ZCode `SED_SAFE_FLAGS`.
pub static SED_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> = LazyLock::new(|| {
    HashMap::from([
        ("-n", SafeFlagValue::None),
        ("--quiet", SafeFlagValue::None),
        ("--silent", SafeFlagValue::None),
        ("-E", SafeFlagValue::None),
        ("-r", SafeFlagValue::None),
        ("--regexp-extended", SafeFlagValue::None),
        ("--posix", SafeFlagValue::None),
        ("-z", SafeFlagValue::None),
        ("--zero-terminated", SafeFlagValue::None),
    ])
});

/// ZCode `XARGS_SAFE_FLAGS`.
pub static XARGS_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> = LazyLock::new(|| {
    HashMap::from([
        ("-0", SafeFlagValue::None),
        ("-E", SafeFlagValue::Eof),
        ("-I", SafeFlagValue::BracePair),
        ("-L", SafeFlagValue::Number),
        ("-P", SafeFlagValue::Number),
        ("-d", SafeFlagValue::Char),
        ("-n", SafeFlagValue::Number),
        ("-r", SafeFlagValue::None),
        ("-s", SafeFlagValue::Number),
        ("-t", SafeFlagValue::None),
        ("-x", SafeFlagValue::None),
    ])
});

/// ZCode `FILE_SAFE_FLAGS`.
pub static FILE_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> = LazyLock::new(|| {
    HashMap::from([
        ("-0", SafeFlagValue::None),
        ("-F", SafeFlagValue::String),
        ("-L", SafeFlagValue::None),
        ("-b", SafeFlagValue::None),
        ("-c", SafeFlagValue::None),
        ("-f", SafeFlagValue::String),
        ("-h", SafeFlagValue::None),
        ("-i", SafeFlagValue::None),
        ("-k", SafeFlagValue::None),
        ("-l", SafeFlagValue::None),
        ("-m", SafeFlagValue::String),
        ("-n", SafeFlagValue::None),
        ("-p", SafeFlagValue::None),
        ("-r", SafeFlagValue::None),
        ("-s", SafeFlagValue::None),
        ("-v", SafeFlagValue::None),
        ("--apple", SafeFlagValue::None),
        ("--brief", SafeFlagValue::None),
        ("--check-encoding", SafeFlagValue::None),
        ("--dereference", SafeFlagValue::None),
        ("--exclude", SafeFlagValue::String),
        ("--exclude-quiet", SafeFlagValue::String),
        ("--help", SafeFlagValue::None),
        ("--keep-going", SafeFlagValue::None),
        ("--list", SafeFlagValue::None),
        ("--magic-file", SafeFlagValue::String),
        ("--mime", SafeFlagValue::None),
        ("--mime-encoding", SafeFlagValue::None),
        ("--mime-type", SafeFlagValue::None),
        ("--no-buffer", SafeFlagValue::None),
        ("--no-dereference", SafeFlagValue::None),
        ("--preserve-date", SafeFlagValue::None),
        ("--print0", SafeFlagValue::None),
        ("--raw", SafeFlagValue::None),
        ("--separator", SafeFlagValue::String),
        ("--special-files", SafeFlagValue::None),
        ("--version", SafeFlagValue::None),
    ])
});

/// ZCode `JQ_SAFE_FLAGS`.
pub static JQ_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> = LazyLock::new(|| {
    HashMap::from([
        ("-C", SafeFlagValue::None),
        ("-M", SafeFlagValue::None),
        ("-S", SafeFlagValue::None),
        ("-c", SafeFlagValue::None),
        ("-e", SafeFlagValue::None),
        ("-n", SafeFlagValue::None),
        ("-r", SafeFlagValue::None),
        ("-s", SafeFlagValue::None),
        ("--color-output", SafeFlagValue::None),
        ("--compact-output", SafeFlagValue::None),
        ("--exit-status", SafeFlagValue::None),
        ("--indent", SafeFlagValue::Number),
        ("--monochrome-output", SafeFlagValue::None),
        ("--null-input", SafeFlagValue::None),
        ("--raw-output", SafeFlagValue::None),
        ("--slurp", SafeFlagValue::None),
        ("--sort-keys", SafeFlagValue::None),
        ("--tab", SafeFlagValue::None),
    ])
});

/// ZCode `FD_SAFE_FLAGS`.
pub static FD_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> = LazyLock::new(|| {
    HashMap::from([
        ("-0", SafeFlagValue::None),
        ("-1", SafeFlagValue::None),
        ("-E", SafeFlagValue::String),
        ("-F", SafeFlagValue::None),
        ("-H", SafeFlagValue::None),
        ("-I", SafeFlagValue::None),
        ("-L", SafeFlagValue::None),
        ("-S", SafeFlagValue::String),
        ("-V", SafeFlagValue::None),
        ("-a", SafeFlagValue::None),
        ("-c", SafeFlagValue::String),
        ("-d", SafeFlagValue::Number),
        ("-e", SafeFlagValue::String),
        ("-g", SafeFlagValue::None),
        ("-h", SafeFlagValue::None),
        ("-i", SafeFlagValue::None),
        ("-j", SafeFlagValue::Number),
        ("-o", SafeFlagValue::String),
        ("-p", SafeFlagValue::None),
        ("-q", SafeFlagValue::None),
        ("-s", SafeFlagValue::None),
        ("-t", SafeFlagValue::String),
        ("--absolute-path", SafeFlagValue::None),
        ("--and", SafeFlagValue::String),
        ("--base-directory", SafeFlagValue::String),
        ("--batch-size", SafeFlagValue::Number),
        ("--case-sensitive", SafeFlagValue::None),
        ("--changed-before", SafeFlagValue::String),
        ("--changed-within", SafeFlagValue::String),
        ("--color", SafeFlagValue::String),
        ("--exact-depth", SafeFlagValue::Number),
        ("--exclude", SafeFlagValue::String),
        ("--extension", SafeFlagValue::String),
        ("--fixed-strings", SafeFlagValue::None),
        ("--follow", SafeFlagValue::None),
        ("--format", SafeFlagValue::String),
        ("--full-path", SafeFlagValue::None),
        ("--glob", SafeFlagValue::None),
        ("--help", SafeFlagValue::None),
        ("--hidden", SafeFlagValue::None),
        ("--hyperlink", SafeFlagValue::String),
        ("--ignore-case", SafeFlagValue::None),
        ("--ignore-file", SafeFlagValue::String),
        ("--max-buffer-time", SafeFlagValue::String),
        ("--max-depth", SafeFlagValue::Number),
        ("--max-results", SafeFlagValue::Number),
        ("--min-depth", SafeFlagValue::Number),
        ("--no-ignore", SafeFlagValue::None),
        ("--no-ignore-parent", SafeFlagValue::None),
        ("--no-ignore-vcs", SafeFlagValue::None),
        ("--no-require-git", SafeFlagValue::None),
        ("--one-file-system", SafeFlagValue::None),
        ("--owner", SafeFlagValue::String),
        ("--path-separator", SafeFlagValue::String),
        ("--print0", SafeFlagValue::None),
        ("--prune", SafeFlagValue::None),
        ("--quiet", SafeFlagValue::None),
        ("--regex", SafeFlagValue::None),
        ("--search-path", SafeFlagValue::String),
        ("--show-errors", SafeFlagValue::None),
        ("--size", SafeFlagValue::String),
        ("--strip-cwd-prefix", SafeFlagValue::None),
        ("--threads", SafeFlagValue::Number),
        ("--type", SafeFlagValue::String),
        ("--version", SafeFlagValue::None),
    ])
});

/// ZCode `SORT_SAFE_FLAGS`.
pub static SORT_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> = LazyLock::new(|| {
    HashMap::from([
        ("-C", SafeFlagValue::None),
        ("-M", SafeFlagValue::None),
        ("-R", SafeFlagValue::None),
        ("-S", SafeFlagValue::String),
        ("-V", SafeFlagValue::None),
        ("-b", SafeFlagValue::None),
        ("-c", SafeFlagValue::None),
        ("-d", SafeFlagValue::None),
        ("-f", SafeFlagValue::None),
        ("-g", SafeFlagValue::None),
        ("-h", SafeFlagValue::None),
        ("-i", SafeFlagValue::None),
        ("-k", SafeFlagValue::String),
        ("-m", SafeFlagValue::None),
        ("-n", SafeFlagValue::None),
        ("-r", SafeFlagValue::None),
        ("-s", SafeFlagValue::None),
        ("-t", SafeFlagValue::String),
        ("-u", SafeFlagValue::None),
        ("-z", SafeFlagValue::None),
        ("--batch-size", SafeFlagValue::Number),
        ("--buffer-size", SafeFlagValue::String),
        ("--check", SafeFlagValue::None),
        ("--check-char-order", SafeFlagValue::None),
        ("--dictionary-order", SafeFlagValue::None),
        ("--field-separator", SafeFlagValue::String),
        ("--general-numeric-sort", SafeFlagValue::None),
        ("--help", SafeFlagValue::None),
        ("--human-numeric-sort", SafeFlagValue::None),
        ("--ignore-case", SafeFlagValue::None),
        ("--ignore-leading-blanks", SafeFlagValue::None),
        ("--ignore-nonprinting", SafeFlagValue::None),
        ("--key", SafeFlagValue::String),
        ("--merge", SafeFlagValue::None),
        ("--month-sort", SafeFlagValue::None),
        ("--numeric-sort", SafeFlagValue::None),
        ("--parallel", SafeFlagValue::Number),
        ("--random-sort", SafeFlagValue::None),
        ("--reverse", SafeFlagValue::None),
        ("--sort", SafeFlagValue::String),
        ("--stable", SafeFlagValue::None),
        ("--unique", SafeFlagValue::None),
        ("--version", SafeFlagValue::None),
        ("--version-sort", SafeFlagValue::None),
        ("--zero-terminated", SafeFlagValue::None),
    ])
});

/// ZCode `BASE64_SAFE_FLAGS`.
pub static BASE64_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> =
    LazyLock::new(|| {
        HashMap::from([
            ("-D", SafeFlagValue::None),
            ("-b", SafeFlagValue::Number),
            ("-d", SafeFlagValue::None),
            ("-h", SafeFlagValue::None),
            ("-i", SafeFlagValue::String),
            ("-w", SafeFlagValue::Number),
            ("--break", SafeFlagValue::Number),
            ("--decode", SafeFlagValue::None),
            ("--help", SafeFlagValue::None),
            ("--ignore-garbage", SafeFlagValue::None),
            ("--input", SafeFlagValue::String),
            ("--version", SafeFlagValue::None),
            ("--wrap", SafeFlagValue::Number),
        ])
    });

/// ZCode `SHA_SUM_SAFE_FLAGS`.
pub static SHA_SUM_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> =
    LazyLock::new(|| {
        HashMap::from([
            ("-b", SafeFlagValue::None),
            ("-c", SafeFlagValue::None),
            ("-t", SafeFlagValue::None),
            ("-w", SafeFlagValue::None),
            ("-z", SafeFlagValue::None),
            ("--binary", SafeFlagValue::None),
            ("--check", SafeFlagValue::None),
            ("--help", SafeFlagValue::None),
            ("--ignore-missing", SafeFlagValue::None),
            ("--quiet", SafeFlagValue::None),
            ("--status", SafeFlagValue::None),
            ("--strict", SafeFlagValue::None),
            ("--tag", SafeFlagValue::None),
            ("--text", SafeFlagValue::None),
            ("--version", SafeFlagValue::None),
            ("--warn", SafeFlagValue::None),
            ("--zero", SafeFlagValue::None),
        ])
    });

/// ZCode `DATE_SAFE_FLAGS`.
pub static DATE_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> = LazyLock::new(|| {
    HashMap::from([
        ("-I", SafeFlagValue::None),
        ("-R", SafeFlagValue::None),
        ("-d", SafeFlagValue::String),
        ("-r", SafeFlagValue::String),
        ("-u", SafeFlagValue::None),
        ("--date", SafeFlagValue::String),
        ("--debug", SafeFlagValue::None),
        ("--help", SafeFlagValue::None),
        ("--iso-8601", SafeFlagValue::String),
        ("--reference", SafeFlagValue::String),
        ("--rfc-3339", SafeFlagValue::String),
        ("--rfc-email", SafeFlagValue::None),
        ("--universal", SafeFlagValue::None),
        ("--utc", SafeFlagValue::None),
        ("--version", SafeFlagValue::None),
    ])
});

/// ZCode `HOSTNAME_SAFE_FLAGS`.
pub static HOSTNAME_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> =
    LazyLock::new(|| {
        HashMap::from([
            ("-A", SafeFlagValue::None),
            ("-V", SafeFlagValue::None),
            ("-a", SafeFlagValue::None),
            ("-d", SafeFlagValue::None),
            ("-f", SafeFlagValue::None),
            ("-h", SafeFlagValue::None),
            ("-i", SafeFlagValue::None),
            ("-I", SafeFlagValue::None),
            ("-s", SafeFlagValue::None),
            ("-v", SafeFlagValue::None),
            ("--alias", SafeFlagValue::None),
            ("--all-fqdns", SafeFlagValue::None),
            ("--all-ip-addresses", SafeFlagValue::None),
            ("--domain", SafeFlagValue::None),
            ("--fqdn", SafeFlagValue::None),
            ("--help", SafeFlagValue::None),
            ("--ip-address", SafeFlagValue::None),
            ("--long", SafeFlagValue::None),
            ("--short", SafeFlagValue::None),
            ("--verbose", SafeFlagValue::None),
            ("--version", SafeFlagValue::None),
        ])
    });

/// ZCode `PS_SAFE_FLAGS`.
pub static PS_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> = LazyLock::new(|| {
    HashMap::from([
        ("-A", SafeFlagValue::None),
        ("-C", SafeFlagValue::String),
        ("-F", SafeFlagValue::None),
        ("-G", SafeFlagValue::String),
        ("-H", SafeFlagValue::None),
        ("-L", SafeFlagValue::None),
        ("-N", SafeFlagValue::None),
        ("-T", SafeFlagValue::None),
        ("-U", SafeFlagValue::String),
        ("-V", SafeFlagValue::None),
        ("-a", SafeFlagValue::None),
        ("-c", SafeFlagValue::None),
        ("-d", SafeFlagValue::None),
        ("-e", SafeFlagValue::None),
        ("-f", SafeFlagValue::None),
        ("-g", SafeFlagValue::String),
        ("-j", SafeFlagValue::None),
        ("-l", SafeFlagValue::None),
        ("-m", SafeFlagValue::None),
        ("-n", SafeFlagValue::String),
        ("-p", SafeFlagValue::String),
        ("-q", SafeFlagValue::String),
        ("-s", SafeFlagValue::String),
        ("-t", SafeFlagValue::String),
        ("-u", SafeFlagValue::String),
        ("-w", SafeFlagValue::None),
        ("-ww", SafeFlagValue::None),
        ("-y", SafeFlagValue::None),
        ("--deselect", SafeFlagValue::None),
        ("--forest", SafeFlagValue::None),
        ("--headers", SafeFlagValue::None),
        ("--help", SafeFlagValue::None),
        ("--info", SafeFlagValue::None),
        ("--no-headers", SafeFlagValue::None),
        ("--pid", SafeFlagValue::String),
        ("--quick-pid", SafeFlagValue::String),
        ("--sid", SafeFlagValue::String),
        ("--sort", SafeFlagValue::String),
        ("--tty", SafeFlagValue::String),
        ("--user", SafeFlagValue::String),
        ("--version", SafeFlagValue::None),
        ("--width", SafeFlagValue::Number),
    ])
});

/// ZCode `PYRIGHT_SAFE_FLAGS`.
pub static PYRIGHT_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> =
    LazyLock::new(|| {
        HashMap::from([
            ("--dependencies", SafeFlagValue::None),
            ("--level", SafeFlagValue::String),
            ("--outputjson", SafeFlagValue::None),
            ("--pythonplatform", SafeFlagValue::String),
            ("--pythonversion", SafeFlagValue::String),
            ("--stats", SafeFlagValue::None),
            ("--verbose", SafeFlagValue::None),
            ("--version", SafeFlagValue::None),
            ("--warnings", SafeFlagValue::None),
        ])
    });

/// ZCode `MAN_SAFE_FLAGS`.
pub static MAN_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> = LazyLock::new(|| {
    HashMap::from([
        ("-S", SafeFlagValue::String),
        ("-a", SafeFlagValue::None),
        ("-d", SafeFlagValue::None),
        ("-f", SafeFlagValue::None),
        ("-h", SafeFlagValue::None),
        ("-k", SafeFlagValue::None),
        ("-s", SafeFlagValue::String),
        ("-w", SafeFlagValue::None),
        ("--all", SafeFlagValue::None),
        ("--apropos", SafeFlagValue::None),
        ("--whatis", SafeFlagValue::None),
    ])
});

/// ZCode `NETSTAT_SAFE_FLAGS`.
pub static NETSTAT_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> =
    LazyLock::new(|| {
        HashMap::from([
            ("-I", SafeFlagValue::String),
            ("-L", SafeFlagValue::None),
            ("-a", SafeFlagValue::None),
            ("-f", SafeFlagValue::String),
            ("-g", SafeFlagValue::None),
            ("-i", SafeFlagValue::None),
            ("-l", SafeFlagValue::None),
            ("-m", SafeFlagValue::None),
            ("-n", SafeFlagValue::None),
            ("-r", SafeFlagValue::None),
            ("-s", SafeFlagValue::None),
            ("-v", SafeFlagValue::None),
        ])
    });

/// ZCode `LSOF_SAFE_FLAGS`.
pub static LSOF_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> = LazyLock::new(|| {
    HashMap::from([
        ("-?", SafeFlagValue::None),
        ("-A", SafeFlagValue::String),
        ("-C", SafeFlagValue::None),
        ("-E", SafeFlagValue::None),
        ("-F", SafeFlagValue::None),
        ("-H", SafeFlagValue::None),
        ("-K", SafeFlagValue::None),
        ("-L", SafeFlagValue::None),
        ("-N", SafeFlagValue::None),
        ("-O", SafeFlagValue::None),
        ("-P", SafeFlagValue::None),
        ("-Q", SafeFlagValue::None),
        ("-R", SafeFlagValue::None),
        ("-S", SafeFlagValue::None),
        ("-T", SafeFlagValue::None),
        ("-U", SafeFlagValue::None),
        ("-V", SafeFlagValue::None),
        ("-X", SafeFlagValue::None),
        ("-a", SafeFlagValue::None),
        ("-b", SafeFlagValue::None),
        ("-c", SafeFlagValue::String),
        ("-d", SafeFlagValue::String),
        ("-e", SafeFlagValue::String),
        ("-g", SafeFlagValue::None),
        ("-h", SafeFlagValue::None),
        ("-i", SafeFlagValue::None),
        ("-k", SafeFlagValue::String),
        ("-l", SafeFlagValue::None),
        ("-n", SafeFlagValue::None),
        ("-o", SafeFlagValue::None),
        ("-p", SafeFlagValue::String),
        ("-r", SafeFlagValue::None),
        ("-s", SafeFlagValue::None),
        ("-t", SafeFlagValue::None),
        ("-u", SafeFlagValue::String),
        ("-v", SafeFlagValue::None),
        ("-x", SafeFlagValue::None),
    ])
});

/// ZCode `PGREP_SAFE_FLAGS`.
pub static PGREP_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> = LazyLock::new(|| {
    HashMap::from([
        ("-F", SafeFlagValue::String),
        ("-G", SafeFlagValue::String),
        ("-L", SafeFlagValue::None),
        ("-O", SafeFlagValue::String),
        ("-P", SafeFlagValue::String),
        ("-U", SafeFlagValue::String),
        ("-V", SafeFlagValue::None),
        ("-a", SafeFlagValue::None),
        ("-c", SafeFlagValue::None),
        ("-d", SafeFlagValue::String),
        ("-f", SafeFlagValue::None),
        ("-g", SafeFlagValue::String),
        ("-i", SafeFlagValue::None),
        ("-l", SafeFlagValue::None),
        ("-n", SafeFlagValue::None),
        ("-o", SafeFlagValue::None),
        ("-r", SafeFlagValue::String),
        ("-s", SafeFlagValue::String),
        ("-t", SafeFlagValue::String),
        ("-u", SafeFlagValue::String),
        ("-v", SafeFlagValue::None),
        ("-w", SafeFlagValue::None),
        ("-x", SafeFlagValue::None),
        ("--count", SafeFlagValue::None),
        ("--delimiter", SafeFlagValue::String),
        ("--euid", SafeFlagValue::String),
        ("--exact", SafeFlagValue::None),
        ("--full", SafeFlagValue::None),
        ("--group", SafeFlagValue::String),
        ("--help", SafeFlagValue::None),
        ("--ignore-case", SafeFlagValue::None),
        ("--inverse", SafeFlagValue::None),
        ("--lightweight", SafeFlagValue::None),
        ("--list-full", SafeFlagValue::None),
        ("--list-name", SafeFlagValue::None),
        ("--logpidfile", SafeFlagValue::None),
        ("--newest", SafeFlagValue::None),
        ("--ns", SafeFlagValue::String),
        ("--nslist", SafeFlagValue::String),
        ("--older", SafeFlagValue::String),
        ("--oldest", SafeFlagValue::None),
        ("--parent", SafeFlagValue::String),
        ("--pgroup", SafeFlagValue::String),
        ("--pidfile", SafeFlagValue::String),
        ("--runstates", SafeFlagValue::String),
        ("--session", SafeFlagValue::String),
        ("--terminal", SafeFlagValue::String),
        ("--uid", SafeFlagValue::String),
        ("--version", SafeFlagValue::None),
    ])
});

/// ZCode `TPUT_SAFE_FLAGS`.
pub static TPUT_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> = LazyLock::new(|| {
    HashMap::from([
        ("-T", SafeFlagValue::String),
        ("-V", SafeFlagValue::None),
        ("-x", SafeFlagValue::None),
    ])
});

/// ZCode `SS_SAFE_FLAGS`.
pub static SS_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> = LazyLock::new(|| {
    HashMap::from([
        ("-0", SafeFlagValue::None),
        ("-4", SafeFlagValue::None),
        ("-6", SafeFlagValue::None),
        ("-A", SafeFlagValue::String),
        ("-E", SafeFlagValue::None),
        ("-H", SafeFlagValue::None),
        ("-M", SafeFlagValue::None),
        ("-O", SafeFlagValue::None),
        ("-S", SafeFlagValue::None),
        ("-V", SafeFlagValue::None),
        ("-Z", SafeFlagValue::None),
        ("-a", SafeFlagValue::None),
        ("-b", SafeFlagValue::None),
        ("-d", SafeFlagValue::None),
        ("-e", SafeFlagValue::None),
        ("-f", SafeFlagValue::String),
        ("-h", SafeFlagValue::None),
        ("-i", SafeFlagValue::None),
        ("-l", SafeFlagValue::None),
        ("-m", SafeFlagValue::None),
        ("-n", SafeFlagValue::None),
        ("-o", SafeFlagValue::None),
        ("-p", SafeFlagValue::None),
        ("-s", SafeFlagValue::None),
        ("-t", SafeFlagValue::None),
        ("-u", SafeFlagValue::None),
        ("-w", SafeFlagValue::None),
        ("-x", SafeFlagValue::None),
        ("-z", SafeFlagValue::None),
        ("--all", SafeFlagValue::None),
        ("--bpf", SafeFlagValue::None),
        ("--cgroup", SafeFlagValue::None),
        ("--contexts", SafeFlagValue::None),
        ("--context", SafeFlagValue::None),
        ("--dccp", SafeFlagValue::None),
        ("--events", SafeFlagValue::None),
        ("--extended", SafeFlagValue::None),
        ("--family", SafeFlagValue::String),
        ("--help", SafeFlagValue::None),
        ("--inet-sockopt", SafeFlagValue::None),
        ("--info", SafeFlagValue::None),
        ("--ipv4", SafeFlagValue::None),
        ("--ipv6", SafeFlagValue::None),
        ("--listening", SafeFlagValue::None),
        ("--memory", SafeFlagValue::None),
        ("--mptcp", SafeFlagValue::None),
        ("--no-header", SafeFlagValue::None),
        ("--numeric", SafeFlagValue::None),
        ("--oneline", SafeFlagValue::None),
        ("--options", SafeFlagValue::None),
        ("--packet", SafeFlagValue::None),
        ("--processes", SafeFlagValue::None),
        ("--query", SafeFlagValue::String),
        ("--raw", SafeFlagValue::None),
        ("--sctp", SafeFlagValue::None),
        ("--socket", SafeFlagValue::String),
        ("--summary", SafeFlagValue::None),
        ("--tcp", SafeFlagValue::None),
        ("--tipc", SafeFlagValue::None),
        ("--tipcinfo", SafeFlagValue::None),
        ("--tos", SafeFlagValue::None),
        ("--udp", SafeFlagValue::None),
        ("--unix", SafeFlagValue::None),
        ("--version", SafeFlagValue::None),
        ("--vsock", SafeFlagValue::None),
    ])
});

/// ZCode `TEST_SAFE_FLAGS`.
pub static TEST_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> = LazyLock::new(|| {
    HashMap::from([
        ("-b", SafeFlagValue::String),
        ("-c", SafeFlagValue::String),
        ("-d", SafeFlagValue::String),
        ("-e", SafeFlagValue::String),
        ("-ef", SafeFlagValue::String),
        ("-eq", SafeFlagValue::String),
        ("-f", SafeFlagValue::String),
        ("-g", SafeFlagValue::String),
        ("-G", SafeFlagValue::String),
        ("-ge", SafeFlagValue::String),
        ("-gt", SafeFlagValue::String),
        ("-h", SafeFlagValue::String),
        ("-k", SafeFlagValue::String),
        ("-L", SafeFlagValue::String),
        ("-le", SafeFlagValue::String),
        ("-lt", SafeFlagValue::String),
        ("-N", SafeFlagValue::String),
        ("-n", SafeFlagValue::String),
        ("-ne", SafeFlagValue::String),
        ("-nt", SafeFlagValue::String),
        ("-O", SafeFlagValue::String),
        ("-ot", SafeFlagValue::String),
        ("-p", SafeFlagValue::String),
        ("-r", SafeFlagValue::String),
        ("-s", SafeFlagValue::String),
        ("-S", SafeFlagValue::String),
        ("-t", SafeFlagValue::String),
        ("-u", SafeFlagValue::String),
        ("-w", SafeFlagValue::String),
        ("-x", SafeFlagValue::String),
        ("-z", SafeFlagValue::String),
    ])
});

/// ZCode `DOCKER_LOGS_SAFE_FLAGS`.
pub static DOCKER_LOGS_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> =
    LazyLock::new(|| {
        HashMap::from([
            ("-f", SafeFlagValue::None),
            ("-n", SafeFlagValue::String),
            ("-t", SafeFlagValue::None),
            ("--details", SafeFlagValue::None),
            ("--follow", SafeFlagValue::None),
            ("--since", SafeFlagValue::String),
            ("--tail", SafeFlagValue::String),
            ("--timestamps", SafeFlagValue::None),
            ("--until", SafeFlagValue::String),
        ])
    });

/// ZCode `DOCKER_INSPECT_SAFE_FLAGS`.
pub static DOCKER_INSPECT_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> =
    LazyLock::new(|| {
        HashMap::from([
            ("-f", SafeFlagValue::String),
            ("-s", SafeFlagValue::None),
            ("--format", SafeFlagValue::String),
            ("--size", SafeFlagValue::None),
            ("--type", SafeFlagValue::String),
        ])
    });

/// ZCode `GH_REPO_FLAGS`.
pub static GH_REPO_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> = LazyLock::new(|| {
    HashMap::from([
        ("-R", SafeFlagValue::String),
        ("--repo", SafeFlagValue::String),
    ])
});

/// ZCode `GH_JSON_REPO_FLAGS`.
pub static GH_JSON_REPO_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> =
    LazyLock::new(|| HashMap::from([("--json", SafeFlagValue::String)]));

/// ZCode `GH_LIMIT_REPO_FLAGS`.
pub static GH_LIMIT_REPO_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> =
    LazyLock::new(|| {
        HashMap::from([
            ("-L", SafeFlagValue::Number),
            ("--limit", SafeFlagValue::Number),
        ])
    });

/// ZCode `COMMON_GIT_REF_FLAGS`.
pub static COMMON_GIT_REF_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> =
    LazyLock::new(|| {
        HashMap::from([
            ("--all", SafeFlagValue::None),
            ("--branches", SafeFlagValue::None),
            ("--remotes", SafeFlagValue::None),
            ("--tags", SafeFlagValue::None),
        ])
    });

/// ZCode `COMMON_GIT_DATE_FLAGS`.
pub static COMMON_GIT_DATE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> =
    LazyLock::new(|| {
        HashMap::from([
            ("--after", SafeFlagValue::String),
            ("--before", SafeFlagValue::String),
            ("--since", SafeFlagValue::String),
            ("--until", SafeFlagValue::String),
        ])
    });

/// ZCode `COMMON_GIT_LOG_STYLE_FLAGS`.
pub static COMMON_GIT_LOG_STYLE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> =
    LazyLock::new(|| {
        HashMap::from([
            ("--date", SafeFlagValue::String),
            ("--decorate", SafeFlagValue::None),
            ("--graph", SafeFlagValue::None),
            ("--no-decorate", SafeFlagValue::None),
            ("--oneline", SafeFlagValue::None),
            ("--relative-date", SafeFlagValue::None),
        ])
    });

/// ZCode `COMMON_GIT_COUNT_FLAGS`.
pub static COMMON_GIT_COUNT_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> =
    LazyLock::new(|| {
        HashMap::from([
            ("-n", SafeFlagValue::Number),
            ("--max-count", SafeFlagValue::Number),
        ])
    });

/// ZCode `COMMON_GIT_STAT_FLAGS`.
pub static COMMON_GIT_STAT_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> =
    LazyLock::new(|| {
        HashMap::from([
            ("--name-only", SafeFlagValue::None),
            ("--name-status", SafeFlagValue::None),
            ("--numstat", SafeFlagValue::None),
            ("--shortstat", SafeFlagValue::None),
            ("--stat", SafeFlagValue::None),
        ])
    });

/// ZCode `COMMON_GIT_COLOR_FLAGS`.
pub static COMMON_GIT_COLOR_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> =
    LazyLock::new(|| {
        HashMap::from([
            ("--color", SafeFlagValue::None),
            ("--no-color", SafeFlagValue::None),
        ])
    });

/// ZCode `COMMON_GIT_PATCH_FLAGS`.
pub static COMMON_GIT_PATCH_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> =
    LazyLock::new(|| {
        HashMap::from([
            ("-p", SafeFlagValue::None),
            ("-s", SafeFlagValue::None),
            ("--no-ext-diff", SafeFlagValue::None),
            ("--no-patch", SafeFlagValue::None),
            ("--patch", SafeFlagValue::None),
        ])
    });

/// ZCode `GIT_STATUS_SAFE_FLAGS`.
pub static GIT_STATUS_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> =
    LazyLock::new(|| {
        HashMap::from([
            ("-b", SafeFlagValue::None),
            ("-M", SafeFlagValue::String),
            ("-s", SafeFlagValue::None),
            ("-u", SafeFlagValue::String),
            ("--ahead-behind", SafeFlagValue::None),
            ("--branch", SafeFlagValue::None),
            ("--column", SafeFlagValue::None),
            ("--find-renames", SafeFlagValue::String),
            ("--ignore-submodules", SafeFlagValue::String),
            ("--ignored", SafeFlagValue::None),
            ("--long", SafeFlagValue::None),
            ("--no-ahead-behind", SafeFlagValue::None),
            ("--no-column", SafeFlagValue::None),
            ("--no-renames", SafeFlagValue::None),
            ("--porcelain", SafeFlagValue::None),
            ("--renames", SafeFlagValue::None),
            ("--short", SafeFlagValue::None),
            ("--untracked-files", SafeFlagValue::String),
            ("--verbose", SafeFlagValue::None),
            ("-v", SafeFlagValue::None),
        ])
    });

/// ZCode `GIT_DIFF_SAFE_FLAGS`.
pub static GIT_DIFF_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> =
    LazyLock::new(|| {
        HashMap::from([
            ("-B", SafeFlagValue::None),
            ("-C", SafeFlagValue::None),
            ("-D", SafeFlagValue::None),
            ("-G", SafeFlagValue::String),
            ("-M", SafeFlagValue::None),
            ("-O", SafeFlagValue::String),
            ("-R", SafeFlagValue::None),
            ("-S", SafeFlagValue::String),
            ("-l", SafeFlagValue::None),
            ("-p", SafeFlagValue::None),
            ("-s", SafeFlagValue::None),
            ("-u", SafeFlagValue::None),
            ("--cached", SafeFlagValue::None),
            ("--check", SafeFlagValue::None),
            ("--color", SafeFlagValue::None),
            ("--color-words", SafeFlagValue::None),
            ("--diff-algorithm", SafeFlagValue::String),
            ("--diff-filter", SafeFlagValue::String),
            ("--dirstat", SafeFlagValue::None),
            ("--exit-code", SafeFlagValue::None),
            ("--find-copies", SafeFlagValue::None),
            ("--find-copies-harder", SafeFlagValue::None),
            ("--find-renames", SafeFlagValue::None),
            ("--full-index", SafeFlagValue::None),
            ("--function-context", SafeFlagValue::None),
            ("--histogram", SafeFlagValue::None),
            ("--ignore-all-space", SafeFlagValue::None),
            ("--ignore-blank-lines", SafeFlagValue::None),
            ("--ignore-space-at-eol", SafeFlagValue::None),
            ("--ignore-space-change", SafeFlagValue::None),
            ("--inter-hunk-context", SafeFlagValue::Number),
            ("--irreversible-delete", SafeFlagValue::None),
            ("--minimal", SafeFlagValue::None),
            ("--name-only", SafeFlagValue::None),
            ("--name-status", SafeFlagValue::None),
            ("--no-ext-diff", SafeFlagValue::None),
            ("--no-index", SafeFlagValue::None),
            ("--no-renames", SafeFlagValue::None),
            ("--no-color", SafeFlagValue::None),
            ("--numstat", SafeFlagValue::None),
            ("--patch-with-stat", SafeFlagValue::None),
            ("--patience", SafeFlagValue::None),
            ("--pickaxe-all", SafeFlagValue::None),
            ("--pickaxe-regex", SafeFlagValue::None),
            ("--quiet", SafeFlagValue::None),
            ("--relative", SafeFlagValue::String),
            ("--shortstat", SafeFlagValue::None),
            ("--staged", SafeFlagValue::None),
            ("--stat", SafeFlagValue::None),
            ("--summary", SafeFlagValue::None),
            ("--word-diff", SafeFlagValue::None),
            ("--word-diff-regex", SafeFlagValue::String),
            ("--ws-error-highlight", SafeFlagValue::String),
        ])
    });

/// ZCode `GIT_LOG_SAFE_FLAGS`.
pub static GIT_LOG_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> =
    LazyLock::new(|| {
        HashMap::from([
            ("-G", SafeFlagValue::String),
            ("-S", SafeFlagValue::String),
            ("--abbrev-commit", SafeFlagValue::None),
            ("--ancestry-path", SafeFlagValue::None),
            ("--author", SafeFlagValue::String),
            ("--author-date-order", SafeFlagValue::None),
            ("--boundary", SafeFlagValue::None),
            ("--cherry-mark", SafeFlagValue::None),
            ("--cherry-pick", SafeFlagValue::None),
            ("--committer", SafeFlagValue::String),
            ("--date-order", SafeFlagValue::None),
            ("--dense", SafeFlagValue::None),
            ("--diff-filter", SafeFlagValue::String),
            ("--first-parent", SafeFlagValue::None),
            ("--format", SafeFlagValue::String),
            ("--full-history", SafeFlagValue::None),
            ("--grep", SafeFlagValue::String),
            ("--left-right", SafeFlagValue::None),
            ("--max-age", SafeFlagValue::Number),
            ("--min-age", SafeFlagValue::Number),
            ("--merges", SafeFlagValue::None),
            ("--no-max-parents", SafeFlagValue::None),
            ("--no-merges", SafeFlagValue::None),
            ("--no-min-parents", SafeFlagValue::None),
            ("--no-walk", SafeFlagValue::None),
            ("--pickaxe-all", SafeFlagValue::None),
            ("--pickaxe-regex", SafeFlagValue::None),
            ("--pretty", SafeFlagValue::String),
            ("--reverse", SafeFlagValue::None),
            ("--simplify-merges", SafeFlagValue::None),
            ("--skip", SafeFlagValue::Number),
            ("--source", SafeFlagValue::None),
            ("--sparse", SafeFlagValue::None),
            ("--topo-order", SafeFlagValue::None),
            ("--walk-reflogs", SafeFlagValue::None),
        ])
    });

/// ZCode `GIT_SHOW_SAFE_FLAGS`.
pub static GIT_SHOW_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> =
    LazyLock::new(|| {
        HashMap::from([
            ("-m", SafeFlagValue::None),
            ("--abbrev-commit", SafeFlagValue::None),
            ("--color-words", SafeFlagValue::None),
            ("--diff-filter", SafeFlagValue::String),
            ("--first-parent", SafeFlagValue::None),
            ("--format", SafeFlagValue::String),
            ("--pretty", SafeFlagValue::String),
            ("--quiet", SafeFlagValue::None),
            ("--raw", SafeFlagValue::None),
            ("--word-diff", SafeFlagValue::None),
            ("--word-diff-regex", SafeFlagValue::String),
        ])
    });

/// ZCode `GIT_GREP_SAFE_FLAGS`.
pub static GIT_GREP_SAFE_FLAGS: LazyLock<HashMap<&'static str, SafeFlagValue>> =
    LazyLock::new(|| {
        HashMap::from([
            ("-A", SafeFlagValue::Number),
            ("-B", SafeFlagValue::Number),
            ("-C", SafeFlagValue::Number),
            ("-E", SafeFlagValue::None),
            ("-F", SafeFlagValue::None),
            ("-G", SafeFlagValue::None),
            ("-H", SafeFlagValue::None),
            ("-L", SafeFlagValue::None),
            ("-P", SafeFlagValue::None),
            ("-c", SafeFlagValue::None),
            ("-e", SafeFlagValue::String),
            ("-h", SafeFlagValue::None),
            ("-i", SafeFlagValue::None),
            ("-l", SafeFlagValue::None),
            ("-n", SafeFlagValue::None),
            ("-o", SafeFlagValue::None),
            ("-q", SafeFlagValue::None),
            ("-v", SafeFlagValue::None),
            ("-w", SafeFlagValue::None),
            ("--after-context", SafeFlagValue::Number),
            ("--and", SafeFlagValue::None),
            ("--basic-regexp", SafeFlagValue::None),
            ("--break", SafeFlagValue::None),
            ("--cached", SafeFlagValue::None),
            ("--color", SafeFlagValue::None),
            ("--context", SafeFlagValue::Number),
            ("--count", SafeFlagValue::None),
            ("--extended-regexp", SafeFlagValue::None),
            ("--files-with-matches", SafeFlagValue::None),
            ("--files-without-match", SafeFlagValue::None),
            ("--fixed-strings", SafeFlagValue::None),
            ("--full-name", SafeFlagValue::None),
            ("--heading", SafeFlagValue::None),
            ("--ignore-case", SafeFlagValue::None),
            ("--invert-match", SafeFlagValue::None),
            ("--line-number", SafeFlagValue::None),
            ("--max-depth", SafeFlagValue::Number),
            ("--no-color", SafeFlagValue::None),
            ("--no-index", SafeFlagValue::None),
            ("--not", SafeFlagValue::None),
            ("--only-matching", SafeFlagValue::None),
            ("--or", SafeFlagValue::None),
            ("--perl-regexp", SafeFlagValue::None),
            ("--quiet", SafeFlagValue::None),
            ("--recurse-submodules", SafeFlagValue::None),
            ("--threads", SafeFlagValue::Number),
            ("--untracked", SafeFlagValue::None),
            ("--word-regexp", SafeFlagValue::None),
        ])
    });
