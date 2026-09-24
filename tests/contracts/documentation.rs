//! Bounded current-product documentation guard; historical Rust comments are out of scope.
use std::path::{Path, PathBuf};

const OBSOLETE: &[&str] = &[
    "settings.toml",
    "models.toml",
    "schemas/settings.schema.json",
    "schemas/models.schema.json",
    "--user-settings",
    "--models",
    "--skill",
    "--no-direct-tools",
    "--no-builtin-tools",
    "--trust",
    "--untrusted",
    "skills.sources",
    "[skills].sources",
    "disabled_skills",
    "no_direct_tools",
    "no_builtin_tools",
    "no_automatic_skills",
    "skill_paths",
    "exclude_tools",
    "~/.agents",
    "global skill",
    "xdg_config_home",
    ".config/rustx",
    "trustaction",
    "trustepoch",
    "trusted workspace",
    "untrusted workspace",
    "resources/reload",
    "configuration/reload",
    "pending_reload",
    "settings/selectmodel",
    "settings/replace",
    "resource reload",
    "source enablement",
    "source activation",
    "source disabling",
    "enabled source",
    "rustx.app-server.v5",
    "protocol/app-server/v5",
    "settings/sourcesread",
    "settings/sourceswrite",
    "settings/setapprovalmode",
    "agent.extensions",
    "`/approval`",
    "`/permission`",
];

fn obsolete_contract(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    OBSOLETE.iter().any(|term| lower.contains(term))
        || (lower.contains("enabled by default")
            && ["todo", "goal", "agent status"]
                .iter()
                .any(|plugin| lower.contains(plugin)))
}

fn files(directory: &Path, output: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files(&path, output);
        } else if matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("md" | "toml" | "yaml" | "json")
        ) {
            output.push(path);
        }
    }
}

#[test]
fn current_documentation_rejects_obsolete_cfg2_contracts() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut paths = Vec::new();
    for directory in ["docs", "examples"] {
        files(&root.join(directory), &mut paths);
    }
    for directory in [root.to_owned(), root.join("web-console"), root.join("tui")] {
        paths.extend(
            std::fs::read_dir(directory)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| path.extension().is_some_and(|extension| extension == "md")),
        );
    }
    // Exact exceptions: labeled historical records and the negative-term audit itself.
    let exceptions = [
        "web-console/VALIDATION.md",
        "docs/tui-dogfooding-267.md",
        "docs/cfg3-residue-audit.md",
        "docs/issue-380-audit.md",
    ];
    let mut failures = Vec::new();
    paths.sort();
    for path in paths {
        let relative = path.strip_prefix(root).unwrap();
        if exceptions
            .iter()
            .any(|exception| relative == Path::new(exception))
        {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for (index, line) in text.lines().enumerate() {
            if obsolete_contract(line) {
                failures.push(format!("{}:{}: {line}", relative.display(), index + 1));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "obsolete current-product contracts:\n{}",
        failures.join("\n")
    );
}

#[test]
fn documentation_guard_distinguishes_current_paths_and_negative_spellings() {
    for obsolete in [
        "settings.toml",
        "models.toml",
        "[skills].sources",
        "--skill",
        "--no-direct-tools",
        "--user-settings",
        "~/.agents/skills",
        "rustx.app-server.v5",
        "Todo Plugin, enabled by default",
    ] {
        assert!(obsolete_contract(obsolete), "{obsolete}");
    }
    for current in [
        "~/rustx/.agents/skills",
        "<workspace>/.agents/skills",
        "rustx.toml",
        "--config",
        "configuration/reconcile",
        "Plugins default off",
    ] {
        assert!(!obsolete_contract(current), "{current}");
    }
}

#[test]
fn current_app_server_documentation_matches_negotiated_version() {
    use rustx::app_server::protocol::APP_SERVER_PROTOCOL_VERSION;

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let doc = std::fs::read_to_string(root.join("docs/app-server-protocol.md")).unwrap();
    let version = APP_SERVER_PROTOCOL_VERSION;
    assert!(doc.starts_with(&format!("# App Server protocol v{version}\n")));
    let section = |heading: &str| {
        doc.split_once(heading)
            .unwrap()
            .1
            .split("\n##")
            .next()
            .unwrap()
    };
    let lifecycle = section("## Current Session lifecycle contract");
    assert!(lifecycle.contains(&format!("Initialization requires exactly v{version} ")));
    assert!(lifecycle.contains(&format!("WebSocket requires `rustx.app-server.v{version}`")));
    assert!(lifecycle.contains(&format!(
        "v{} and all earlier versions are rejected",
        version - 1
    )));
    for current in [section("### Reproducible generation"), lifecycle] {
        for suffix in ["ts", "schema.json"] {
            assert!(current.contains(&format!("`v{version}.{suffix}`")));
            for older in 1..version {
                assert!(!current.contains(&format!("`v{older}.{suffix}`")));
                assert!(
                    !root
                        .join(format!("protocol/app-server/v{older}.{suffix}"))
                        .exists()
                );
            }
            assert!(
                root.join(format!("protocol/app-server/v{version}.{suffix}"))
                    .exists()
            );
        }
    }
}
