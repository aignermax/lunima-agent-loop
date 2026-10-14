use super::*;

const ENV: &str = r#"
GITHUB_TOKEN=ghp_secret
ANTHROPIC_API_KEY=sk-ant-secret
AGENT_REPOS=aignermax/Lunima, Akhetonics/khepri
AGENT_CODER_MODEL=claude-opus-5-5
AGENT_REVIEWER_MODEL=claude-sonnet-4-6 # Default reviewer model
AGENT_OPENROUTER_REPOS=aignermax/Lunima
AGENT_OPENROUTER_FORCE_REPOS=aignermax/Lunima
AGENT_OPENROUTER_KEY_FILE=/mnt/c/Users/MaxAigner/.ssh/openrouter.txt
AGENT_OPENROUTER_MODEL=moonshotai/kimi-k3
AGENT_PR_LABELS=agent-pr
AGENT_DISCOVERY_ORG=
AGENT_MAX_TURNS_REGULAR=200
AGENT_COMPLEX_USES_CLAUDE=false
"#;

#[test]
fn env_import_maps_repos_routing_models_and_budgets() {
    let mut c = TeamConfig::default();
    apply_env(&mut c, &parse_env(ENV));
    assert_eq!(c.repos.len(), 2);
    assert_eq!(c.repo("aignermax/Lunima").unwrap().routing, Routing::KimiOnly);
    assert_eq!(c.repo("Akhetonics/khepri").unwrap().routing, Routing::Claude);
    assert_eq!(c.models.reviewer, "claude-sonnet-4-6", "inline comment stripped");
    assert_eq!(c.discovery.org, "", "explicit empty = discovery off");
    assert_eq!(c.budgets.max_turns_regular, 200);
    assert!(!c.providers.complex_uses_premium);
    assert_eq!(c.providers.openrouter.model, "moonshotai/kimi-k3");
}

#[test]
fn secrets_are_never_imported() {
    let mut c = TeamConfig::default();
    apply_env(&mut c, &parse_env(ENV));
    let text = toml::to_string(&c).unwrap();
    assert!(!text.contains("ghp_secret") && !text.contains("sk-ant-secret"));
}

#[test]
fn wsl_paths_become_windows_paths_on_windows() {
    let p = native_path("/mnt/c/Users/MaxAigner/.ssh/openrouter.txt");
    if cfg!(windows) {
        assert_eq!(p, r"C:\Users\MaxAigner\.ssh\openrouter.txt");
    } else {
        assert_eq!(p, "/mnt/c/Users/MaxAigner/.ssh/openrouter.txt");
    }
    assert_eq!(native_path("/home/x/key"), "/home/x/key");
}

#[test]
fn loop_config_import_sets_po_but_keeps_role_off() {
    let json = serde_json::json!({
        "githubRepo": "aignermax/Lunima", "integrationBranch": "dev", "baseBranch": "main",
        "ownerIntervalMinutes": 240, "ownerRunner": "claude", "ownerModel": "claude-opus-5-5",
        "workersEnabled": false
    });
    let mut c = TeamConfig::default();
    apply_loop_config(&mut c, &json, None);
    assert_eq!(c.product_owner.repo, "aignermax/Lunima");
    assert_eq!(c.product_owner.interval_minutes, 240);
    assert!(!c.roles.product_owner);
    assert!(c.product_owner.customer_command.is_empty(), "customer review stays off unless enabled");
    let mut c = TeamConfig::default();
    apply_loop_config(&mut c, &serde_json::json!({"githubRepo": "o/r", "customerEnabled": true, "customerMaxAgeHours": 12, "customerPython": "C:/Program Files/Py/python.exe", "customerMaxReviewsPerCycle": 2, "customerTimeoutMinutes": 20, "ownerIntervalMinutes": 60}), Some(std::path::Path::new("L")));
    let cmd = &c.product_owner.customer_command;
    assert!(cmd.contains("--state-dir") && cmd.contains("--max-age 43200") && cmd.contains("--max-reviews 2"), "{cmd}");
    assert!(cmd.starts_with("\"C:/Program Files/Py/python.exe\""), "{cmd}");
    assert!(cmd.contains("customer_cycle.py") && cmd.contains("L"), "{cmd}");
    assert_eq!(c.product_owner.customer_timeout_minutes, 95);
    assert_eq!(c.product_owner.max_runs_per_day, 24);
    assert!(c.product_owner.customer_command.contains("--repo o/r --base"));
    assert_eq!(c.product_owner.customer_max_age_hours, 12);
}

#[test]
fn from_legacy_reads_both_files() {
    let tmp = tempfile::tempdir().unwrap();
    let env = tmp.path().join(".env");
    let json = tmp.path().join("agent-loop.json");
    std::fs::write(&env, "AGENT_REPOS=o/r\n").unwrap();
    std::fs::write(&json, "\u{feff}{\"githubRepo\":\"o/r\"}").unwrap();
    let c = from_legacy(Some(&env), Some(&json)).unwrap();
    assert_eq!(c.coder_repos(), ["o/r"]);
    assert_eq!(c.product_owner.repo, "o/r");
    assert_eq!(from_legacy(None, None).unwrap(), TeamConfig::default());
    assert!(from_legacy(Some(&tmp.path().join("missing.env")), None).is_err());
    std::fs::write(tmp.path().join("bad.json"), "{ nope").unwrap();
    assert!(from_legacy(None, Some(&tmp.path().join("bad.json"))).is_err());
}

#[test]
fn provider_list_repos_keep_their_routing_and_premium_follows_coder() {
    let mut c = TeamConfig::default();
    let env = parse_env("AGENT_REPOS=o/a\nAGENT_OPENROUTER_FORCE_REPOS=o/forced\nAGENT_OPENROUTER_REPOS=o/kimi\nAGENT_CODER_MODEL=m1\n");
    apply_env(&mut c, &env);
    assert_eq!(c.repo("o/forced").unwrap().routing, Routing::KimiOnly);
    assert_eq!(c.repo("o/kimi").unwrap().routing, Routing::Kimi);
    assert_eq!(c.models.premium, "m1");
}
