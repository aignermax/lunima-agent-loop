//! Codex CLI sessions using the same watchdog and outcome contract as Claude.

use crate::claude::{Outcome, Session};
use crate::proc::which;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

mod context;

pub fn find_cli() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CODEX_CLI_PATH").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(path);
    }
    let path = which("codex")?;
    if cfg!(windows) && path.extension().is_some_and(|e| e == "cmd") {
        let npm = path.parent()?.join("node_modules/@openai/codex/node_modules/@openai");
        let (package, target) = if cfg!(target_arch = "aarch64") {
            ("codex-win32-arm64", "aarch64-pc-windows-msvc")
        } else {
            ("codex-win32-x64", "x86_64-pc-windows-msvc")
        };
        let native = npm.join(package).join("vendor").join(target).join("bin/codex.exe");
        if native.is_file() {
            return Some(native);
        }
    }
    Some(path)
}

pub fn args(s: &Session) -> Vec<String> {
    args_for(s, None)
}

fn args_for(s: &Session, api_url: Option<&str>) -> Vec<String> {
    let mut a: Vec<String> = ["exec", "--ignore-user-config", "--ephemeral", "--skip-git-repo-check", "--json", "--color", "never"]
        .into_iter()
        .map(String::from)
        .collect();
    a.extend(["-c".into(), format!("cli_auth_credentials_store={}", credential_store(s))]);
    for setting in [
        "approval_policy=\"never\"",
        "model_provider=\"openai\"",
        "mcp_servers={}",
        "features.apps=false",
        "features.plugins=false",
        "features.browser_use=false",
        "features.computer_use=false",
        "features.multi_agent=false",
        "features.multi_agent_v2=false",
        "web_search=\"live\"",
    ] {
        a.extend(["-c".into(), setting.into()]);
    }
    if let Some(url) = api_url {
        let url = toml::Value::String(url.to_string()).to_string();
        for setting in [
            "model_provider=\"lunima_api\"".into(),
            "model_providers.lunima_api.name=\"Lunima API\"".into(),
            format!("model_providers.lunima_api.base_url={url}"),
            "model_providers.lunima_api.env_key=\"LUNIMA_CODEX_API_KEY\"".into(),
            "model_providers.lunima_api.wire_api=\"responses\"".into(),
            "model_providers.lunima_api.requires_openai_auth=false".into(),
        ] {
            a.extend(["-c".into(), setting]);
        }
    }
    if s.tools.is_some() {
        a.extend(["--sandbox".into(), "workspace-write".into()]);
        if cfg!(windows) {
            a.extend(["-c".into(), "windows.sandbox=\"unelevated\"".into()]);
        }
        for setting in [
            "features.shell_tool=false",
            "features.unified_exec=false",
            "features.code_mode=false",
            "features.code_mode_only=false",
            "features.hooks=false",
        ] {
            a.extend(["-c".into(), setting.into()]);
        }
    } else {
        a.push("--dangerously-bypass-approvals-and-sandbox".into());
        for setting in crate::codex_hooks::config_overrides() {
            a.extend(["-c".into(), setting]);
        }
        if which("codegraph").is_some() {
            for setting in codegraph_mcp() {
                a.extend(["-c".into(), setting]);
            }
        }
    }
    if !s.model.is_empty() {
        a.extend(["--model".into(), s.model.clone()]);
    }
    if let Some(effort) = s.env.set.get("LUNIMA_CODEX_REASONING_EFFORT").filter(|e| matches!(e.as_str(), "low" | "medium" | "high" | "xhigh" | "max" | "ultra")) {
        a.extend(["-c".into(), format!("model_reasoning_effort=\"{effort}\"")]);
    }
    a.push("-".into());
    a
}

/// The code-navigation tools of the CodeGraph index the coder prepares in each worktree.
const CODEGRAPH_TOOLS: [&str; 9] = [
    "codegraph_explore",
    "codegraph_search",
    "codegraph_context",
    "codegraph_node",
    "codegraph_callers",
    "codegraph_callees",
    "codegraph_impact",
    "codegraph_files",
    "codegraph_status",
];

/// The one MCP server a full session gets on top of `mcp_servers={}`: CodeGraph, read only
/// over the worktree's own index. Personal connectors stay off. Without the approvals,
/// `approval_policy="never"` would refuse every call.
fn codegraph_mcp() -> Vec<String> {
    let mut settings = vec!["mcp_servers.codegraph.command=\"codegraph\"".to_string(), "mcp_servers.codegraph.args=[\"serve\", \"--mcp\"]".to_string()];
    settings.extend(CODEGRAPH_TOOLS.iter().map(|t| format!("mcp_servers.codegraph.tools.{t}.approval_mode=\"approve\"")));
    settings
}

pub fn run(s: &Session) -> Result<Outcome, String> {
    run_with_api(s, None)
}

/// An explicitly configured Responses-compatible endpoint, never the ChatGPT subscription.
pub fn run_api(s: &Session) -> Result<Outcome, String> {
    let url = s.env.set.get("LUNIMA_CODEX_API_BASE_URL").filter(|v| !v.trim().is_empty()).ok_or("OpenAI-API: Endpoint fehlt")?;
    if !s.env.set.get("LUNIMA_CODEX_API_KEY").is_some_and(|v| !v.trim().is_empty()) {
        return Err("OpenAI-API: API-Key fehlt; kein Ausweichen auf ein Abo".into());
    }
    if s.model.trim().is_empty() {
        return Err("OpenAI-API: Modell fehlt".into());
    }
    run_with_api(s, Some(url))
}

fn run_with_api(s: &Session, api_url: Option<&str>) -> Result<Outcome, String> {
    let cli = find_cli().ok_or("Codex-CLI nicht gefunden (codex nicht im PATH)")?;
    let mut session = s.clone();
    session.env.set.insert("LUNIMA_CODEX_AUTH_STORE".into(), credential_store(s));
    session.env.set.insert("LUNIMA_CODEX_MODEL".into(), s.model.clone());
    session.env.set.insert("LUNIMA_CODEX_EXECUTABLE".into(), cli.display().to_string());
    session
        .env
        .set
        .retain(|k, _| !k.starts_with("ANTHROPIC_") && !matches!(k.as_str(), "OPENAI_API_KEY" | "OPENAI_BASE_URL" | "CODEX_API_KEY"));
    session.env.remove.extend(["OPENAI_API_KEY", "OPENAI_BASE_URL", "CODEX_API_KEY"].into_iter().map(String::from));
    if api_url.is_none() {
        session.env.set.remove("LUNIMA_CODEX_API_KEY");
        session.env.set.remove("LUNIMA_CODEX_API_BASE_URL");
        session.env.remove.push("LUNIMA_CODEX_API_KEY".into());
        session.env.remove.push("LUNIMA_CODEX_API_BASE_URL".into());
        let mut check = crate::proc::command(&cli);
        check.args(["-c", &format!("cli_auth_credentials_store={}", credential_store(&session)), "login", "status"]);
        session.env.apply(&mut check);
        let status = crate::proc::run(check, None, Duration::from_secs(20)).map_err(|e| e.to_string())?;
        if !status.ok() || !format!("{}{}", status.stdout, status.stderr).contains("Logged in using ChatGPT") {
            return Err("Codex: ChatGPT-Anmeldung fehlt. Bitte `codex login` ausführen; kein Ausweichen auf einen API-Key.".into());
        }
    }
    session.deadline = Some(s.deadline.unwrap_or(Duration::from_secs(2 * 60 * 60)));
    session.prompt = format!(
        "Follow the repository's AGENTS.md instructions. Also read and follow CLAUDE.md when present; its project-specific conventions apply to this session too.\n\n{}",
        session.prompt
    );
    if s.tools.is_some() {
        session.prompt.push_str(&context::snapshot(&s.cwd)?);
    }
    if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
        session.prompt.push_str(&context::global_memory(&PathBuf::from(home))?);
    }
    let mut events = Events::default();
    crate::claude::run_cli(&session, cli, args_for(&session, api_url), |line| events.normalize(line)).map_err(|e| e.to_string().replace("Claude", "Codex"))
}

fn credential_store(s: &Session) -> String {
    if let Some(store) = s.env.set.get("LUNIMA_CODEX_AUTH_STORE").filter(|v| matches!(v.as_str(), "file" | "keyring" | "auto" | "ephemeral")) {
        return store.clone();
    }
    let home = s
        .env
        .set
        .get("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CODEX_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(|p| PathBuf::from(p).join(".codex")));
    home.and_then(|p| std::fs::read_to_string(p.join("config.toml")).ok())
        .and_then(|text| text.parse::<toml::Value>().ok())
        .and_then(|v| v.get("cli_auth_credentials_store").and_then(toml::Value::as_str).map(String::from))
        .filter(|v| matches!(v.as_str(), "file" | "keyring" | "auto" | "ephemeral"))
        .unwrap_or_else(|| "file".into())
}

#[derive(Default)]
struct Events {
    text: String,
    commands: std::collections::HashSet<String>,
}

impl Events {
    fn normalize(&mut self, line: &str) -> Vec<String> {
        let Ok(ev) = serde_json::from_str::<Value>(line) else { return Vec::new() };
        let mut output = Vec::new();
        match ev["type"].as_str().unwrap_or_default() {
            "thread.started" => output.push(json!({"type":"system","subtype":"init","session_id":ev["thread_id"],"model":"Codex"})),
            "item.started" | "item.completed" => {
                let item = &ev["item"];
                let completed = ev["type"] == "item.completed";
                let id = item["id"].as_str().unwrap_or_default();
                match item["type"].as_str().unwrap_or_default() {
                    "agent_message" if completed => self.text = item["text"].as_str().unwrap_or_default().to_string(),
                    "command_execution" => {
                        if self.commands.insert(id.to_string()) {
                            output.push(json!({"type":"assistant","message":{"id":id,"content":[{"type":"tool_use","id":id,"name":"Bash","input":{"command":item["command"]}}]}}));
                        }
                        if completed {
                            output.push(json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":id,"content":item["aggregated_output"],"is_error":item["exit_code"].as_i64() != Some(0)}]}}));
                        }
                    }
                    _ => {}
                }
            }
            "turn.completed" => {
                let usage = &ev["usage"];
                let cached = usage["cached_input_tokens"].as_u64().unwrap_or(0);
                let input = usage["input_tokens"].as_u64().unwrap_or(0).saturating_sub(cached);
                output.push(json!({"type":"result","subtype":"success","is_error":false,"result":self.text,"num_turns":1,"usage":{"input_tokens":input,"cache_read_input_tokens":cached,"output_tokens":usage["output_tokens"]}}));
            }
            "turn.failed" | "error" => {
                let message = ev.pointer("/error/message").or_else(|| ev.get("message")).and_then(Value::as_str).unwrap_or("Codex session failed");
                if ev["type"] == "error" && message.starts_with("Reconnecting...") {
                    return Vec::new();
                }
                output.push(json!({"type":"result","subtype":"error_during_execution","is_error":true,"result":message,"num_turns":1}));
            }
            _ => {}
        }
        output.into_iter().map(|v| v.to_string()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(events: &[Value]) -> Option<crate::claude::Outcome> {
        let mut adapter = Events::default();
        let lines: Vec<_> = events.iter().flat_map(|e| adapter.normalize(&e.to_string())).collect();
        crate::claude::parse_events(&lines, 150)
    }

    #[test]
    fn codex_success_preserves_answer_commands_and_does_not_double_count_cached_tokens() {
        let o = outcome(&[
            json!({"type":"thread.started","thread_id":"thread-1"}),
            json!({"type":"item.started","item":{"id":"c1","type":"command_execution","command":"git status"}}),
            json!({"type":"item.completed","item":{"id":"c1","type":"command_execution","command":"git status","exit_code":0,"aggregated_output":"clean"}}),
            json!({"type":"item.completed","item":{"id":"a1","type":"agent_message","text":"Done"}}),
            json!({"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":80,"output_tokens":15}}),
        ])
        .expect("completed outcome");
        assert_eq!(o.text, "Done");
        assert_eq!(o.session_id, "thread-1");
        assert_eq!(o.bash_commands, ["git status"]);
        assert_eq!(o.usage.total(), 115);
        assert_eq!(o.usage.cache_read, 80);
        assert!(!o.is_error);
    }

    #[test]
    fn codex_failure_and_truncation_are_not_success() {
        let o = outcome(&[json!({"type":"turn.failed","error":{"message":"You've hit your usage limit"}})]).unwrap();
        assert!(o.is_error);
        assert!(o.usage_limit_until.is_some());
        assert!(outcome(&[json!({"type":"item.completed","item":{"type":"agent_message","text":"partial"}})]).is_none());
    }

    fn session(cwd: PathBuf) -> Session<'static> {
        Session {
            prompt: "Reply with exactly PONG. Do not use tools.".into(),
            cwd,
            model: "gpt-6-astra".into(),
            max_turns: 2,
            env: crate::provider::ChildEnv::subscription(),
            settings_file: None,
            log_file: None,
            inactivity: Duration::from_secs(90),
            deadline: Some(Duration::from_secs(180)),
            on_tick: None,
            tools: None,
            memory_guard: false,
            stop: None,
        }
    }

    #[test]
    fn restricted_sessions_keep_shell_and_personal_connectors_unavailable() {
        let mut s = session(".".into());
        s.tools = Some(vec!["Read".into(), "Write".into(), "WebSearch".into()]);
        let a = args(&s);
        assert!(!a.iter().any(|x| x == "--dangerously-bypass-approvals-and-sandbox"));
        assert!(a.windows(2).any(|x| x == ["--sandbox", "workspace-write"]));
        for setting in [
            "features.shell_tool=false",
            "features.multi_agent=false",
            "features.plugins=false",
            "features.apps=false",
            "mcp_servers={}",
            "features.hooks=false",
        ] {
            assert!(a.windows(2).any(|x| x == ["-c", setting]), "{setting}");
        }
    }

    #[test]
    fn full_sessions_get_codegraph_and_nothing_else_restricted_ones_get_no_server() {
        let full = args(&session(".".into()));
        let codegraph: Vec<&String> = full.iter().filter(|x| x.starts_with("mcp_servers.")).collect();
        if which("codegraph").is_some() {
            assert!(full.windows(2).any(|x| x == ["-c", "mcp_servers.codegraph.command=\"codegraph\""]));
            assert!(full.windows(2).any(|x| x == ["-c", "mcp_servers.codegraph.tools.codegraph_explore.approval_mode=\"approve\""]));
            assert!(codegraph.iter().all(|x| x.starts_with("mcp_servers.codegraph.")), "no other server: {codegraph:?}");
            assert_eq!(codegraph.len(), 2 + CODEGRAPH_TOOLS.len());
        } else {
            assert!(codegraph.is_empty(), "no server without the CLI");
        }
        assert!(full.windows(2).any(|x| x == ["-c", "mcp_servers={}"]), "personal connectors still cleared first");
        let mut restricted = session(".".into());
        restricted.tools = Some(vec!["Read".into()]);
        assert!(!args(&restricted).iter().any(|x| x.starts_with("mcp_servers.")), "a sandboxed session gets no MCP server");
    }

    #[test]
    fn api_routing_keeps_the_secret_out_of_command_line_and_never_falls_back() {
        let mut s = session(".".into());
        assert!(run_api(&s).unwrap_err().contains("Endpoint fehlt"));
        s.env.set.insert("LUNIMA_CODEX_API_BASE_URL".into(), "https://api.example.test/v1".into());
        assert!(run_api(&s).unwrap_err().contains("API-Key fehlt"));
        s.env.set.insert("LUNIMA_CODEX_API_KEY".into(), "test-secret".into());
        let a = args_for(&s, Some("https://api.example.test/v1"));
        assert!(a.windows(2).any(|x| x == ["-c", "model_provider=\"lunima_api\""]));
        assert!(a.windows(2).any(|x| x == ["-c", "model_providers.lunima_api.base_url=\"https://api.example.test/v1\""]));
        assert!(!a.join(" ").contains("test-secret"));
        assert!(!a.iter().any(|v| v.starts_with("forced_login_method=")), "must not invalidate an existing subscription login");
    }

    #[test]
    fn credential_store_survives_ignoring_personal_configuration() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "cli_auth_credentials_store = 'keyring'\n").unwrap();
        let mut s = session(dir.path().into());
        s.env.set.insert("CODEX_HOME".into(), dir.path().display().to_string());
        assert_eq!(credential_store(&s), "keyring");
        assert!(args(&s).windows(2).any(|a| a == ["-c", "cli_auth_credentials_store=keyring"]));
    }

    #[test]
    fn project_reasoning_effort_is_passed_to_codex_as_a_separate_setting() {
        let mut s = session(".".into());
        s.env.set.insert("LUNIMA_CODEX_REASONING_EFFORT".into(), "high".into());
        assert!(args(&s).windows(2).any(|a| a == ["-c", "model_reasoning_effort=\"high\""]));
        assert!(args(&s).windows(2).any(|a| a == ["--model", "gpt-6-astra"]));
    }

    #[test]
    #[ignore = "Uses the installed Codex CLI and ChatGPT subscription"]
    fn live_codex_subscription_session() {
        let dir = tempfile::tempdir().unwrap();
        let s = session(dir.path().into());
        let out = run(&s).unwrap();
        assert!(!out.is_error, "{out:?}");
        assert_eq!(out.text.trim(), "PONG");
        assert!(out.usage.total() > 0);
    }

    #[test]
    #[ignore = "Uses the installed Codex CLI against a local fake API (no paid request)"]
    fn live_codex_api_uses_configured_endpoint_and_key() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let start = std::time::Instant::now();
            loop {
                if let Ok((mut stream, _)) = listener.accept() {
                    stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                    let mut request_bytes = Vec::new();
                    let mut bytes = [0; 32768];
                    loop {
                        let n = stream.read(&mut bytes).unwrap();
                        assert!(n > 0);
                        request_bytes.extend_from_slice(&bytes[..n]);
                        let request = String::from_utf8_lossy(&request_bytes);
                        if let Some(end) = request.find("\r\n\r\n") {
                            let length = request[..end]
                                .lines()
                                .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok()))
                                .unwrap_or(0);
                            if request_bytes.len() >= end + 4 + length {
                                break;
                            }
                        }
                        assert!(request_bytes.len() < 10_000_000);
                    }
                    let request = String::from_utf8_lossy(&request_bytes).to_string();
                    let body = r#"{"error":{"message":"local test rejection","type":"invalid_request_error"}}"#;
                    write!(
                        stream,
                        "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                    .unwrap();
                    return request;
                }
                assert!(start.elapsed() < Duration::from_secs(30), "Codex never contacted configured API");
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let mut s = session(dir.path().into());
        let auth_dir = tempfile::tempdir().unwrap();
        s.env.set.insert("CODEX_HOME".into(), auth_dir.path().display().to_string());
        s.env.set.insert("LUNIMA_CODEX_API_BASE_URL".into(), url);
        s.env.set.insert("LUNIMA_CODEX_API_KEY".into(), "local-fixture-key".into());
        s.deadline = Some(Duration::from_secs(30));
        let result = run_api(&s);
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /v1/responses"), "{request}");
        assert!(request.to_ascii_lowercase().contains("authorization: bearer local-fixture-key"));
        assert!(result.is_err() || result.as_ref().unwrap().is_error, "{result:?}");
    }

    #[test]
    #[ignore = "Uses the installed Codex CLI and ChatGPT subscription"]
    fn live_codex_restricted_workspace_write() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = session(dir.path().into());
        s.tools = Some(vec!["Read".into(), "Write".into(), "WebSearch".into()]);
        s.prompt = "Use apply_patch to create smoke.txt in the working directory containing exactly PONG on one line. Do not run shell commands. Then reply DONE.".into();
        let out = run(&s).unwrap();
        assert!(!out.is_error, "{out:?}");
        assert!(dir.path().join("smoke.txt").is_file(), "{out:?}");
        assert_eq!(std::fs::read_to_string(dir.path().join("smoke.txt")).unwrap().trim(), "PONG");
        assert!(out.bash_commands.is_empty());
    }
}
