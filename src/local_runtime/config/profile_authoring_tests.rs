use super::tests::MINIMAL;
use super::*;
#[test]
fn root_and_named_share_profile_fields_but_only_root_delegates() {
    let common = "description = 'review'\nskills = []\nworkflows = ['check']\n[tools]\nbuiltin = ['read']\n[model]\nmodel = 'provider/model'\n";
    let root = format!(
        "[agent]\nagents = ['helper']\n{}",
        common
            .replace("[tools]", "[agent.tools]")
            .replace("[model]", "[agent.model]")
    );
    let root = CurrentRuntimeConfig::from_toml_slice(root.as_bytes()).unwrap();
    let named = crate::local_runtime::agent_resources::parse(common).unwrap();
    assert_eq!(root.agent.lower().unwrap(), named.lower().unwrap());
    assert_eq!(root.agent.agents[0].as_str(), "helper");
}
#[test]
fn cfg332_root_and_named_skill_visibility_share_all_exact_none_semantics() {
    use crate::runtime::agent_profile::AgentSkillSelection;
    for (authored, expected) in [
        ("skills = 'all'", AgentSkillSelection::All),
        (
            "skills = ['guide']",
            AgentSkillSelection::Exact(vec!["guide".into()]),
        ),
        ("skills = []", AgentSkillSelection::Exact(vec![])),
    ] {
        let root = CurrentRuntimeConfig::from_toml_slice(
            MINIMAL
                .replace("[agent]", &format!("[agent]\n{authored}"))
                .as_bytes(),
        )
        .unwrap();
        let named = crate::local_runtime::agent_resources::parse(&format!(
            "description = 'r'\ninstructions = 'i'\n{authored}"
        ))
        .unwrap();
        assert_eq!(root.agent.skills, Some(expected.clone()));
        assert_eq!(named.skills, Some(expected));
    }
    let named = crate::local_runtime::agent_resources::parse(
        "description = 'r'\nskills = ['UPPERCASE and spaces']",
    )
    .unwrap();
    assert_eq!(named.skills.unwrap().names(), &["UPPERCASE and spaces"]);
    let omitted = CurrentRuntimeConfig::from_toml_slice(MINIMAL.as_bytes()).unwrap();
    assert_eq!(omitted.agent.skills, None);
    let named = crate::local_runtime::agent_resources::parse("description = 'r'").unwrap();
    assert_eq!(named.skills, None);
    for authored in ["disabled_skills = []", "skills = 'none'"] {
        assert!(
            CurrentRuntimeConfig::from_toml_slice(
                MINIMAL
                    .replace("[agent]", &format!("[agent]\n{authored}"))
                    .as_bytes(),
            )
            .is_err(),
            "{authored}"
        );
        assert!(
            crate::local_runtime::agent_resources::parse(&format!(
                "description = 'r'\n{authored}"
            ),)
            .is_err(),
            "{authored}"
        );
    }
}

#[test]
fn cfg332_skill_roots_are_process_bindings_not_authored_source_policy() {
    for sources in ["[]", "['workspace']", "['user', 'workspace']"] {
        assert!(
            CurrentRuntimeConfig::from_toml_slice(
                format!("{MINIMAL}\n[skills]\nsources = {sources}").as_bytes(),
            )
            .is_err()
        );
    }
}

#[test]
fn cfg273_closed_profile_authoring_rejects_unknown_and_duplicate_tools() {
    for text in [
        "unknown = true",
        "[extensions.unknown]",
        "[tools]\nbuiltin = ['read', 'read']",
        "agents = ['INVALID']",
        "[tools]\nbuiltin = false",
    ] {
        assert!(
            crate::local_runtime::agent_resources::parse(text).is_err(),
            "{text}"
        );
    }
}
