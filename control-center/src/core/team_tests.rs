use super::*;
use crate::core::config::ConfigDoc;
use std::path::PathBuf;

fn env(text: &str) -> (tempfile::TempDir, EnvFile) {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join(".env");
    std::fs::write(&p, text).unwrap();
    let e = EnvFile::load(&p).unwrap();
    (tmp, e)
}

fn snapshot(workers: bool) -> (tempfile::TempDir, Snapshot) {
    let mut s = Snapshot::empty(PathBuf::from("root"), Some(PathBuf::from("cli.exe")));
    let map = serde_json::json!({"githubRepo": "aignermax/Lunima", "clonePath": "C:/c", "workersEnabled": workers}).as_object().unwrap().clone();
    s.config = Ok(ConfigDoc { path: PathBuf::from("agent-loop.json"), map, had_comments: false });
    let (tmp, e) = env("AGENT_REPOS=aignermax/Lunima\n");
    s.team.dir = Some(PathBuf::from("agent"));
    s.team.env = Some(e);
    let all_active = UNITS.iter().map(|(r, _)| (r.to_string(), "active".to_string())).collect();
    s.team.units = Some(Ok(all_active));
    for (role, _) in UNITS {
        s.team.heartbeats.insert(role.to_string(), Heartbeat { state: "idle".into(), updated: Some(Local::now()), ..Default::default() });
    }
    (tmp, s)
}

fn titles(s: &Snapshot) -> Vec<(String, Level)> {
    checks(s, Local::now()).into_iter().map(|c| (c.title, c.level)).collect()
}

#[test]
fn healthy_team_has_no_findings() {
    let (_t, s) = snapshot(false);
    assert!(titles(&s).is_empty(), "{:?}", titles(&s));
}

#[test]
fn stopped_unit_is_an_error_with_start_fix() {
    let (_t, mut s) = snapshot(false);
    if let Some(Ok(u)) = s.team.units.as_mut() {
        u.insert("qa".into(), "inactive".into());
    }
    let c = checks(&s, Local::now()).into_iter().find(|c| c.title == "QA-Tester läuft nicht").unwrap();
    assert_eq!(c.level, Level::Error);
    assert_eq!(c.fix.unwrap().1, Fix::StartUnit("qa".into()));
}

#[test]
fn paused_role_offers_resume() {
    let (_t, mut s) = snapshot(false);
    s.team.pauses.insert("coder".into(), "Urlaub".into());
    let c = checks(&s, Local::now()).into_iter().find(|c| c.title == "Coder pausiert").unwrap();
    assert_eq!(c.fix.unwrap().1, Fix::ResumeRole("coder".into()));
}

#[test]
fn silent_idle_role_is_flagged_but_long_work_is_not() {
    let (_t, mut s) = snapshot(false);
    let old = Local::now() - Duration::minutes(30);
    s.team.heartbeats.insert("coder".into(), Heartbeat { state: "idle".into(), updated: Some(old), ..Default::default() });
    s.team.heartbeats.insert("qa".into(), Heartbeat { state: "working".into(), updated: Some(old), ..Default::default() });
    let t = titles(&s);
    assert!(t.iter().any(|(n, _)| n == "Coder reagiert nicht"));
    assert!(!t.iter().any(|(n, _)| n == "QA-Tester reagiert nicht"), "a 30-min test suite is fine");
}

#[test]
fn this_weeks_outages_are_reported() {
    let (_t, mut s) = snapshot(false);
    let now = Local::now();
    s.team.log = Some(LogFindings {
        model_unsupported: Some(now - Duration::minutes(5)),
        not_logged_in: Some(now - Duration::minutes(5)),
        claude_failures: vec![now; 4],
        qa_loop: Some((1471, 154, now - Duration::minutes(10))),
        log_bytes: 491 * 1024 * 1024,
    });
    let t = titles(&s);
    for expected in ["Claude-CLI in WSL zu alt", "Claude in WSL nicht angemeldet", "QA-Schleife", "Claude-Aufrufe scheitern", "agent.log sehr groß"] {
        assert!(t.iter().any(|(n, _)| n == expected), "missing {expected}: {t:?}");
    }
}

#[test]
fn old_log_findings_expire() {
    let (_t, mut s) = snapshot(false);
    let old = Local::now() - Duration::hours(5);
    s.team.log = Some(LogFindings { model_unsupported: Some(old), qa_loop: Some((1, 99, old)), ..Default::default() });
    assert!(titles(&s).is_empty());
}

#[test]
fn two_coders_on_one_repo_offer_disabling_loop_workers() {
    let (_t, s) = snapshot(true);
    let c = checks(&s, Local::now()).into_iter().find(|c| c.title == "Zwei Coder auf einem Repo").unwrap();
    assert_eq!(c.fix.unwrap().1, Fix::DisableLoopWorkers);
}

#[test]
fn po_only_setup_has_no_team_findings() {
    let (_t, mut s) = snapshot(false);
    s.team = TeamSnapshot::default();
    assert!(titles(&s).is_empty());
}
