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
    if command.contains('*') || command.contains('?') {
        // Glob expansion rewrites argv before the executable runs, so a
        // source-level token check cannot prove the post-expansion argv.
        return BashReadonlyClass::Unproven {
            reason: "shell glob patterns require full argv analysis".to_owned(),
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
    classify_allowlisted_executable(executable, args.as_slice())
}

fn classify_allowlisted_executable(executable: &str, args: &[&str]) -> BashReadonlyClass {
    // Keep this surface tiny until brush-parser AST argv analysis lands.
    // Tools with rich option grammars (`git`, `find`, `fd`, `jq`, `yq`, `rg`)
    // stay unproven: whitespace tokenization cannot safely prove argv.
    match executable {
        "ls" | "pwd" | "echo" | "printf" | "true" | "false" | "test" | "[" | "cat" | "head"
        | "tail" | "wc" | "stat" | "file" | "which" | "type" | "basename" | "dirname"
        | "realpath" | "readlink" | "uname" | "id" | "whoami" | "printenv" | "grep" | "sleep" => {
            BashReadonlyClass::Readonly
        }
        "date" => {
            // Conservative: only bare `date` is proven display-only.
            if args.is_empty() {
                BashReadonlyClass::Readonly
            } else {
                BashReadonlyClass::Unproven {
                    reason: "date with arguments is not proven read-only".to_owned(),
                }
            }
        }
        "env" => {
            // Bare `env` prints the environment; wrappers need argv walking.
            if args.is_empty() {
                BashReadonlyClass::Readonly
            } else {
                BashReadonlyClass::Unproven {
                    reason: "env wrappers require full argv analysis".to_owned(),
                }
            }
        }
        _ => BashReadonlyClass::Unproven {
            reason: format!(
                "executable `{executable}` is not on the conservative read-only allowlist"
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_simple_readonly_commands() {
        for command in [
            "ls -la",
            "pwd",
            "cat README.md",
            "env",
            "date",
            "grep pattern",
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
            "git status",
            "git status -v",
            "git commit -am x",
            "env rm -rf /",
            "find .",
            "find . -delete",
            "find . -fprin? result.txt",
            "yq .x file.yaml",
            "jq . file.json",
            "rg .",
            "rg --pre=sh needle",
            "fd .",
            "fd . -X touch",
            "date -s 12:00:00",
            "date -u",
            "pwd\nrm -rf /",
            "pwd & rm -rf /",
            "true && rm -rf /",
            "date \"-u\"",
            "ls file?",
            "cat *.txt",
        ] {
            let analysis = analyze_bash_readonly(command);
            assert!(
                matches!(analysis.class, BashReadonlyClass::Unproven { .. }),
                "{command} should be unproven"
            );
        }
    }
}
