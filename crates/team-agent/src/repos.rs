//! Which repositories the roles look at: the configured ones plus those discovered in the
//! org (an open `agent-task` issue, an open `Agent:` PR or a `qa-failed` PR). Discovery runs
//! in the coder thread; registry and work hints are files every role reads.
//!
//! * `discovered-repos.json` — `{"o/r": {"first_seen": "…", "last_seen": "…"}}`
//! * `work-hints.json` — `{"issues": [...], "qa_failed": [...], "ts": "…"}`

use chrono::{Duration, Local, NaiveDateTime};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::path::Path;
use team_core::config::TeamConfig;
use team_core::state::write_atomic;

pub const REGISTRY: &str = "discovered-repos.json";
pub const HINTS: &str = "work-hints.json";
const STAMP: &str = "%Y-%m-%d %H:%M:%S";

fn read_object(path: &Path) -> Map<String, Value> {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default()
}

fn stamp_age_ok(v: &Value, key: &str, max: Duration) -> bool {
    v.get(key).and_then(Value::as_str).and_then(|s| NaiveDateTime::parse_from_str(s, STAMP).ok()).is_some_and(|t| Local::now().naive_local() - t <= max)
}

/// Registry repos seen within `expiry_days`, sorted.
pub fn active_discovered(state: &Path, expiry_days: i64) -> Vec<String> {
    let map = read_object(&state.join(REGISTRY));
    let mut out: Vec<String> = map.iter().filter(|(_, v)| stamp_age_ok(v, "last_seen", Duration::days(expiry_days))).map(|(k, _)| k.clone()).collect();
    out.sort();
    out
}

/// Configured repos (in order) then active discovered ones, case-insensitively unique.
/// `coder_only` drops configured repos whose coder is switched off.
pub fn effective(cfg: &TeamConfig, state: &Path, coder_only: bool) -> Vec<String> {
    let mut out: Vec<String> = cfg.repos.iter().filter(|r| !coder_only || r.coder).map(|r| r.name.clone()).collect();
    if !cfg.discovery.org.is_empty() {
        for r in active_discovered(state, cfg.discovery.expiry_days) {
            if !cfg.repos.iter().any(|c| c.name.eq_ignore_ascii_case(&r)) && !out.iter().any(|o| o.eq_ignore_ascii_case(&r)) {
                out.push(r);
            }
        }
    }
    out
}

/// Adds/refreshes repos, then drops entries not seen for `expiry_days`. Returns new ones.
pub fn update_registry(state: &Path, seen: &BTreeSet<String>, expiry_days: i64) -> Vec<String> {
    let path = state.join(REGISTRY);
    let mut map = read_object(&path);
    let now = Local::now().format(STAMP).to_string();
    let mut added = Vec::new();
    for r in seen {
        match map.get_mut(r).and_then(Value::as_object_mut) {
            Some(e) => {
                e.insert("last_seen".into(), json!(now));
            }
            None => {
                map.insert(r.clone(), json!({"first_seen": now, "last_seen": now}));
                added.push(r.clone());
            }
        }
    }
    map.retain(|_, v| stamp_age_ok(v, "last_seen", Duration::days(expiry_days)));
    let _ = write_atomic(&path, serde_json::to_string_pretty(&Value::Object(map)).unwrap_or_default().as_bytes());
    added
}

/// Drops a repo that can no longer be set up (deleted, access lost).
pub fn forget(state: &Path, repo: &str) {
    let path = state.join(REGISTRY);
    let mut map = read_object(&path);
    if map.remove(repo).is_some() {
        let _ = write_atomic(&path, serde_json::to_string_pretty(&Value::Object(map)).unwrap_or_default().as_bytes());
    }
}

pub fn save_hints(state: &Path, issues: &BTreeSet<String>, qa_failed: &BTreeSet<String>) {
    let v = json!({"issues": issues, "qa_failed": qa_failed, "ts": Local::now().format(STAMP).to_string()});
    let _ = write_atomic(&state.join(HINTS), v.to_string().as_bytes());
}

/// Repos with known coder work, if the hints are fresh (else empty = no preference).
pub fn load_hints(state: &Path, max_age: Duration) -> BTreeSet<String> {
    let v = Value::Object(read_object(&state.join(HINTS)));
    if !stamp_age_ok(&v, "ts", max_age) {
        return BTreeSet::new();
    }
    ["issues", "qa_failed"].iter().flat_map(|k| v.get(*k).and_then(Value::as_array).cloned().unwrap_or_default()).filter_map(|x| x.as_str().map(|s| s.to_lowercase())).collect()
}

/// Search function (`gh::search_repos`), injectable for tests.
pub type Search<'a> = &'a dyn Fn(&str) -> Result<Vec<String>, String>;

/// One discovery sweep: four searches, registry update, work hints.
pub fn sweep(cfg: &TeamConfig, state: &Path, search: Search) -> Result<Vec<String>, String> {
    let org = &cfg.discovery.org;
    let task = &cfg.labels.task;
    let set = |q: String| -> Result<BTreeSet<String>, String> { Ok(search(&q)?.into_iter().collect()) };
    let issues = set(format!("org:{org} label:\"{task}\" is:issue is:open"))?;
    let prs = set(format!("org:{org} is:pr is:open in:title \"Agent:\""))?;
    let qa_failed = set(format!("org:{org} is:pr is:open label:{}", cfg.labels.qa_failed))?;
    let configured: Vec<String> = cfg.repos.iter().map(|r| r.name.clone()).collect();
    let manual = if configured.is_empty() {
        BTreeSet::new()
    } else {
        set(format!("{} label:\"{task}\" is:issue is:open", configured.iter().map(|r| format!("repo:{r}")).collect::<Vec<_>>().join(" ")))?
    };
    let usage: BTreeSet<String> = issues.iter().chain(&prs).chain(&qa_failed).filter(|r| !configured.iter().any(|c| c.eq_ignore_ascii_case(r))).cloned().collect();
    let added = update_registry(state, &usage, cfg.discovery.expiry_days);
    save_hints(state, &issues.union(&manual).cloned().collect(), &qa_failed);
    Ok(added)
}

/// Round-robin over a list that may change between cycles.
#[derive(Default)]
pub struct RoundRobin {
    last: Option<String>,
}

impl RoundRobin {
    /// The entry after the previously returned one (by name, so list changes don't skip repos).
    pub fn next(&mut self, list: &[String]) -> Option<String> {
        if list.is_empty() {
            return None;
        }
        let idx = self.last.as_ref().and_then(|l| list.iter().position(|x| x.eq_ignore_ascii_case(l))).map(|i| (i + 1) % list.len()).unwrap_or(0);
        self.last = Some(list[idx].clone());
        Some(list[idx].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use team_core::config::RepoConfig;

    fn cfg(repos: &[&str]) -> TeamConfig {
        let mut c = TeamConfig::default();
        c.repos = repos.iter().map(|r| RepoConfig { name: r.to_string(), ..Default::default() }).collect();
        c
    }

    #[test]
    fn sweep_registers_org_repos_and_writes_hints() {
        let t = tempfile::tempdir().unwrap();
        let c = cfg(&["aignermax/Lunima"]);
        let queries = std::sync::Mutex::new(Vec::new());
        let search = |q: &str| -> Result<Vec<String>, String> {
            queries.lock().unwrap().push(q.to_string());
            Ok(if q.contains("label:\"agent-task\" is:issue") && q.starts_with("org:") { vec!["Akhetonics/a".into()] } else if q.contains("qa-failed") { vec!["Akhetonics/b".into()] } else if q.starts_with("repo:") { vec!["aignermax/Lunima".into()] } else { vec![] })
        };
        let added = sweep(&c, t.path(), &search).unwrap();
        assert_eq!(added, vec!["Akhetonics/a", "Akhetonics/b"]);
        let q = queries.lock().unwrap().clone();
        assert!(q.contains(&"org:Akhetonics is:pr is:open in:title \"Agent:\"".to_string()));
        assert!(q.contains(&"repo:aignermax/Lunima label:\"agent-task\" is:issue is:open".to_string()));
        assert_eq!(effective(&c, t.path(), false), vec!["aignermax/Lunima", "Akhetonics/a", "Akhetonics/b"]);
        let hints = load_hints(t.path(), Duration::minutes(6));
        assert!(hints.contains("akhetonics/a") && hints.contains("akhetonics/b") && hints.contains("aignermax/lunima"));
        assert!(sweep(&c, t.path(), &search).unwrap().is_empty(), "known repos are not new");
    }

    #[test]
    fn expired_and_broken_entries_are_dropped() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join(REGISTRY), r#"{"o/old":{"last_seen":"2020-01-01 00:00:00"},"o/bad":{"last_seen":"x"},"o/new":{"last_seen":"2999-01-01 00:00:00"}}"#).unwrap();
        update_registry(t.path(), &BTreeSet::new(), 8);
        let map = read_object(&t.path().join(REGISTRY));
        assert_eq!(map.keys().cloned().collect::<Vec<_>>(), vec!["o/new"]);
        forget(t.path(), "o/new");
        assert!(read_object(&t.path().join(REGISTRY)).is_empty());
    }

    #[test]
    fn discovery_off_means_configured_only() {
        let t = tempfile::tempdir().unwrap();
        update_registry(t.path(), &["o/x".to_string()].into_iter().collect(), 8);
        let mut c = cfg(&["o/a"]);
        c.repos[0].coder = false;
        assert_eq!(effective(&c, t.path(), true), vec!["o/x"]);
        c.discovery.org.clear();
        assert_eq!(effective(&c, t.path(), false), vec!["o/a"]);
    }

    #[test]
    fn round_robin_follows_names() {
        let mut rr = RoundRobin::default();
        let l = vec!["a".to_string(), "b".into(), "c".into()];
        assert_eq!(rr.next(&l).unwrap(), "a");
        assert_eq!(rr.next(&l).unwrap(), "b");
        assert_eq!(rr.next(&["b".to_string(), "c".into()]).unwrap(), "c");
        assert_eq!(rr.next(&l).unwrap(), "a");
        assert!(rr.next(&[]).is_none());
    }
}
