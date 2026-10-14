//! Visual walkthrough: screenshots the coder rendered to `artifacts/ui-screenshots/issue-N/`
//! are committed to `docs/pr-media/issue-N/` and embedded in the PR body (blob URLs with
//! `?raw=true` — the only form that renders in private repos; pinned to the commit SHA).

use regex::Regex;
use serde_json::Value;
use std::path::Path;
use std::sync::OnceLock;
use team_core::git::run_git;

pub const START: &str = "<!-- pr-walkthrough:start -->";
pub const END: &str = "<!-- pr-walkthrough:end -->";

/// (file, caption) in display order.
pub fn steps(dir: &Path) -> Vec<(String, String)> {
    let mut pngs: Vec<String> = std::fs::read_dir(dir)
        .map(|it| it.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.to_lowercase().ends_with(".png")).collect())
        .unwrap_or_default();
    pngs.sort();
    let manifest: Option<Value> = std::fs::read_to_string(dir.join("manifest.json")).ok().and_then(|t| serde_json::from_str(&t).ok());
    let mut out: Vec<(String, String)> = Vec::new();
    let push = |file: &str, caption: &str, out: &mut Vec<(String, String)>| {
        if pngs.iter().any(|p| p == file) && !out.iter().any(|(f, _)| f == file) {
            out.push((file.to_string(), caption.trim().to_string()));
        }
    };
    match manifest {
        Some(Value::Array(items)) => {
            for it in items {
                if let Some(f) = it.get("file").and_then(Value::as_str) {
                    push(f, it.get("caption").and_then(Value::as_str).unwrap_or(""), &mut out);
                }
            }
        }
        Some(Value::Object(map)) => {
            let mut keys: Vec<_> = map.keys().cloned().collect();
            keys.sort();
            for k in keys {
                push(&k, map[&k].as_str().unwrap_or(""), &mut out);
            }
        }
        _ => {}
    }
    for p in &pngs {
        if !out.iter().any(|(f, _)| f == p) {
            out.push((p.clone(), String::new()));
        }
    }
    out
}

pub fn markdown(repo: &str, sha: &str, issue: u64, steps: &[(String, String)]) -> String {
    if steps.is_empty() {
        return String::new();
    }
    let mut md = format!("\n{START}\n\n## 📸 Visual walkthrough\n_Step-by-step of the user flow, rendered headlessly. Review here — no checkout needed._\n");
    for (i, (file, caption)) in steps.iter().enumerate() {
        let n = i + 1;
        let title = if caption.is_empty() { file.as_str() } else { caption.as_str() };
        md.push_str(&format!("\n**Step {n} — {title}**\n\n![step {n}](https://github.com/{repo}/blob/{sha}/docs/pr-media/issue-{issue}/{file}?raw=true)\n"));
    }
    md.push_str(&format!("\n{END}\n"));
    md
}

/// Replaces an existing walkthrough section, else appends; empty walkthrough → unchanged.
pub fn merge_into_body(body: &str, walkthrough: &str) -> String {
    if walkthrough.trim().is_empty() {
        return body.to_string();
    }
    static R: OnceLock<Regex> = OnceLock::new();
    let re = R.get_or_init(|| Regex::new(&format!("(?s){}.*?{}", regex::escape(START), regex::escape(END))).expect("regex"));
    if re.is_match(body) {
        return re.replace(body, regex::NoExpand(walkthrough.trim_matches('\n'))).into_owned();
    }
    format!("{}\n{walkthrough}", body.trim_end_matches('\n'))
}

/// Moves the screenshots into `docs/pr-media/issue-N/`, commits and pushes them on `branch`.
/// Never fails: any problem → "" (the PR just has no walkthrough).
pub fn publish(worktree: &Path, repo: &str, branch: &str, issue: u64) -> String {
    match try_publish(worktree, repo, branch, issue) {
        Ok(md) => md,
        Err(e) => {
            crate::log::warn(&format!("Walkthrough für #{issue} nicht veröffentlicht: {e}"));
            String::new()
        }
    }
}

fn try_publish(worktree: &Path, repo: &str, branch: &str, issue: u64) -> Result<String, String> {
    let src = worktree.join("artifacts").join("ui-screenshots").join(format!("issue-{issue}"));
    let list = steps(&src);
    if list.is_empty() || !repo.contains('/') {
        return Ok(String::new());
    }
    let rel = format!("docs/pr-media/issue-{issue}");
    let dst = worktree.join("docs").join("pr-media").join(format!("issue-{issue}"));
    std::fs::create_dir_all(&dst).map_err(|e| e.to_string())?;
    for (file, _) in &list {
        std::fs::copy(src.join(file), dst.join(file)).map_err(|e| e.to_string())?;
    }
    if src.join("manifest.json").is_file() {
        let _ = std::fs::copy(src.join("manifest.json"), dst.join("manifest.json"));
    }
    let g = |args: &[&str]| run_git(Some(worktree), args, false);
    g(&["add", "--", &rel])?;
    let _ = g(&["rm", "-r", "-f", "-q", "--ignore-unmatch", "--", "artifacts/ui-screenshots"]);
    let _ = std::fs::remove_dir_all(worktree.join("artifacts").join("ui-screenshots"));
    if !g(&["status", "--porcelain", "--", "docs/pr-media", "artifacts"])?.trim().is_empty() {
        g(&["-c", "user.name=lunima-team-agent", "-c", "user.email=agent@lunima.invalid", "commit", "-q", "-m", &format!("docs(pr-media): visual walkthrough for #{issue}"), "--", &rel, "artifacts"])?;
        run_git(Some(worktree), &["push", "-q", "origin", branch], true)?;
    }
    let sha = g(&["rev-parse", "HEAD"]).map(|s| s.trim().to_string()).unwrap_or_else(|_| branch.to_string());
    Ok(markdown(repo, &sha, issue, &list))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), b"png").unwrap();
    }

    #[test]
    fn manifest_orders_steps_and_extra_files_follow() {
        let t = tempfile::tempdir().unwrap();
        for f in ["01-a.png", "02-b.png", "03-c.png"] {
            png(t.path(), f);
        }
        std::fs::write(t.path().join("manifest.json"), r#"[{"file":"02-b.png","caption":"B"},{"file":"missing.png","caption":"x"},{"file":"01-a.png","caption":"A"}]"#).unwrap();
        let s = steps(t.path());
        assert_eq!(s, vec![("02-b.png".into(), "B".into()), ("01-a.png".into(), "A".into()), ("03-c.png".into(), "".into())]);
        std::fs::write(t.path().join("manifest.json"), "{bad").unwrap();
        assert_eq!(steps(t.path())[0].0, "01-a.png");
    }

    #[test]
    fn markdown_uses_raw_blob_urls_and_markers() {
        let md = markdown("o/r", "abc", 5, &[("01.png".into(), "".into())]);
        assert!(md.contains(START) && md.contains(END));
        assert!(md.contains("https://github.com/o/r/blob/abc/docs/pr-media/issue-5/01.png?raw=true"));
        assert!(md.contains("**Step 1 — 01.png**"));
        assert!(markdown("o/r", "abc", 5, &[]).is_empty());
    }

    #[test]
    fn merge_appends_then_replaces() {
        let first = markdown("o/r", "a1", 5, &[("01.png".into(), "one".into())]);
        let body = merge_into_body("Body\n", &first);
        assert!(body.starts_with("Body\n\n") && body.contains("a1"));
        let second = markdown("o/r", "b2$1", 5, &[("01.png".into(), "two".into())]);
        let body = merge_into_body(&body, &second);
        assert!(body.contains("b2$1") && !body.contains("a1"));
        assert_eq!(body.matches(START).count(), 1);
        assert_eq!(merge_into_body("x", ""), "x");
    }
}
