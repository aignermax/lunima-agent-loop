use super::*;
use crate::config::RepoConfig;

fn cfg(with_key: bool) -> (tempfile::TempDir, TeamConfig) {
    let tmp = tempfile::tempdir().unwrap();
    let mut c = TeamConfig::default();
    c.models.coder = "coder-model".into();
    c.models.premium = "premium-model".into();
    c.repos = vec![
        RepoConfig { name: "aignermax/Lunima".into(), coder: true, routing: Routing::KimiOnly },
        RepoConfig { name: "o/kimi".into(), coder: true, routing: Routing::Kimi },
        RepoConfig { name: "Akhetonics/khepri".into(), coder: true, routing: Routing::Claude },
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
fn codex_overrides_every_routing_and_label_even_without_provider_keys() {
    for with_key in [false, true] {
        let (_t, c) = cfg(with_key);
        let mut doc = toml::Value::try_from(c).unwrap();
        doc.as_table_mut().unwrap().insert("runner".into(), toml::from_str::<toml::Value>("backend = 'codex'\nmodel = 'gpt-test'").unwrap());
        let c: TeamConfig = doc.try_into().unwrap();
        for repo in ["aignermax/Lunima", "o/kimi", "Akhetonics/khepri", "unknown/repo"] {
            for tags in [vec![], labels(&["claudeapi"]), labels(&["eco", "complex", "critical"])] {
                let choice = choose(&c, repo, &tags).unwrap();
                assert_eq!(choice.model, "gpt-test", "{repo} {tags:?}");
                assert!(choice.env.set.is_empty());
                assert!(choice.why.contains("Codex"));
            }
        }
    }
}

#[test]
fn global_api_choices_override_labels_and_refuse_missing_keys() {
    let (_t, mut c) = cfg(true);
    for (backend, expected) in [(Backend::Openrouter, "moonshotai/kimi-k3"), (Backend::Eco, "kimi-k2-thinking")] {
        c.runner.backend = backend;
        let choice = choose(&c, "Akhetonics/khepri", &labels(&["claudeapi", "complex", "eco"])).unwrap();
        assert_eq!(choice.model, expected);
        assert!(choice.env.set.contains_key("ANTHROPIC_AUTH_TOKEN"));
    }
    let (_t, mut c) = cfg(false);
    c.providers.eco.key_env.clear();
    for backend in [Backend::Openrouter, Backend::Eco, Backend::OpenAi] {
        c.runner.backend = backend;
        c.providers.openai.key_env.clear();
        assert!(choose(&c, "unknown/repo", &labels(&["claudeapi"])).is_err());
    }
}

#[test]
fn openai_uses_its_endpoint_model_and_dedicated_api_transport() {
    let (t, mut c) = cfg(false);
    let key = t.path().join("openai-key");
    std::fs::write(&key, "test-secret").unwrap();
    c.runner.backend = Backend::OpenAi;
    c.providers.openai.key_file = key.display().to_string();
    for model in ["", "   "] {
        c.providers.openai.model = model.into();
        assert!(runner_choice(&c.runner, &c.providers).unwrap_err().contains("Modell fehlt"));
    }
    c.providers.openai.model = "api-model".into();
    let choice = choose(&c, "aignermax/Lunima", &labels(&["claudeapi"])).unwrap();
    assert_eq!(choice.model, "api-model");
    assert_eq!(choice.env.set["LUNIMA_CODEX_API_KEY"], "test-secret");
    assert_eq!(choice.env.set["LUNIMA_CODEX_API_BASE_URL"], "https://api.openai.com/v1");
}
