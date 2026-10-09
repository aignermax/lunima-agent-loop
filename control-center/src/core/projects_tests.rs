use super::*;

fn env(text: &str) -> (tempfile::TempDir, EnvFile) {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join(".env");
    std::fs::write(&p, text).unwrap();
    let e = EnvFile::load(&p).unwrap();
    (tmp, e)
}

#[test]
fn lists_every_known_repo_once_with_its_routing() {
    let (_t, e) = env("AGENT_REPOS=aignermax/Lunima\nAGENT_OPENROUTER_REPOS=aignermax/Lunima\nAGENT_OPENROUTER_FORCE_REPOS=AignerMax/Lunima\n");
    let projects = list(&e, &["Akhetonics/khepri".to_string()], "aignermax/Lunima");
    assert_eq!(projects.len(), 2);
    let lunima = projects.iter().find(|p| p.repo.eq_ignore_ascii_case("aignermax/Lunima")).unwrap();
    assert!(lunima.manual && lunima.po && lunima.handled());
    assert_eq!(lunima.routing, Routing::KimiForced);
    let khepri = projects.iter().find(|p| p.repo == "Akhetonics/khepri").unwrap();
    assert!(khepri.discovered && !khepri.manual && khepri.handled());
    assert_eq!(khepri.routing, Routing::Claude);
}

#[test]
fn routing_edits_both_lists() {
    let (_t, mut e) = env("AGENT_OPENROUTER_REPOS=a/b,c/d\nAGENT_OPENROUTER_FORCE_REPOS=a/b\n");
    set_routing(&mut e, "A/B", Routing::Claude);
    assert_eq!(e.list(KIMI_KEY), ["c/d"]);
    assert!(e.list(FORCE_KEY).is_empty());
    set_routing(&mut e, "x/y", Routing::KimiForced);
    assert_eq!(e.list(KIMI_KEY), ["c/d", "x/y"]);
    assert_eq!(e.list(FORCE_KEY), ["x/y"]);
}

#[test]
fn adding_and_removing_manual_repos() {
    let (_t, mut e) = env("AGENT_REPOS=a/b\n");
    set_listed(&mut e, REPOS_KEY, "c/d", true);
    set_listed(&mut e, REPOS_KEY, "A/B", false);
    assert_eq!(e.list(REPOS_KEY), ["c/d"]);
}

#[test]
fn reads_agents_enabled_from_agent_toml() {
    let toml = "build_cmd = \"dotnet build\"\nagents_enabled = [\"coder\", \"reviewer\", 'qa']\n";
    assert_eq!(parse_agents_enabled(toml), ["coder", "reviewer", "qa"]);
    assert_eq!(parse_agents_enabled("build_cmd = \"x\""), ["coder"]);
}

#[test]
fn discovered_registry_keys() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".sessions")).unwrap();
    std::fs::write(tmp.path().join(".sessions/discovered-repos.json"), r#"{"Akhetonics/khepri": {"first_seen": 1}}"#).unwrap();
    assert_eq!(read_discovered(tmp.path()), ["Akhetonics/khepri"]);
}
