//! Per-run Codex CI merge gate and commit-comment review (Codex CLI 0.169.0+).
//!
//! Requires Node.js and Codex on PATH. The caller adds each returned value with
//! `-c` and registers this module in `lib.rs`. Do not pass the broad hook-trust
//! bypass: the overrides trust only this embedded definition for this run.
//! No global configuration or installed asset files are changed.

/// Returns TOML CLI overrides enabling an independent, read-only Codex comment review.
///
/// The embedded hook invokes the event's active model. Review failure and findings
/// become blocking PostToolUse feedback; they cannot undo an already-created commit.
/// Existing hooks in other config layers still run, per Codex's merge semantics.
pub fn config_overrides() -> Vec<String> {
    let mut overrides = vec!["features.hooks=true".to_owned()];
    let mut states = Vec::new();
    for (event, event_key, payload, trust_json, timeout, status) in [
        (
            "PostToolUse",
            "post_tool_use",
            include_str!("../assets/codex-comment-hook.b64"),
            include_str!("../assets/codex-hook-trust.json"),
            300,
            "Checking commit comments with Codex",
        ),
        (
            "PreToolUse",
            "pre_tool_use",
            include_str!("../assets/codex-ci-gate.b64"),
            include_str!("../assets/codex-ci-gate-trust.json"),
            30,
            "Checking PR CI before merge",
        ),
    ] {
        let payload = payload.trim();
        let command = format!(
        "node -e \"eval(require('node:zlib').gunzipSync(Buffer.from('{payload}','base64')).toString())\""
    );
        let quoted = toml::Value::String(command).to_string();
        let trust: serde_json::Value = serde_json::from_str(trust_json).unwrap();
        // Codex 0.169.0's synthetic_layer_path explicitly anchors Windows at C:\,
        // independently of the executable, session cwd, or system drive.
        let key = if cfg!(windows) {
            r"C:\<session-flags>\config.toml"
        } else {
            "/<session-flags>/config.toml"
        };
        let key = toml::Value::String(format!("{key}:{event_key}:0:0")).to_string();
        let hash = toml::Value::String(trust["hash"].as_str().unwrap().to_owned()).to_string();
        overrides.push(format!(
        "hooks.{event}=[{{matcher='^Bash$',hooks=[{{type='command',command={quoted},timeout={timeout},statusMessage='{status}'}}]}}]"
    ));
        states.push(format!("{key}={{enabled=true,trusted_hash={hash}}}"));
    }
    overrides.push(format!("hooks.state={{{}}}", states.join(",")));
    overrides
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_are_inline_toml_and_fit_windows_command_limit() {
        let overrides = config_overrides();
        let config: toml::Value = overrides.join("\n").parse().unwrap();
        assert_eq!(config["features"]["hooks"].as_bool(), Some(true));
        let group = &config["hooks"]["PostToolUse"][0];
        assert_eq!(group["matcher"].as_str(), Some("^Bash$"));
        let hook = &group["hooks"][0];
        assert_eq!(hook["type"].as_str(), Some("command"));
        assert_eq!(hook["timeout"].as_integer(), Some(300));
        assert!(hook["command"].as_str().unwrap().len() < 8000);
        let gate = &config["hooks"]["PreToolUse"][0]["hooks"][0];
        assert_eq!(gate["timeout"].as_integer(), Some(30));
        assert!(gate["command"].as_str().unwrap().len() < 8000);
        assert_eq!(config["hooks"]["state"].as_table().unwrap().len(), 2);
    }

    #[test]
    #[ignore = "Requires Node.js; run explicitly when validating Codex hook assets"]
    fn embedded_payload_matches_reviewed_source() {
        for (payload, source) in [
            (
                include_str!("../assets/codex-comment-hook.b64"),
                include_bytes!("../assets/codex-comment-hook.cjs").as_slice(),
            ),
            (
                include_str!("../assets/codex-ci-gate.b64"),
                include_bytes!("../assets/codex-ci-gate.cjs").as_slice(),
            ),
        ] {
            let output = std::process::Command::new("node")
            .args(["-e", "process.stdout.write(require('node:zlib').gunzipSync(Buffer.from(process.argv[1],'base64')))"])
            .arg(payload.trim())
            .output()
            .expect("Node.js is required for Codex hook tests");
            assert!(output.status.success());
            assert_eq!(output.stdout, source);
        }
    }
}
