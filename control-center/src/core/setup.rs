//! Setup assistant: what this machine needs before the team agent can work, and the
//! one step that fixes each gap. Never asks for tokens — Claude and GitHub use the
//! user's own logins.

use team_core::config::Backend;

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
    fn codex_cli(&self) -> Option<String>;
    fn codex_login(&self) -> Result<bool, String>;
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

/// `path` with the user's own folder shown as `~`: the page ends up in screenshots and
/// support requests, and the user name doesn't help anyone there.
pub fn without_home(path: &str, home: Option<&str>) -> String {
    match home.filter(|h| !h.is_empty()) {
        Some(h) if path.get(..h.len()).is_some_and(|p| p.eq_ignore_ascii_case(h)) && matches!(path.as_bytes().get(h.len()), Some(b'\\' | b'/')) => {
            format!("~{}", &path[h.len()..])
        }
        _ => path.to_string(),
    }
}

/// All checks in the order a new machine has to work through them.
pub fn evaluate(p: &dyn Probes, config_exists: bool, backend: Backend, api_ready: Result<(), String>) -> Vec<SetupCheck> {
    let git = check("Git", p.version("git"), false, Some(("Herunterladen", SetupAction::Open("https://git-scm.com/downloads"))));
    let gh_installed = p.version("gh");
    let gh_ok = gh_installed.is_ok();
    let gh = check("GitHub CLI (gh)", gh_installed, false, Some(("Herunterladen", SetupAction::Open("https://cli.github.com/"))));
    let gh_login = if gh_ok { check("GitHub-Anmeldung", p.gh_login().map(|_| "angemeldet".into()), false, Some(("Im Terminal anmelden", SetupAction::Terminal("gh auth login")))) } else { check("GitHub-Anmeldung", Err("erst gh installieren".into()), false, None) };
    let (cli, title, login_title, install_url, login_command) = match backend {
        Backend::Claude | Backend::Openrouter | Backend::Eco => (p.claude_cli(), "Claude Code CLI", "Claude-Anmeldung", "https://docs.claude.com/en/docs/claude-code/setup", "claude auth login"),
        Backend::Codex | Backend::OpenAi => (p.codex_cli(), "Codex CLI", "Codex-Anmeldung", "https://developers.openai.com/codex/cli", "codex login"),
    };
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).ok();
    let shown = cli.as_deref().map(|c| without_home(c, home.as_deref()));
    let claude = check(title, shown.ok_or_else(|| format!("{} nicht im PATH", backend.label())), false, Some(("Anleitung öffnen", SetupAction::Open(install_url))));
    let login_status = if cli.is_none() {
        Ok(false)
    } else {
        match backend {
            Backend::Claude => p.claude_login(),
            Backend::Codex => p.codex_login(),
            Backend::Openrouter | Backend::Eco | Backend::OpenAi => Ok(true),
        }
    };
    let login = match (cli.is_some(), login_status) {
        (false, _) => Err(format!("erst die {}-CLI installieren", backend.label())),
        (true, Ok(true)) => Ok(if backend == Backend::Codex { "angemeldet (ChatGPT)" } else { "angemeldet (Abo)" }.to_string()),
        (true, Ok(false)) => Err("nicht angemeldet".to_string()),
        (true, Err(e)) => Err(e),
    };
    let claude_login = if matches!(backend, Backend::Openrouter | Backend::Eco | Backend::OpenAi) { check("API-Provider", api_ready.map(|_| format!("{}: API-Key konfiguriert (nicht online geprüft)", backend.label())), false, None) } else { check(login_title, login, false, Some(("Im Terminal anmelden", SetupAction::Terminal(login_command)))) };
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
        fn codex_cli(&self) -> Option<String> {
            Some("codex.exe".into())
        }
        fn codex_login(&self) -> Result<bool, String> {
            Ok(true)
        }
    }

    #[test]
    fn api_setup_checks_keys_instead_of_subscription_login() {
        for backend in [Backend::Openrouter, Backend::Eco, Backend::OpenAi] {
            let fake = Fake { missing: &[], gh_login: true, claude: true, claude_login: false };
            assert!(ready(&evaluate(&fake, true, backend, Ok(()))));
            let checks = evaluate(&fake, true, backend, Err("API-Key fehlt".into()));
            assert!(!ready(&checks));
            assert!(checks.iter().any(|c| c.title == "API-Provider" && !c.ok));
            assert!(!checks.iter().any(|c| c.title.ends_with("-Anmeldung") && c.title != "GitHub-Anmeldung"));
        }
    }

    #[test]
    fn codex_setup_does_not_require_claude() {
        let checks = evaluate(&Fake { missing: &[], gh_login: true, claude: false, claude_login: false }, true, Backend::Codex, Ok(()));
        assert!(ready(&checks));
        assert!(checks.iter().any(|c| c.title == "Codex-Anmeldung" && c.ok && c.detail.contains("ChatGPT")));
        assert!(!checks.iter().any(|c| c.title.contains("Claude")));
    }

    #[test]
    fn complete_machine_is_ready_even_without_dotnet() {
        let checks = evaluate(&Fake { missing: &["dotnet"], gh_login: true, claude: true, claude_login: true }, true, Backend::Claude, Ok(()));
        assert!(ready(&checks));
        assert!(!checks.iter().find(|c| c.title.starts_with(".NET")).unwrap().ok);
    }

    #[test]
    fn each_gap_offers_its_fix() {
        let checks = evaluate(&Fake { missing: &[], gh_login: false, claude: true, claude_login: false }, false, Backend::Claude, Ok(()));
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
        let checks = evaluate(&Fake { missing: &["gh"], gh_login: true, claude: false, claude_login: true }, true, Backend::Claude, Ok(()));
        let login = checks.iter().find(|c| c.title == "GitHub-Anmeldung").unwrap();
        assert!(!login.ok && login.action.is_none());
        assert!(!checks.iter().find(|c| c.title == "Claude-Anmeldung").unwrap().ok);
    }

    #[test]
    fn paths_hide_the_user_folder() {
        let home = Some(r"C:\Users\max_a");
        assert_eq!(without_home(r"C:\Users\max_a\AppData\Roaming\npm\claude.cmd", home), r"~\AppData\Roaming\npm\claude.cmd");
        assert_eq!(without_home(r"c:\users\MAX_A\x.exe", home), r"~\x.exe", "Windows paths ignore case");
        assert_eq!(without_home(r"C:\Users\max_ab\x.exe", home), r"C:\Users\max_ab\x.exe", "another user's folder");
        assert_eq!(without_home("/home/max/.local/bin/claude", Some("/home/max")), "~/.local/bin/claude");
        assert_eq!(without_home("claude.exe", None), "claude.exe");
        assert_eq!(without_home(r"C:\Üser\x", Some(r"C:\Ü")), r"C:\Üser\x", "no panic inside a multi-byte character");
    }
}
