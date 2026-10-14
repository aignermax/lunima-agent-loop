//! Projects: the repositories the team works on — configured in team.toml or found by the
//! org discovery — with coder on/off, model routing and the roles each repo's own
//! `.agent.toml` enables.

use std::collections::BTreeMap;
use team_core::config::{RepoConfig, Routing, TeamConfig};

pub fn routing_label(r: Routing) -> &'static str {
    match r {
        Routing::Claude => "Claude",
        Routing::Kimi => "Kimi (OpenRouter)",
        Routing::KimiOnly => "nur Kimi",
    }
}

pub const ROUTINGS: [Routing; 3] = [Routing::Claude, Routing::Kimi, Routing::KimiOnly];

#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub repo: String,
    /// listed in team.toml
    pub configured: bool,
    /// the coder works on it (configured with coder = true, or discovered and not switched off)
    pub coder: bool,
    /// found by the org discovery
    pub discovered: bool,
    /// the product owner's repo
    pub po: bool,
    pub routing: Routing,
}

/// All known repos, de-duplicated case-insensitively, sorted.
pub fn list(cfg: &TeamConfig, discovered: &[String]) -> Vec<Project> {
    let mut all: BTreeMap<String, String> = BTreeMap::new();
    let po = cfg.product_owner.repo.clone();
    for r in cfg.repos.iter().map(|r| &r.name).chain(discovered).chain(std::iter::once(&po)) {
        if !r.is_empty() {
            all.entry(r.to_lowercase()).or_insert_with(|| r.clone());
        }
    }
    all.into_values()
        .map(|repo| {
            let conf = cfg.repo(&repo);
            let discovered = discovered.iter().any(|d| d.eq_ignore_ascii_case(&repo));
            Project {
                configured: conf.is_some(),
                coder: conf.map(|c| c.coder).unwrap_or(discovered),
                discovered,
                po: repo.eq_ignore_ascii_case(&po),
                routing: conf.map(|c| c.routing).unwrap_or_default(),
                repo,
            }
        })
        .collect()
}

fn entry<'a>(cfg: &'a mut TeamConfig, repo: &str) -> &'a mut RepoConfig {
    if let Some(i) = cfg.repos.iter().position(|r| r.name.eq_ignore_ascii_case(repo)) {
        return &mut cfg.repos[i];
    }
    cfg.repos.push(RepoConfig { name: repo.to_string(), ..Default::default() });
    cfg.repos.last_mut().expect("just pushed")
}

/// Coder on/off. A discovered repo gets a team.toml entry so the choice sticks.
pub fn set_coder(cfg: &mut TeamConfig, repo: &str, on: bool) {
    entry(cfg, repo).coder = on;
}

pub fn set_routing(cfg: &mut TeamConfig, repo: &str, routing: Routing) {
    entry(cfg, repo).routing = routing;
}

/// Adds a repo (coder on). Err for anything that is not `owner/name`.
pub fn add(cfg: &mut TeamConfig, repo: &str) -> Result<(), String> {
    let repo = repo.trim();
    if repo.split('/').filter(|p| !p.is_empty()).count() != 2 || repo.contains(' ') {
        return Err(format!("„{repo}“ hat nicht die Form owner/name"));
    }
    entry(cfg, repo).coder = true;
    Ok(())
}

/// Drops the team.toml entry (a discovered repo then follows the discovery again).
pub fn remove(cfg: &mut TeamConfig, repo: &str) {
    cfg.repos.retain(|r| !r.name.eq_ignore_ascii_case(repo));
}

/// Strips a `# comment` that is outside a TOML string.
fn strip_toml_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    for (i, c) in line.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), _) if c == q => quote = None,
            (None, '#') => return &line[..i],
            _ => {}
        }
    }
    line
}

/// `agents_enabled = [...]` from a .agent.toml — single- or multi-line, with comments.
/// Missing or empty means `["coder"]`, exactly like the agent's own loader.
pub fn parse_agents_enabled(toml: &str) -> Vec<String> {
    let text: String = toml.lines().map(strip_toml_comment).collect::<Vec<_>>().join("\n");
    let roles: Vec<String> = text
        .find("agents_enabled")
        .and_then(|i| text[i..].find('[').map(|o| i + o))
        .and_then(|start| text[start..].find(']').map(|end| &text[start + 1..start + end]))
        .map(|inner| inner.split(',').map(|s| s.trim().trim_matches(|c| c == '"' || c == '\'').to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    if roles.is_empty() { vec!["coder".into()] } else { roles }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> TeamConfig {
        let mut c = TeamConfig { repos: vec![RepoConfig { name: "aignermax/Lunima".into(), coder: true, routing: Routing::KimiOnly }], ..Default::default() };
        c.product_owner.repo = "aignermax/Lunima".into();
        c
    }

    #[test]
    fn lists_configured_discovered_and_po_repos_once() {
        let list = list(&cfg(), &["Akhetonics/khepri".into(), "AIGNERMAX/lunima".into()]);
        assert_eq!(list.len(), 2);
        let lunima = list.iter().find(|p| p.repo == "aignermax/Lunima").unwrap();
        assert!(lunima.configured && lunima.coder && lunima.po && lunima.discovered);
        assert_eq!(lunima.routing, Routing::KimiOnly);
        let khepri = list.iter().find(|p| p.repo == "Akhetonics/khepri").unwrap();
        assert!(!khepri.configured && khepri.coder && khepri.discovered);
    }

    #[test]
    fn edits_create_entries_and_survive_a_save() {
        let mut c = cfg();
        set_coder(&mut c, "Akhetonics/khepri", false);
        set_routing(&mut c, "aignermax/lunima", Routing::Claude);
        add(&mut c, " o/new ").unwrap();
        assert!(add(&mut c, "nope").is_err());
        assert_eq!(c.repos.len(), 3);
        assert!(!c.repo("akhetonics/khepri").unwrap().coder);
        assert_eq!(c.repo("aignermax/Lunima").unwrap().routing, Routing::Claude);
        let t = tempfile::tempdir().unwrap();
        c.save(&t.path().join("team.toml")).unwrap();
        let back = TeamConfig::load(&t.path().join("team.toml")).unwrap();
        assert_eq!(back.repos, c.repos);
        remove(&mut c, "O/NEW");
        assert!(c.repo("o/new").is_none());
    }

    #[test]
    fn agents_enabled_parsing() {
        assert_eq!(parse_agents_enabled("agents_enabled = [\"coder\", \"qa\"] # x"), ["coder", "qa"]);
        assert_eq!(parse_agents_enabled("agents_enabled = [\n  'qa', # c\n  'pr-feedback'\n]"), ["qa", "pr-feedback"]);
        assert_eq!(parse_agents_enabled(""), ["coder"]);
    }
}
