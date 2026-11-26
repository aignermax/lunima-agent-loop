use super::*;
use crate::config::RepoConfig;

fn cfg(with_key: bool) -> (tempfile::TempDir, TeamConfig) {
    let tmp = tempfile::tempdir().unwrap();
    let mut c = TeamConfig::default();
    c.models.coder = "coder-model".into();
    c.models.premium = "premium-model".into();
    c.repos = vec![
        RepoConfig { name: "aignermax/Lunima".into(), coder: true, routing: Routing::KimiOnly, ..Default::default() },
        RepoConfig { name: "o/kimi".into(), coder: true, routing: Routing::Kimi, ..Default::default() },
        RepoConfig { name: "Akhetonics/khepri".into(), coder: true, routing: Routing::Claude, ..Default::default() },
    ];
    if with_key {
        let key = tmp.path().join("or.txt");
        std::fs::write(&key, "sk-or").unwrap();
        c.providers.openrouter.key_file = key.display().to_string();
        let eco = tmp.path().join("eco.txt");
        std::fs::write(&eco, "sk-eco").unwrap();
        c.providers.eco.key_file = eco.display().to_string();
        c.providers.eco.key_env = String::new();
    }
    (tmp, c)
}

fn labels(l: &[&str]) -> Vec<String> {
    l.iter().map(|s| s.to_string()).collect()
}

#[test]
fn matrix_matches_the_team_policy() {
    let (_t, c) = cfg(true);
    let m = |repo: &str, l: &[&str]| choose(&c, repo, &labels(l)).unwrap().model;
    assert_eq!(m("aignermax/Lunima", &[]), "moonshotai/kimi-k3");
    assert_eq!(m("aignermax/Lunima", &["complex", "eco"]), "moonshotai/kimi-k3");
    assert_eq!(m("aignermax/Lunima", &["claudeapi"]), "premium-model");
    assert_eq!(m("Akhetonics/khepri", &[]), "coder-model");
    assert_eq!(m("Akhetonics/khepri", &["eco"]), "coder-model", "internal repo ignores eco");
    assert_eq!(m("Akhetonics/khepri", &["eco", "complex"]), "premium-model");
    assert_eq!(m("o/kimi", &["eco"]), "kimi-k2-thinking");
    assert_eq!(m("o/kimi", &[]), "moonshotai/kimi-k3");
    assert_eq!(m("unknown/repo", &[]), "coder-model");
}

#[test]
fn claude_runs_on_the_subscription_and_kimi_on_its_key() {
    let (_t, c) = cfg(true);
    let claude = choose(&c, "Akhetonics/khepri", &[]).unwrap();
    assert!(claude.env.set.is_empty());
    assert!(claude.env.remove.contains(&"ANTHROPIC_API_KEY".to_string()));
    let kimi = choose(&c, "aignermax/Lunima", &[]).unwrap();
    assert_eq!(kimi.env.set["ANTHROPIC_AUTH_TOKEN"], "sk-or");
    assert_eq!(kimi.env.set["ANTHROPIC_BASE_URL"], "https://openrouter.ai/api");
}

#[test]
fn kimi_only_without_key_refuses_instead_of_using_claude() {
    let (_t, c) = cfg(false);
    assert!(choose(&c, "aignermax/Lunima", &[]).is_err());
    assert_eq!(choose(&c, "o/kimi", &[]).unwrap().model, "coder-model", "kimi (not only) falls back");
}

#[test]
fn global_codex_does_not_override_project_routing() {
    for with_key in [false, true] {
        let (_t, c) = cfg(with_key);
        let mut doc = toml::Value::try_from(c).unwrap();
        doc.as_table_mut().unwrap().insert("runner".into(), toml::from_str::<toml::Value>("backend = 'codex'\nmodel = 'gpt-test'").unwrap());
        let c: TeamConfig = doc.try_into().unwrap();
        for repo in ["Akhetonics/khepri", "unknown/repo"] {
            for tags in [vec![], labels(&["claudeapi"]), labels(&["eco", "complex", "critical"])] {
                let choice = choose(&c, repo, &tags).unwrap();
                if repo == "Akhetonics/khepri" || repo == "unknown/repo" {
                    assert!(!choice.model.starts_with("gpt"), "{repo} {tags:?}");
                    assert!(choice.env.set.is_empty());
                }
            }
        }
    }
}

#[test]
fn global_api_choices_cannot_send_company_projects_to_third_parties() {
    let (_t, mut c) = cfg(true);
    for backend in [Backend::Openrouter, Backend::Eco, Backend::OpenAi] {
        c.runner.backend = backend;
        let choice = choose(&c, "Akhetonics/khepri", &labels(&["claudeapi", "complex", "eco"])).unwrap();
        assert_eq!(choice.model, "premium-model");
        assert!(choice.env.set.is_empty());
    }
    let (_t, mut c) = cfg(false);
    c.providers.eco.key_env.clear();
    for backend in [Backend::Openrouter, Backend::Eco, Backend::OpenAi] {
        c.runner.backend = backend;
        c.providers.openai.key_env.clear();
        assert_eq!(choose(&c, "unknown/repo", &labels(&["claudeapi"])).unwrap().model, "premium-model");
    }
}

#[test]
fn openai_uses_its_endpoint_model_and_dedicated_api_transport() {
    let (t, mut c) = cfg(false);
    let key = t.path().join("openai-key");
    std::fs::write(&key, "test-secret").unwrap();
    c.repos[0].routing = Routing::OpenAi;
    c.providers.openai.key_file = key.display().to_string();
    for model in ["", "   "] {
        c.providers.openai.model = model.into();
        assert!(choose(&c, "aignermax/Lunima", &[]).unwrap_err().contains("Modell fehlt"));
    }
    c.providers.openai.model = "api-model".into();
    let choice = choose(&c, "aignermax/Lunima", &labels(&["claudeapi"])).unwrap();
    assert_eq!(choice.model, "api-model");
    assert_eq!(choice.env.set["LUNIMA_CODEX_API_KEY"], "test-secret");
    assert_eq!(choice.env.set["LUNIMA_CODEX_API_BASE_URL"], "https://api.openai.com/v1");
}

#[test]
fn codex_is_explicit_per_project_and_labels_cannot_escape_it() {
    let (_t, mut c) = cfg(true);
    c.repos[2].routing = Routing::Codex;
    c.runner.model = "project-codex".into();
    for global in [Backend::Claude, Backend::Openrouter, Backend::Eco, Backend::OpenAi] {
        c.runner.backend = global;
        for tags in [vec![], labels(&["claudeapi", "complex", "critical", "eco"])] {
            let choice = choose(&c, "AKHETONICS/KHEPRI", &tags).unwrap();
            assert_eq!(repo_runner(&c, "Akhetonics/khepri").backend, Backend::Codex);
            assert_eq!(choice.model, "project-codex");
            assert!(choice.env.set.is_empty());
            assert_eq!(choose(&c, "aignermax/Lunima", &[]).unwrap().model, "moonshotai/kimi-k3");
        }
    }
}

#[test]
fn mixed_project_chat_refuses_third_party_global_settings() {
    let (_t, mut c) = cfg(true);
    for backend in [Backend::Openrouter, Backend::Eco, Backend::OpenAi] {
        c.runner.backend = backend;
        assert!(chat_runner(&c).unwrap_err().contains("projektübergreifenden"));
    }
    for backend in [Backend::Claude, Backend::Codex] {
        c.runner.backend = backend;
        assert_eq!(chat_runner(&c).unwrap().backend, backend);
    }
}
