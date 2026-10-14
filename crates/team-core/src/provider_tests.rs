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
