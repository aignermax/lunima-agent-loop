use super::*;

fn cfg() -> TeamConfig {
    toml::from_str("[[repos]]\nname='o/r'\nrouting='codex'\n[repos.profile]\nallowed_backends=['codex','claude']\n[repos.profile.standard]\nmodel='normal'\neffort='medium'\n[repos.profile.complex]\nmodel='complex'\neffort='high'\n[repos.profile.reviewer]\nbackend='claude'\nmodel='review'\n[repos.profile.critical]\nmodel='critical-review'\n").unwrap()
}

#[test]
fn role_and_label_variants_inherit_without_cross_provider_models_or_effort() {
    let cfg = cfg();
    let d = |role, tags: &[&str]| {
        resolve(
            &cfg,
            "O/R",
            role,
            &tags.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
        )
        .unwrap()
    };
    assert_eq!(d(SessionRole::Coder, &[]).choice.model, "normal");
    assert_eq!(d(SessionRole::Coder, &["complex"]).choice.model, "complex");
    assert_eq!(
        d(SessionRole::Coder, &["complex"]).effort,
        Some(ReasoningEffort::High)
    );
    let reviewer = d(SessionRole::Reviewer, &[]);
    assert_eq!(reviewer.backend, Backend::Claude);
    assert_eq!(reviewer.choice.model, "review");
    assert_eq!(reviewer.effort, None);
    assert_eq!(
        d(SessionRole::Qa, &["critical"]).choice.model,
        "critical-review"
    );
    assert_eq!(d(SessionRole::ProductOwner, &[]).choice.model, "normal");
    assert_eq!(d(SessionRole::Discovery, &[]).choice.model, "normal");
    assert_eq!(
        d(SessionRole::Feedback, &["complex"]).choice.model,
        "complex"
    );
}

#[test]
fn absent_complex_rule_keeps_standard_while_budget_escalates_independently() {
    let mut cfg = cfg();
    let p = cfg.repos[0].profile.as_mut().unwrap();
    p.complex = None;
    p.budgets.max_tokens_complex = Some(1234);
    p.budgets.timeout_minutes_complex = Some(17);
    let normal = resolve(&cfg, "o/r", SessionRole::Coder, &[]).unwrap();
    let complex = resolve(&cfg, "o/r", SessionRole::Coder, &["complex".into()]).unwrap();
    assert_eq!(normal.choice.model, complex.choice.model);
    let budgets = budgets_for(&cfg, "o/r");
    assert_eq!(budgets.max_tokens_complex, 1234);
    assert_eq!(budgets.timeout_minutes_complex, 17);
    assert_eq!(budgets.max_attempts, cfg.budgets.max_attempts);
}

#[test]
fn changing_backend_uses_its_global_defaults_and_explicit_cli_empty_is_preserved() {
    let mut cfg = cfg();
    let p = cfg.repos[0].profile.as_mut().unwrap();
    p.reviewer.as_mut().unwrap().model = None;
    assert_eq!(
        resolve(&cfg, "o/r", SessionRole::Reviewer, &[])
            .unwrap()
            .choice
            .model,
        cfg.models.reviewer
    );
    cfg.repos[0].profile.as_mut().unwrap().standard.model = Some(String::new());
    cfg.runner.model = "must-not-override".into();
    assert_eq!(
        resolve(&cfg, "o/r", SessionRole::Coder, &[])
            .unwrap()
            .choice
            .model,
        ""
    );
}

#[test]
fn missing_api_key_uses_only_explicit_allowed_fallback() {
    let mut cfg = cfg();
    cfg.providers.openai.key_env.clear();
    let p = cfg.repos[0].profile.as_mut().unwrap();
    p.allowed_backends.push(Backend::OpenAi);
    p.standard.backend = Some(Backend::OpenAi);
    p.standard.model = Some("api-model".into());
    assert!(resolve(&cfg, "o/r", SessionRole::Coder, &[]).is_err());
    cfg.repos[0].profile.as_mut().unwrap().fallback = Some(ModelSpec {
        backend: Some(Backend::Codex),
        model: Some("fallback".into()),
        ..Default::default()
    });
    let fallback = resolve(&cfg, "o/r", SessionRole::Coder, &[]).unwrap();
    assert_eq!(fallback.backend, Backend::Codex);
    assert_eq!(fallback.choice.model, "fallback");
    cfg.repos[0]
        .profile
        .as_mut()
        .unwrap()
        .allowed_backends
        .retain(|b| *b != Backend::Codex);
    assert!(resolve(&cfg, "o/r", SessionRole::Coder, &[]).is_err());
}

#[test]
fn role_only_provider_and_invalid_effort_cannot_bypass_allowlist() {
    let mut cfg = cfg();
    cfg.repos[0].profile.as_mut().unwrap().allowed_backends = vec![Backend::Codex];
    assert!(
        resolve(&cfg, "o/r", SessionRole::Coder, &[]).is_err(),
        "inactive forbidden reviewer is still rejected"
    );
    cfg.repos[0]
        .profile
        .as_mut()
        .unwrap()
        .allowed_backends
        .push(Backend::Claude);
    cfg.repos[0]
        .profile
        .as_mut()
        .unwrap()
        .reviewer
        .as_mut()
        .unwrap()
        .effort = Some(ReasoningEffort::High);
    assert!(cfg.validate().unwrap_err().contains("Denkaufwand"));
}

#[test]
fn inherited_defaults_are_live_and_explicit_overrides_stay_fixed() {
    let mut cfg = cfg();
    cfg.repos[0].profile.as_mut().unwrap().standard.model = None;
    cfg.runner.model = "first".into();
    assert_eq!(
        resolve(&cfg, "o/r", SessionRole::Coder, &[])
            .unwrap()
            .choice
            .model,
        "first"
    );
    cfg.runner.model = "second".into();
    assert_eq!(
        resolve(&cfg, "o/r", SessionRole::Coder, &[])
            .unwrap()
            .choice
            .model,
        "second"
    );
    assert_eq!(
        resolve(&cfg, "o/r", SessionRole::Coder, &["complex".into()])
            .unwrap()
            .choice
            .model,
        "complex"
    );
}
