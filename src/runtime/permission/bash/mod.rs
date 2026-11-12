//! ZCode-derived Bash read-only analysis (Issue #544 P06).
//!
//! Upstream pin: `zai-org/ZCode@b95ebaffeab641907b38652f45c1502bd762972d`
//! (Apache-2.0). Parser: `brush-parser` 0.4.x (MIT), used instead of `ZCode`'s
//! TypeScript `unbash`.
//!
//! Parse failures, unsupported constructs, dynamic executable names, and
//! unresolved behavior never count as proven read-only. Unproven is distinct
//! from known-malicious: Plan elevates and Automatic reviews.

// Dense ZCode table/callback ports trip pedantic Clippy; keep the port
// structure close to upstream rather than reshaping for lint style.
#![allow(
    clippy::assigning_clones,
    clippy::collapsible_if,
    clippy::doc_markdown,
    clippy::manual_let_else,
    clippy::map_unwrap_or,
    clippy::redundant_closure_for_method_calls,
    clippy::similar_names,
    clippy::single_match_else,
    clippy::struct_excessive_bools,
    clippy::too_many_lines,
    clippy::wildcard_imports
)]

macro_rules! zcode_port_mod {
    ($($name:ident),+ $(,)?) => {
        $(
            #[allow(
                clippy::assigning_clones,
                clippy::collapsible_if,
                clippy::doc_markdown,
                clippy::manual_let_else,
                clippy::map_unwrap_or,
                clippy::redundant_closure_for_method_calls,
                clippy::similar_names,
                clippy::single_match_else,
                clippy::struct_excessive_bools,
                clippy::too_many_lines,
                clippy::wildcard_imports
            )]
            mod $name;
        )+
    };
}

zcode_port_mod!(
    callbacks, commands, direct, flags, flags_walk, git, io, parse, policy, types
);

use parse::analyze_bash_command;
use policy::{
    analysis_contains_git_and_directory_change, evaluate_bash_readonly_policy,
    has_known_bash_write_option,
};
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
    /// Number of simple commands collected from the AST.
    pub command_count: usize,
    /// True when the AST walk observed redirects.
    pub has_redirects: bool,
    /// True when dynamic expansions were observed.
    pub has_dynamic_words: bool,
    /// Unsupported construct names when the walk fail-closed.
    pub unsupported_node_types: Vec<String>,
}

/// Analyzes whether a Bash command is proven read-only.
#[must_use]
pub fn analyze_bash_readonly(command: &str) -> BashReadonlyAnalysis {
    let command = command.trim().to_owned();
    if command.is_empty() {
        return BashReadonlyAnalysis {
            class: BashReadonlyClass::Unproven {
                reason: "empty Bash command is not proven read-only".to_owned(),
            },
            command,
            command_count: 0,
            has_redirects: false,
            has_dynamic_words: false,
            unsupported_node_types: Vec::new(),
        };
    }

    let ast = analyze_bash_command(&command);
    let facts = |class: BashReadonlyClass| BashReadonlyAnalysis {
        class,
        command: command.clone(),
        command_count: ast.commands.len(),
        has_redirects: ast.has_redirects,
        has_dynamic_words: ast.has_dynamic_words,
        unsupported_node_types: ast.unsupported_node_types.clone(),
    };

    if ast.has_parse_errors {
        return facts(BashReadonlyClass::Unproven {
            reason: "Bash parse failure is not proven read-only".to_owned(),
        });
    }
    if !ast.is_permission_safe() {
        let detail = if ast.has_dynamic_words {
            "dynamic shell expansions are not proven read-only"
        } else if !ast.unsupported_node_types.is_empty() {
            "unsupported shell constructs are not proven read-only"
        } else {
            "Bash command is not permission-safe"
        };
        return facts(BashReadonlyClass::Unproven {
            reason: detail.to_owned(),
        });
    }
    if ast.commands.is_empty() {
        return facts(BashReadonlyClass::Unproven {
            reason: "no simple commands to classify".to_owned(),
        });
    }
    if analysis_contains_git_and_directory_change(&ast.commands) {
        return facts(BashReadonlyClass::Unproven {
            reason: "git combined with directory change is not proven read-only".to_owned(),
        });
    }

    let mut has_readonly = false;
    for part in &ast.commands {
        if has_known_bash_write_option(part) {
            return facts(BashReadonlyClass::Unproven {
                reason: format!("command `{}` has a known write/exec option", part.name),
            });
        }
        match evaluate_bash_readonly_policy(part) {
            Some(true) => has_readonly = true,
            Some(false) => {
                return facts(BashReadonlyClass::Unproven {
                    reason: format!(
                        "command `{}` is not on the read-only policy",
                        if part.name.is_empty() {
                            "<empty>"
                        } else {
                            part.name.as_str()
                        }
                    ),
                });
            }
            None => {
                return facts(BashReadonlyClass::Unproven {
                    reason: format!(
                        "command `{}` has unresolved read-only policy",
                        if part.name.is_empty() {
                            "<empty>"
                        } else {
                            part.name.as_str()
                        }
                    ),
                });
            }
        }
    }

    if has_readonly {
        facts(BashReadonlyClass::Readonly)
    } else {
        facts(BashReadonlyClass::Unproven {
            reason: "no proven read-only command in the pipeline".to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_readonly(command: &str) {
        let analysis = analyze_bash_readonly(command);
        assert!(
            matches!(analysis.class, BashReadonlyClass::Readonly),
            "{command} should be readonly, got {:?}",
            analysis.class
        );
    }

    fn assert_unproven(command: &str) {
        let analysis = analyze_bash_readonly(command);
        assert!(
            matches!(analysis.class, BashReadonlyClass::Unproven { .. }),
            "{command} should be unproven, got {:?}",
            analysis.class
        );
    }

    #[test]
    fn allows_simple_readonly_commands() {
        for command in [
            "ls -la",
            "pwd",
            "cat README.md",
            "env",
            "date",
            "grep pattern",
            "hostname",
            "hostname -f",
            "hostname --fqdn",
            "git status",
            "git log --oneline",
            "git log -n 5 --graph --stat",
            "git show --stat HEAD",
            "git branch -a",
            "rg -n foo src",
            "find . -name '*.rs'",
            "head -n 20 README.md",
            "wc -l README.md",
            "stat README.md",
            "file README.md",
            "echo hello",
            "printf '%s\\n' hello",
            "true",
            "false",
            "test -f package.json",
            "tree -a",
            "tree -L 2",
            "command grep -n foo src",
            "LC_ALL=C grep -n foo src",
            "cat < README.md",
            "grep -n foo src 2>/dev/null",
            "grep -n foo src 2>&1",
            "ls | cat",
            "pwd && whoami",
            "sed -n '1,20p' README.md",
            "sed 's/foo/bar/g'",
            "uniq",
            "uniq input.txt",
            "uniq -c input.txt",
            "uniq -f 1 -s 2 input.txt",
        ] {
            assert_readonly(command);
        }
    }

    #[test]
    fn rejects_writes_dynamics_and_unknowns() {
        for command in [
            "rm -rf /",
            "echo hi > file",
            "ls | rm",
            "curl http://x",
            "git commit -am x",
            "env rm -rf /",
            "git branch feature",
            "git tag v1.0.0",
            "find . -delete",
            "find . -exec rm {} +",
            "find . '-delete'",
            "date -s 12:00:00",
            "date -s12:00:00",
            r#"date "-s" 12:00:00"#,
            "date 1234567890",
            "pwd\nrm -rf /",
            "pwd\r\nrm -rf /",
            "pwd & rm -rf /",
            "true && rm -rf /",
            "git diff --output=/tmp/written.patch",
            "git -calias.status=!touch /tmp/pwned status",
            "git -c alias.status=!touch /tmp/pwned status",
            "hostname new-host",
            "hostname -- evil",
            "hostname -",
            r#"hostname """#,
            "fd -x rm",
            "fd --exec rm {}",
            "fd --exec-batch rm",
            "fd -X rm",
            "rg --pre=sh needle",
            "rg --pre ./filter foo src",
            "jq -f filter.jq package.json",
            "jq 'env' package.json",
            "tree -o out.txt",
            "tree --bogus",
            "xargs rm -rf",
            "sed -i 's/a/b/' file",
            "sed -n -e 'e printf sed-probe' README.md",
            "uniq input.txt output.txt",
            "uniq -- input.txt output.txt",
            "cat README.md > out.txt",
            "ls $(touch out.txt)",
            "ls `touch out.txt`",
            "FOO=bar grep foo src",
            "cat /dev/tcp/127.0.0.1/80",
            "head /dev/udp/127.0.0.1/53",
            "cat < /dev/tcp/example.com/80",
            "ls file?",
            "cat *.txt",
            "/usr/bin/sed -n '1p' README.md",
            "{ ls; }",
            "(ls)",
            "[[ -f x ]]",
            "cd /tmp && git status",
            "touch approval-probe.flag",
        ] {
            assert_unproven(command);
        }
    }

    #[test]
    fn preserves_redirect_and_dev_tcp_hardenings() {
        assert_unproven("echo hi > file");
        assert_unproven("cat < /dev/tcp/127.0.0.1/80");
        assert_unproven("cat /dev/tcp/127.0.0.1/80");
        assert_unproven("head /dev/udp/127.0.0.1/53");
        assert_readonly("cat < README.md");
    }

    #[test]
    fn preserves_glued_flag_hardenings() {
        assert_unproven("date -s12:00:00");
        assert_unproven("git -calias.status=!touch /tmp/pwned status");
        assert_unproven("fd -xrm");
        assert_unproven("fd -Xrm");
        assert_unproven("fd --execrm");
        assert_unproven("fd --exec-batch=rm");
    }

    #[test]
    fn extends_structured_facts() {
        let analysis = analyze_bash_readonly("ls | cat");
        assert!(matches!(analysis.class, BashReadonlyClass::Readonly));
        assert_eq!(analysis.command_count, 2);
        assert!(!analysis.has_dynamic_words);
    }
}
