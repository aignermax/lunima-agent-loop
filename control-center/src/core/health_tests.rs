use super::*;
use crate::core::agent_files::Heartbeat;
use crate::core::setup::SetupCheck;
use crate::sys::agent_service::ServiceInfo;
use team_core::config::TeamConfig;
use team_core::paths::Paths;
use team_core::state::PoState;

fn healthy() -> Snapshot {
    let now = Local::now();
    let mut s = Snapshot::empty(Paths::new("x"));
    s.config = Ok(TeamConfig::default());
    s.agent_running = true;
    s.agent_exe = Some("lunima-team-agent.exe".into());
    s.service = Some(Ok(ServiceInfo { autostart: true, state: "Running".into() }));
    for role in ["coder", "qa", "pr-feedback"] {
        s.heartbeats.insert(role.into(), Heartbeat { state: "idle".into(), updated: Some(now), ..Default::default() });
    }
    s
}

fn titles(h: &Health) -> Vec<String> {
    h.checks.iter().map(|c| c.title.clone()).collect()
}

#[test]
fn healthy_team_is_green() {
    let h = evaluate(&healthy(), Local::now());
    assert_eq!(h.overall, Level::Ok, "{:?}", titles(&h));
    assert_eq!(h.headline, "Alles in Ordnung");
}

#[test]
fn missing_config_and_stopped_agent_need_attention() {
    let mut s = healthy();
    s.config = Err("fehlt".into());
    s.config_missing = true;
    s.agent_running = false;
    let h = evaluate(&s, Local::now());
    assert_eq!(h.overall, Level::Error);
    let cfg = h.checks.iter().find(|c| c.title == "Keine Konfiguration").unwrap();
    assert_eq!(cfg.fix.as_ref().unwrap().1, Fix::OpenSetup);
    let agent = h.checks.iter().find(|c| c.title == "Team-Agent läuft nicht").unwrap();
    assert!(agent.fix.is_none(), "no start without a config");
}

#[test]
fn paused_and_silent_roles_offer_fixes() {
    let mut s = healthy();
    let now = Local::now();
    s.pauses.insert("qa".into(), "Control Center".into());
    s.heartbeats.get_mut("coder").unwrap().updated = Some(now - Duration::minutes(45));
    let h = evaluate(&s, now);
    let fix = |t: &str| h.checks.iter().find(|c| c.title == t).and_then(|c| c.fix.clone()).map(|f| f.1);
    assert_eq!(fix("QA-Tester pausiert"), Some(Fix::ResumeRole("qa".into())));
    assert_eq!(fix("Coder: reagiert nicht"), Some(Fix::RestartAgent));
    assert_eq!(h.overall, Level::Warn);
}

#[test]
fn failed_setup_steps_and_logged_out_claude_are_errors() {
    let mut s = healthy();
    s.setup = Some(vec![SetupCheck { title: "GitHub-Anmeldung", ok: false, optional: false, detail: "not logged in".into(), action: None }]);
    s.log = Some(crate::core::agent_files::LogFindings { not_logged_in: Some(Local::now()), ..Default::default() });
    let h = evaluate(&s, Local::now());
    assert!(titles(&h).contains(&"GitHub-Anmeldung".to_string()));
    assert!(titles(&h).contains(&"Claude nicht angemeldet".to_string()));
    assert_eq!(h.overall, Level::Error);
}

#[test]
fn product_owner_results_show_up_when_enabled() {
    let mut s = healthy();
    let mut cfg = TeamConfig::default();
    cfg.roles.product_owner = true;
    s.config = Ok(cfg);
    s.heartbeats.insert("product-owner".into(), Heartbeat { state: "idle".into(), updated: Some(Local::now()), ..Default::default() });
    s.po = Some(PoState { last_result: "timeout".into(), last_start: Some(Local::now().to_rfc3339()), ..Default::default() });
    assert!(titles(&evaluate(&s, Local::now())).contains(&"Letzter PO-Lauf fehlgeschlagen".to_string()));
    s.po = Some(PoState { last_result: "ok".into(), next_due: Some((Local::now() - Duration::hours(3)).to_rfc3339()), ..Default::default() });
    assert!(titles(&evaluate(&s, Local::now())).contains(&"PO-Lauf überfällig".to_string()));
}

#[test]
fn everything_paused_reads_as_paused() {
    let mut s = healthy();
    for r in ["coder", "qa", "pr-feedback"] {
        s.pauses.insert(r.into(), "x".into());
    }
    assert_eq!(evaluate(&s, Local::now()).headline, "Pausiert");
}
