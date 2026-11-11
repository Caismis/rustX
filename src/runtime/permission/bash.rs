//! ZCode-derived Bash read-only analysis (Issue #544).
//!
//! Parse failures, unsupported constructs, dynamic executable names, and
//! unresolved behavior never count as proven read-only. Unproven is distinct
//! from known-malicious: Plan elevates and Automatic reviews.

use serde::{Deserialize, Serialize};

/// Structured classification of one Bash command for permission admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BashReadonlyClass {
    /// Every analyzed member is proven read-only under the native checker.
    Readonly,
    /// Analysis could not prove read-only behavior.
    Unproven {
        /// Bounded deterministic explanation for audit/review.
        reason: String,
    },
}

/// Structured analysis facts retained for review and audit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BashReadonlyAnalysis {
    /// Final classification.
    pub class: BashReadonlyClass,
    /// Normalized command text that was analyzed.
    pub command: String,
}

/// Analyzes whether a Bash command is proven read-only.
///
/// Conservative admission only: unknown / unsupported constructs are
/// unproven. This is not an OS sandbox.
#[must_use]
pub fn analyze_bash_readonly(command: &str) -> BashReadonlyAnalysis {
    let command = command.trim().to_owned();
    if command.is_empty() {
        return BashReadonlyAnalysis {
            class: BashReadonlyClass::Unproven {
                reason: "empty Bash command is not proven read-only".to_owned(),
            },
            command,
        };
    }

    // Initial conservative checker: reject clearly mutating builtins/prefixes
    // and require a small allowlisted read-only command surface. Full
    // brush-parser AST coverage lands with the ZCode corpus port.
    let class = classify_conservative(&command);
    BashReadonlyAnalysis { class, command }
}

fn classify_conservative(command: &str) -> BashReadonlyClass {
    if command.contains('`')
        || command.contains("$(")
        || command.contains("${")
        || command.contains(">(")
        || command.contains("<(")
        || command.contains("<<")
    {
        return BashReadonlyClass::Unproven {
            reason: "command substitutions, process substitutions, and heredocs are not proven read-only"
                .to_owned(),
        };
    }
    if command.contains('>') || command.contains(">>") {
        return BashReadonlyClass::Unproven {
            reason: "redirects are not proven read-only".to_owned(),
        };
    }
    if command.contains('|')
        || command.contains('&')
        || command.contains(';')
        || command.contains('\n')
        || command.contains('\r')
    {
        // Pipelines/chains/background jobs/newlines require every member to
        // be proven; until the AST walker lands, treat them as unproven rather
        // than partially trusting a safe prefix. Newlines and bare `&` must
        // not collapse into argv of an allowlisted head via split_whitespace.
        return BashReadonlyClass::Unproven {
            reason: "pipelines and command chains require full AST analysis".to_owned(),
        };
    }
    if command.contains('\'') || command.contains('"') || command.contains('\\') {
        // Whitespace splitting retains quote/escape characters, so exact flag
        // matches miss forms like `'-exec'` / `'-i'` while the shell still
        // runs the mutating option. Unsupported quoting stays unproven.
        return BashReadonlyClass::Unproven {
            reason: "quoted or escaped shell syntax requires full argv analysis".to_owned(),
        };
    }

    let mut tokens = command.split_whitespace();
    let Some(executable) = tokens.next() else {
        return BashReadonlyClass::Unproven {
            reason: "missing executable".to_owned(),
        };
    };
    if executable.contains('/') || executable.contains('$') || executable.contains('=') {
        return BashReadonlyClass::Unproven {
            reason: "dynamic or path-qualified executables are not proven read-only".to_owned(),
        };
    }

    let args: Vec<&str> = tokens.collect();
    match executable {
        "ls" | "pwd" | "echo" | "printf" | "true" | "false" | "test" | "[" | "cat" | "head"
        | "tail" | "wc" | "stat" | "file" | "which" | "type" | "basename" | "dirname"
        | "realpath" | "readlink" | "uname" | "id" | "whoami" | "printenv" | "grep" | "rg"
        | "fd" | "jq" | "sleep" => BashReadonlyClass::Readonly,
        "date" => {
            // Conservative: only bare `date` is proven display-only. GNU glued
            // `-s…`, quoted `-s`, BSD positional set-time, and display flags
            // that take operands need full argv analysis before they can be
            // trusted as read-only.
            if args.is_empty() {
                BashReadonlyClass::Readonly
            } else {
                BashReadonlyClass::Unproven {
                    reason: "date with arguments is not proven read-only".to_owned(),
                }
            }
        }
        "env" => {
            // Bare `env` prints the environment. Any wrapper (`env rm`,
            // `env FOO=1 cmd`) needs full argv walking before it can be proven.
            if args.is_empty() {
                BashReadonlyClass::Readonly
            } else {
                BashReadonlyClass::Unproven {
                    reason: "env wrappers require full argv analysis".to_owned(),
                }
            }
        }
        "find" => {
            if args.iter().copied().any(is_find_write_option) {
                BashReadonlyClass::Unproven {
                    reason: "find write/action options are not read-only".to_owned(),
                }
            } else {
                BashReadonlyClass::Readonly
            }
        }
        "yq" => {
            if args
                .iter()
                .any(|token| *token == "-i" || *token == "--inplace")
            {
                BashReadonlyClass::Unproven {
                    reason: "yq in-place edits are not read-only".to_owned(),
                }
            } else {
                BashReadonlyClass::Readonly
            }
        }
        "git" => classify_git(args.as_slice()),
        _ => BashReadonlyClass::Unproven {
            reason: format!(
                "executable `{executable}` is not on the conservative read-only allowlist"
            ),
        },
    }
}

fn is_find_write_option(token: &str) -> bool {
    matches!(
        token,
        "-delete"
            | "-exec"
            | "-execdir"
            | "-ok"
            | "-okdir"
            | "-fprint"
            | "-fprint0"
            | "-fls"
            | "-fprintf"
    ) || token.starts_with("-exec")
}

fn classify_git(args: &[&str]) -> BashReadonlyClass {
    let mut index = 0usize;
    while index < args.len() {
        let arg = args[index];
        if arg == "-C" {
            index += 2;
            continue;
        }
        // Reject spaced `-c`, glued `-ckey=value`, and bang-alias config that
        // can rewrite a later allowlisted subcommand into shell execution.
        if arg.starts_with("-c") || arg.starts_with("--exec-path") || arg == "--paginate" {
            return BashReadonlyClass::Unproven {
                reason:
                    "git global options that can change execution context are not proven read-only"
                        .to_owned(),
            };
        }
        if arg.starts_with('-') {
            index += 1;
            continue;
        }
        let rest = &args[index + 1..];
        return match arg {
            "status" | "log" | "show" | "diff" | "ls-files" | "ls-tree" | "rev-parse"
            | "describe" | "blame" | "shortlog" | "version" | "help" => {
                // Diff-family options such as `--output=` write a file without
                // shell redirects; keep those unproven under the conservative
                // checker.
                if git_writes_output(rest) {
                    BashReadonlyClass::Unproven {
                        reason: format!(
                            "git {arg} forms that write output files are not proven read-only"
                        ),
                    }
                } else {
                    BashReadonlyClass::Readonly
                }
            }
            "config" => {
                if rest
                    .iter()
                    .any(|a| matches!(*a, "--list" | "-l" | "--get" | "--get-all" | "--get-regexp"))
                {
                    BashReadonlyClass::Readonly
                } else {
                    BashReadonlyClass::Unproven {
                        reason: "git config without an explicit read form is not proven read-only"
                            .to_owned(),
                    }
                }
            }
            "branch" | "tag" | "remote" => {
                if git_ref_query_only(arg, rest) {
                    BashReadonlyClass::Readonly
                } else {
                    BashReadonlyClass::Unproven {
                        reason: format!(
                            "git {arg} forms that may create or mutate refs are not proven read-only"
                        ),
                    }
                }
            }
            _ => BashReadonlyClass::Unproven {
                reason: format!("git subcommand `{arg}` is not proven read-only"),
            },
        };
    }
    BashReadonlyClass::Unproven {
        reason: "git command missing a subcommand is not proven read-only".to_owned(),
    }
}

fn git_writes_output(rest: &[&str]) -> bool {
    rest.iter()
        .any(|arg| *arg == "--output" || arg.starts_with("--output="))
}

/// True only for listing/query forms with no positional ref name that could
/// create a branch, tag, or remote.
fn git_ref_query_only(subcommand: &str, rest: &[&str]) -> bool {
    if rest.iter().any(|a| {
        matches!(
            *a,
            "-d" | "-D"
                | "--delete"
                | "add"
                | "rename"
                | "set-url"
                | "-m"
                | "--move"
                | "-c"
                | "--copy"
                | "--edit-description"
                | "-f"
                | "--force"
                | "-u"
                | "--set-upstream-to"
                | "--unset-upstream"
                | "-i"
                | "--stdin"
        ) || a.starts_with("--set-upstream-to=")
    }) {
        return false;
    }
    // Any positional operand can name a new ref (`git branch foo`,
    // `git tag v1`). After `--`, even dash-prefixed tokens are operands
    // (`git branch -- -foo`), so they are never treated as flags.
    let _ = subcommand;
    let mut end_of_options = false;
    for arg in rest {
        if end_of_options {
            return false;
        }
        if *arg == "--" {
            end_of_options = true;
            continue;
        }
        if !arg.starts_with('-') {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_simple_readonly_commands() {
        for command in [
            "ls -la",
            "pwd",
            "git status",
            "git log --oneline",
            "cat README.md",
            "git branch",
            "git branch -a",
            "env",
            "date",
            "yq .x file.yaml",
        ] {
            let analysis = analyze_bash_readonly(command);
            assert!(
                matches!(analysis.class, BashReadonlyClass::Readonly),
                "{command} should be readonly, got {:?}",
                analysis.class
            );
        }
    }

    #[test]
    fn rejects_writes_and_unknowns() {
        for command in [
            "rm -rf /",
            "echo hi > file",
            "ls | rm",
            "curl http://x",
            "git commit -am x",
            "env rm -rf /",
            "git branch feature",
            "git tag v1.0.0",
            "git branch -- -foo",
            "git tag -- -v1",
            "yq -i '.x=1' file.yaml",
            "date -s 12:00:00",
            "date -s12:00:00",
            "date \"-s\" 12:00:00",
            "date 1234567890",
            "date -u",
            "pwd\nrm -rf /",
            "pwd\r\nrm -rf /",
            "pwd & rm -rf /",
            "true && rm -rf /",
            "git diff --no-index --output=/tmp/written.patch a b",
            "git diff --output /tmp/written.patch",
            "git show --output=patch HEAD",
            "git diff --no-index '--output' /tmp/result.patch /tmp/input-a /tmp/input-b",
            "git diff \"--output=/tmp/written.patch\" a b",
            "git diff --output\\=/tmp/written.patch",
            "git -calias.status=!touch /tmp/pwned status",
            "find . '-delete'",
            "yq '-i' .x=1 file.yaml",
            "yq '.x' file.yaml",
        ] {
            let analysis = analyze_bash_readonly(command);
            assert!(
                matches!(analysis.class, BashReadonlyClass::Unproven { .. }),
                "{command} should be unproven"
            );
        }
    }

    #[test]
    fn find_delete_is_unproven() {
        let analysis = analyze_bash_readonly("find . -delete");
        assert!(matches!(analysis.class, BashReadonlyClass::Unproven { .. }));
    }
}
