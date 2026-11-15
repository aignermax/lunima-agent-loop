use super::*;

#[test]
fn empty_file_gives_working_defaults() {
    let c: TeamConfig = toml::from_str("").unwrap();
    assert_eq!(c.labels.task, "agent-task");
    assert_eq!(c.models.coder, "claude-opus-5-5");
    assert!(c.roles.coder && !c.roles.product_owner);
    assert_eq!(c.discovery.org, "Akhetonics");
    assert!(!c.machine.is_empty());
    c.validate().unwrap();
}

#[test]
fn partial_file_keeps_other_defaults() {
    let c: TeamConfig = toml::from_str(
        r#"
        machine = "linux-box"
        [[repos]]
        name = "aignermax/Lunima"
        routing = "kimi-only"
        [budgets]
        max_turns_regular = 99
    "#,
    )
    .unwrap();
    assert_eq!(c.machine, "linux-box");
    assert_eq!(c.repos[0].routing, Routing::KimiOnly);
    assert!(c.repos[0].coder, "missing field defaults to true");
    assert_eq!(c.budgets.max_turns_regular, 99);
    assert_eq!(c.budgets.max_turns_complex, 500);
    assert_eq!(c.repo("AIGNERMAX/lunima").unwrap().routing, Routing::KimiOnly);
}

#[test]
fn save_load_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("sub").join("team.toml");
    let mut c = TeamConfig::default();
    c.repos.push(RepoConfig { name: "o/r".into(), ..Default::default() });
    c.product_owner.repo = "o/r".into();
    c.save(&path).unwrap();
    assert_eq!(TeamConfig::load(&path).unwrap(), c);
}

#[test]
fn validation_rejects_bad_repo_names() {
    let mut c = TeamConfig::default();
    c.repos.push(RepoConfig { name: "no-slash".into(), ..Default::default() });
    assert!(c.validate().is_err());
}

#[test]
fn endpoint_key_from_file_or_env() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("key.txt");
    std::fs::write(&f, "  sk-or-123\n").unwrap();
    let e = Endpoint { key_file: f.display().to_string(), ..Default::default() };
    assert_eq!(e.api_key().as_deref(), Some("sk-or-123"));
    std::env::set_var("TEAM_CORE_TEST_KEY", "sk-env");
    let e = Endpoint { key_env: "TEAM_CORE_TEST_KEY".into(), ..Default::default() };
    assert_eq!(e.api_key().as_deref(), Some("sk-env"));
    assert_eq!(Endpoint::default().api_key(), None);
}

#[test]
fn save_keeps_unknown_keys() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("team.toml");
    std::fs::write(&path, "future_key = 1\n[budgets]\nfuture_budget = 7\nmax_turns_regular = 99\n").unwrap();
    let mut c = TeamConfig::load(&path).unwrap();
    assert_eq!(c.budgets.max_turns_regular, 99);
    c.poll_seconds = 30;
    c.save(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("future_key = 1") && text.contains("future_budget = 7"), "{text}");
    assert_eq!(TeamConfig::load(&path).unwrap().poll_seconds, 30);
}

#[test]
fn product_discovery_defaults_and_validation() {
    let cfg = TeamConfig::default();
    assert!(!cfg.roles.product_discovery, "off unless switched on");
    assert_eq!((cfg.product_discovery.weekday.as_str(), cfg.product_discovery.weekly_budget_usd, cfg.product_discovery.analytics.as_str()), ("mon", 25.0, "none"));
    assert!(cfg.validate().is_ok());
    let bad = |f: &dyn Fn(&mut TeamConfig)| {
        let mut c = TeamConfig::default();
        f(&mut c);
        c.validate().unwrap_err()
    };
    assert!(bad(&|c| c.product_discovery.weekday = "monday".into()).contains("weekday"));
    assert!(bad(&|c| c.product_discovery.hour = 24).contains("hour"));
    assert!(bad(&|c| c.product_discovery.weekly_budget_usd = f64::NAN).contains("budget"));
    assert!(bad(&|c| c.product_discovery.analytics = "google".into()).contains("analytics"));
    let parsed: TeamConfig = toml::from_str("[roles]\nproduct_discovery = true\n[product_discovery]\nweekday = \"fri\"\n").unwrap();
    assert!(parsed.roles.product_discovery && parsed.product_discovery.weekday == "fri" && parsed.product_discovery.max_open_opportunities == 3);
}
