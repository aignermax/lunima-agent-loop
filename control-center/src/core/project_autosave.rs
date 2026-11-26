use std::time::{Duration, Instant};
use team_core::config::{RepoConfig, TeamConfig};

pub const TYPING_DELAY: Duration = Duration::from_millis(450);
const RETRY_DELAY: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub struct ChangeSet {
    pub base: Vec<RepoConfig>,
    pub edited: Vec<RepoConfig>,
}

#[derive(Default)]
pub struct Autosave {
    base: Vec<RepoConfig>,
    draft: Option<Vec<RepoConfig>>,
    pending: Option<ChangeSet>,
    due: Option<Instant>,
    failed_save: bool,
    pub error: Option<String>,
}

impl Autosave {
    pub fn is_idle(&self) -> bool {
        self.draft.is_none() && self.pending.is_none() && self.error.is_none()
    }
    pub fn is_saving(&self) -> bool {
        self.pending.is_some()
    }
    pub fn repaint_after(&self, now: Instant) -> Option<Duration> {
        if self.pending.is_some() {
            Some(Duration::from_millis(50))
        } else {
            self.due.map(|due| {
                due.saturating_duration_since(now)
                    .max(Duration::from_millis(10))
            })
        }
    }
    pub fn observe(&mut self, saved: &[RepoConfig]) {
        if self.pending.is_some() {
            return;
        }
        if let Some(draft) = &self.draft {
            match merge_repos(&self.base, draft, saved) {
                Ok(merged) => {
                    self.draft = (merged != saved || self.failed_save).then_some(merged);
                    if self.draft.is_none() {
                        self.due = None;
                        self.error = None;
                    }
                    self.base = saved.to_vec();
                }
                Err(e) => self.error = Some(e),
            }
        } else {
            self.base = saved.to_vec();
        }
    }
    pub fn shown(&self, saved: &[RepoConfig]) -> Vec<RepoConfig> {
        self.draft.clone().unwrap_or_else(|| saved.to_vec())
    }
    pub fn edit(
        &mut self,
        saved: &[RepoConfig],
        edited: Vec<RepoConfig>,
        typing: bool,
        now: Instant,
    ) {
        if self.draft.is_none() {
            self.base = saved.to_vec();
        }
        // Keep a reversal even if the old file already shows it: the preceding
        // in-flight save may still write the opposite value.
        self.draft =
            (self.pending.is_some() || self.failed_save || edited != saved).then_some(edited);
        self.due = self
            .draft
            .as_ref()
            .map(|_| now + if typing { TYPING_DELAY } else { Duration::ZERO });
        self.error = None;
    }
    pub fn expedite(&mut self, now: Instant) {
        if self.draft.is_some() {
            self.due = Some(now);
        }
    }
    pub fn request(&mut self, cfg: &TeamConfig, now: Instant) -> Option<ChangeSet> {
        if self.pending.is_some() || self.due.is_none_or(|due| due > now) {
            return None;
        }
        let edited = match merge_repos(&self.base, self.draft.as_ref()?, &cfg.repos) {
            Ok(edited) => edited,
            Err(e) => {
                self.error = Some(e);
                self.due = Some(now + RETRY_DELAY);
                return None;
            }
        };
        let mut checked = cfg.clone();
        checked.repos = edited.clone();
        if let Err(e) = checked.validate() {
            self.error = Some(e);
            self.due = Some(now + RETRY_DELAY);
            return None;
        }
        let request = ChangeSet {
            base: cfg.repos.clone(),
            edited,
        };
        self.base = request.base.clone();
        self.draft = Some(request.edited.clone());
        self.pending = Some(request.clone());
        Some(request)
    }
    pub fn complete(&mut self, result: Result<Vec<RepoConfig>, String>, now: Instant) {
        let Some(sent) = self.pending.take() else {
            return;
        };
        match result {
            Ok(saved) => {
                let desired = self.draft.as_ref().unwrap_or(&sent.edited);
                match merge_repos(&sent.edited, desired, &saved) {
                    Ok(merged) => {
                        self.draft = (merged != saved).then_some(merged);
                        self.base = saved;
                        self.failed_save = false;
                        self.error = None;
                        if self.draft.is_none() {
                            self.due = None;
                        }
                    }
                    Err(e) => {
                        self.error = Some(e);
                        self.due = Some(now + RETRY_DELAY);
                    }
                }
            }
            Err(e) => {
                self.failed_save = true;
                self.error = Some(e);
                self.due = Some(now + RETRY_DELAY);
            }
        }
    }
}

/// Apply only fields edited locally to the newest file contents.
pub fn merge_repos(
    base: &[RepoConfig],
    edited: &[RepoConfig],
    current: &[RepoConfig],
) -> Result<Vec<RepoConfig>, String> {
    let mut result = current.to_vec();
    for old in base {
        if !edited
            .iter()
            .any(|r| r.name.eq_ignore_ascii_case(&old.name))
        {
            result.retain(|r| !r.name.eq_ignore_ascii_case(&old.name));
        }
    }
    for edit in edited {
        let before = base
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case(&edit.name));
        if before == Some(edit) {
            continue;
        }
        if let Some(index) = result
            .iter()
            .position(|r| r.name.eq_ignore_ascii_case(&edit.name))
        {
            let before = before
                .map(toml::Value::try_from)
                .transpose()
                .map_err(|e| e.to_string())?;
            let after = toml::Value::try_from(edit).map_err(|e| e.to_string())?;
            let latest = toml::Value::try_from(&result[index]).map_err(|e| e.to_string())?;
            result[index] = patch(before.as_ref(), Some(&after), Some(&latest))
                .expect("edited repo exists")
                .try_into()
                .map_err(|e: toml::de::Error| e.to_string())?;
        } else if before.is_none() {
            result.push(edit.clone());
        } else {
            return Err(format!("Projekt {} wurde zwischenzeitlich entfernt. Entferne den lokalen Eintrag ebenfalls oder füge es neu hinzu.", edit.name));
        }
    }
    Ok(result)
}

fn patch(
    base: Option<&toml::Value>,
    edited: Option<&toml::Value>,
    current: Option<&toml::Value>,
) -> Option<toml::Value> {
    if base == edited {
        return current.cloned();
    }
    if let (Some(toml::Value::Table(before)), Some(toml::Value::Table(after))) = (base, edited) {
        let mut result = current
            .and_then(toml::Value::as_table)
            .cloned()
            .unwrap_or_default();
        for key in before
            .keys()
            .chain(after.keys())
            .collect::<std::collections::BTreeSet<_>>()
        {
            match patch(before.get(key), after.get(key), result.get(key)) {
                Some(value) => {
                    result.insert(key.clone(), value);
                }
                None => {
                    result.remove(key);
                }
            }
        }
        Some(toml::Value::Table(result))
    } else {
        edited.cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use team_core::config::Routing;

    fn config() -> TeamConfig {
        TeamConfig {
            repos: vec![RepoConfig {
                name: "o/r".into(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn switches_save_immediately_and_typing_saves_automatically_after_a_pause() {
        let cfg = config();
        let now = Instant::now();
        let mut edits = Autosave::default();
        let mut changed = cfg.repos.clone();
        changed[0].routing = Routing::Codex;
        edits.edit(&cfg.repos, changed.clone(), false, now);
        assert_eq!(edits.request(&cfg, now).unwrap().edited, changed);
        edits.complete(Ok(changed.clone()), now);
        assert!(edits.is_idle());
        let mut cfg = cfg;
        cfg.repos = changed.clone();
        changed[0].coder = false;
        edits.edit(&cfg.repos, changed.clone(), true, now);
        assert!(edits.request(&cfg, now).is_none());
        assert!(edits
            .request(&cfg, now + TYPING_DELAY - Duration::from_millis(1))
            .is_none());
        assert_eq!(
            edits.request(&cfg, now + TYPING_DELAY).unwrap().edited,
            changed
        );
    }

    #[test]
    fn reversing_a_switch_during_an_inflight_save_is_saved_after_acknowledgement() {
        let cfg = config();
        let now = Instant::now();
        let mut edits = Autosave::default();
        let mut changed = cfg.repos.clone();
        changed[0].coder = false;
        edits.edit(&cfg.repos, changed.clone(), false, now);
        edits.request(&cfg, now).unwrap();
        edits.edit(&cfg.repos, cfg.repos.clone(), false, now);
        assert_eq!(edits.shown(&cfg.repos), cfg.repos);
        assert!(edits.request(&cfg, now).is_none());
        edits.complete(Ok(changed.clone()), now);
        let mut written = cfg.clone();
        written.repos = changed;
        assert_eq!(edits.request(&written, now).unwrap().edited, cfg.repos);
        edits.complete(Ok(cfg.repos.clone()), now);
        assert!(edits.is_idle());
    }

    #[test]
    fn write_failures_keep_input_and_retry_without_a_save_button() {
        let cfg = config();
        let now = Instant::now();
        let mut edits = Autosave::default();
        let mut changed = cfg.repos.clone();
        changed[0].routing = Routing::Codex;
        edits.edit(&cfg.repos, changed.clone(), false, now);
        edits.request(&cfg, now).unwrap();
        edits.complete(Err("Datei gesperrt".into()), now);
        edits.observe(&cfg.repos);
        assert_eq!(edits.shown(&cfg.repos), changed);
        assert!(edits.error.as_ref().unwrap().contains("gesperrt"));
        assert!(!edits.is_idle());
        assert!(edits.request(&cfg, now).is_none());
        edits.request(&cfg, now + RETRY_DELAY).unwrap();
        edits.complete(Ok(changed), now + RETRY_DELAY);
        assert!(edits.is_idle());
    }

    #[test]
    fn reverting_after_a_write_failure_still_acknowledges_a_successful_retry() {
        let cfg = config();
        let now = Instant::now();
        let mut edits = Autosave::default();
        let mut changed = cfg.repos.clone();
        changed[0].coder = false;
        edits.edit(&cfg.repos, changed, false, now);
        edits.request(&cfg, now).unwrap();
        edits.complete(Err("failed".into()), now);
        edits.edit(&cfg.repos, cfg.repos.clone(), false, now);
        edits.observe(&cfg.repos);
        assert!(
            !edits.is_idle(),
            "the collector's failed-save barrier still needs a successful retry"
        );
        edits.request(&cfg, now).unwrap();
        edits.complete(Ok(cfg.repos.clone()), now);
        assert!(edits.is_idle());
    }

    #[test]
    fn invalid_profile_keeps_last_good_configuration_and_recovers_on_correction() {
        let cfg = config();
        let now = Instant::now();
        let mut edits = Autosave::default();
        let mut changed = cfg.repos.clone();
        changed[0].profile = Some(team_core::project::ProjectProfile::default());
        edits.edit(&cfg.repos, changed.clone(), false, now);
        assert!(edits.request(&cfg, now).is_none());
        assert!(edits.error.is_some());
        assert!(!edits.is_idle());
        assert!(edits.shown(&cfg.repos)[0].profile.is_some());
        changed[0].profile = Some(team_core::project::ProjectProfile::from_legacy(&cfg, "o/r"));
        edits.edit(&cfg.repos, changed, false, now);
        assert!(edits.request(&cfg, now).is_some());
        assert!(cfg.repos[0].profile.is_none());
    }

    #[test]
    fn leaving_page_flushes_typing_without_waiting_for_the_debounce() {
        let cfg = config();
        let now = Instant::now();
        let mut edits = Autosave::default();
        let mut changed = cfg.repos.clone();
        changed[0].routing = Routing::Codex;
        edits.edit(&cfg.repos, changed, true, now);
        edits.expedite(now);
        assert!(edits.request(&cfg, now).is_some());
    }

    #[test]
    fn autosave_preserves_external_changes_while_applying_local_edits() {
        let base = vec![RepoConfig {
            name: "o/r".into(),
            ..Default::default()
        }];
        let mut edited = base.clone();
        edited[0].routing = Routing::Codex;
        let mut current = base.clone();
        current[0].coder = false;
        current.push(RepoConfig {
            name: "another/project".into(),
            ..Default::default()
        });
        let merged = merge_repos(&base, &edited, &current).unwrap();
        assert_eq!(merged[0].routing, Routing::Codex);
        assert!(!merged[0].coder);
        assert_eq!(merged.len(), 2);
    }
}
