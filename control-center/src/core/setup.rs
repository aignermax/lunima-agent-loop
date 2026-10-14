//! Setup assistant: what this machine needs before the team agent can work, and the
//! one step that fixes each gap. Never asks for tokens — Claude and GitHub use the
//! user's own logins.

/// Answers about the machine; `sys::probes::System` asks the real tools.
pub trait Probes {
    /// `<tool> --version`, first line
    fn version(&self, tool: &str) -> Result<String, String>;
    /// `gh auth status`
    fn gh_login(&self) -> Result<(), String>;
    /// Claude CLI found (path)
    fn claude_cli(&self) -> Option<String>;
    /// Claude logged in (subscription or key)
    fn claude_login(&self) -> Result<bool, String>;
}

/// What the "fix" button of a check does.
#[derive(Debug, Clone, PartialEq)]
pub enum SetupAction {
    /// open a terminal running this command
    Terminal(&'static str),
    /// open a download page
    Open(&'static str),
    /// import legacy settings and write team.toml
    CreateConfig,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SetupCheck {
    pub title: &'static str,
    pub ok: bool,
    /// a missing optional tool is only a hint
    pub optional: bool,
    pub detail: String,
    pub action: Option<(&'static str, SetupAction)>,
}

fn check(title: &'static str, result: Result<String, String>, optional: bool, action: Option<(&'static str, SetupAction)>) -> SetupCheck {
    match result {
        Ok(detail) => SetupCheck { title, ok: true, optional, detail, action: None },
        Err(detail) => SetupCheck { title, ok: false, optional, detail, action },
    }
}

/// All checks in the order a new machine has to work through them.
pub fn evaluate(p: &dyn Probes, config_exists: bool) -> Vec<SetupCheck> {
    let git = check("Git", p.version("git"), false, Some(("Herunterladen", SetupAction::Open("https://git-scm.com/downloads"))));
    let gh_installed = p.version("gh");
    let gh_ok = gh_installed.is_ok();
    let gh = check("GitHub CLI (gh)", gh_installed, false, Some(("Herunterladen", SetupAction::Open("https://cli.github.com/"))));
    let gh_login = if gh_ok {
        check("GitHub-Anmeldung", p.gh_login().map(|_| "angemeldet".into()), false, Some(("Im Terminal anmelden", SetupAction::Terminal("gh auth login"))))
    } else {
        check("GitHub-Anmeldung", Err("erst gh installieren".into()), false, None)
    };
    let cli = p.claude_cli();
    let claude = check("Claude Code CLI", cli.clone().ok_or_else(|| "claude nicht im PATH".to_string()), false, Some(("Anleitung öffnen", SetupAction::Open("https://docs.claude.com/en/docs/claude-code/setup"))));
    let login = match (cli.is_some(), p.claude_login()) {
        (false, _) => Err("erst die Claude-CLI installieren".to_string()),
        (true, Ok(true)) => Ok("angemeldet (Abo)".to_string()),
        (true, Ok(false)) => Err("nicht angemeldet".to_string()),
        (true, Err(e)) => Err(e),
    };
    let claude_login = check("Claude-Anmeldung", login, false, Some(("Im Terminal anmelden", SetupAction::Terminal("claude auth login"))));
    let dotnet = check(".NET SDK (für Lunima-Builds)", p.version("dotnet"), true, Some(("Herunterladen", SetupAction::Open("https://dotnet.microsoft.com/download"))));
    let config = check(
        "Konfiguration (team.toml)",
        if config_exists { Ok("vorhanden".into()) } else { Err("fehlt — wird aus alten Einstellungen übernommen, falls vorhanden".into()) },
        false,
        Some(("Konfiguration anlegen", SetupAction::CreateConfig)),
    );
    vec![git, gh, gh_login, claude, claude_login, dotnet, config]
}

/// Writes team.toml from the legacy files (Python agent `.env`, PO loop
/// `agent-loop.json`) — never over an existing one. Secrets are not imported.
pub fn create_config(paths: &team_core::paths::Paths, legacy: (Option<std::path::PathBuf>, Option<std::path::PathBuf>)) -> Result<String, String> {
    if paths.config().is_file() {
        return Err("team.toml existiert bereits — nichts geändert".into());
    }
    let (env, lp) = legacy;
    let cfg = team_core::import::from_legacy(env.as_deref(), lp.as_deref())?;
    paths.ensure().map_err(|e| e.to_string())?;
    cfg.save(&paths.config())?;
    let from: Vec<String> = [env, lp].into_iter().flatten().map(|p| p.display().to_string()).collect();
    Ok(if from.is_empty() { "team.toml mit Standardwerten angelegt".into() } else { format!("team.toml angelegt (übernommen aus {})", from.join(", ")) })
}

/// Everything required is in place.
pub fn ready(checks: &[SetupCheck]) -> bool {
    checks.iter().all(|c| c.ok || c.optional)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        missing: &'static [&'static str],
        gh_login: bool,
        claude: bool,
        claude_login: bool,
    }

    impl Probes for Fake {
        fn version(&self, tool: &str) -> Result<String, String> {
            if self.missing.contains(&tool) { Err("nicht gefunden".into()) } else { Ok(format!("{tool} 1.0")) }
        }
        fn gh_login(&self) -> Result<(), String> {
            if self.gh_login { Ok(()) } else { Err("not logged in".into()) }
        }
        fn claude_cli(&self) -> Option<String> {
            self.claude.then(|| "claude.exe".into())
        }
        fn claude_login(&self) -> Result<bool, String> {
            Ok(self.claude_login)
        }
    }

    #[test]
    fn complete_machine_is_ready_even_without_dotnet() {
        let checks = evaluate(&Fake { missing: &["dotnet"], gh_login: true, claude: true, claude_login: true }, true);
        assert!(ready(&checks));
        assert!(!checks.iter().find(|c| c.title.starts_with(".NET")).unwrap().ok);
    }

    #[test]
    fn each_gap_offers_its_fix() {
        let checks = evaluate(&Fake { missing: &[], gh_login: false, claude: true, claude_login: false }, false);
        assert!(!ready(&checks));
        let fix = |title: &str| checks.iter().find(|c| c.title == title).and_then(|c| c.action.clone()).map(|(_, a)| a);
        assert_eq!(fix("GitHub-Anmeldung"), Some(SetupAction::Terminal("gh auth login")));
        assert_eq!(fix("Claude-Anmeldung"), Some(SetupAction::Terminal("claude auth login")));
        assert_eq!(fix("Konfiguration (team.toml)"), Some(SetupAction::CreateConfig));
    }

    #[test]
    fn config_is_created_from_legacy_files_once() {
        let t = tempfile::tempdir().unwrap();
        let paths = team_core::paths::Paths::new(t.path().join("home"));
        let env = t.path().join(".env");
        std::fs::write(&env, "AGENT_REPOS=o/r
GITHUB_TOKEN=secret
").unwrap();
        let msg = create_config(&paths, (Some(env.clone()), None)).unwrap();
        assert!(msg.contains(".env"));
        let text = std::fs::read_to_string(paths.config()).unwrap();
        assert!(text.contains("o/r") && !text.contains("secret"));
        assert!(create_config(&paths, (Some(env), None)).is_err(), "never overwrites");
    }

    #[test]
    fn missing_tools_block_their_logins() {
        let checks = evaluate(&Fake { missing: &["gh"], gh_login: true, claude: false, claude_login: true }, true);
        let login = checks.iter().find(|c| c.title == "GitHub-Anmeldung").unwrap();
        assert!(!login.ok && login.action.is_none());
        assert!(!checks.iter().find(|c| c.title == "Claude-Anmeldung").unwrap().ok);
    }
}
