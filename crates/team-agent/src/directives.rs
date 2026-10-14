//! Branch directives in issue bodies:
//! * work branch — `branch: feature/x` / `Work on branch: x` / `branch = x` (continue an existing branch)
//! * team branch — `Team branch: team/x`, `base branch = x` or the issue-form heading
//!   `### Team branch\n\nteam/x` (fork from it and target the PR at it)

use regex::Regex;
use std::sync::OnceLock;

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("static regex"))
}

fn clean(raw: &str) -> String {
    raw.trim().replace(['*', '`'], "").trim().to_string()
}

pub fn team_branch(body: Option<&str>) -> Option<String> {
    let body = body.filter(|b| !b.trim().is_empty())?;
    static HEADING: OnceLock<Regex> = OnceLock::new();
    static INLINE: OnceLock<Regex> = OnceLock::new();
    let heading = re(&HEADING, r"(?i)#+\s*\b(?:team|base)\s+branch\s*\n+\s*[`*]*([A-Za-z0-9/_.\-]+)");
    let inline = re(&INLINE, r"(?i)\b(?:team|base)\s+branch\s*[:=]\s*[`*]*([A-Za-z0-9/_.\-]+)");
    let value = heading.captures(body).or_else(|| inline.captures(body)).map(|c| clean(&c[1]))?;
    if value.is_empty() || value.to_lowercase().starts_with("_no") {
        return None;
    }
    if value.starts_with('-') {
        crate::log::warn(&format!("Team-Branch „{value}“ beginnt mit '-' — ignoriert"));
        return None;
    }
    Some(value)
}

pub fn work_branch(body: Option<&str>) -> Option<String> {
    let body = body.filter(|b| !b.trim().is_empty())?;
    static STRIP: OnceLock<Regex> = OnceLock::new();
    static FENCED: OnceLock<Regex> = OnceLock::new();
    static COLON: OnceLock<Regex> = OnceLock::new();
    static EQUALS: OnceLock<Regex> = OnceLock::new();
    let body = re(&STRIP, r"(?i)\b(?:team|base)\s+branch\s*[:=][^\n]*").replace_all(body, "");
    let patterns = [
        re(&FENCED, r"(?is)branch:\s*\n\s*```\s*\n\s*([a-zA-Z0-9/_-]+)"),
        re(&COLON, r"(?is)(?:work\s+on\s+)?branch:\s*([^\s`]+)"),
        re(&EQUALS, r"(?is)(?:use\s+)?branch\s*=\s*(\S+)"),
    ];
    let value = patterns.iter().find_map(|p| p.captures(&body)).map(|c| clean(&c[1]))?;
    // a branch name starting with '-' would be read by git as an option
    if value.is_empty() || value.starts_with('-') {
        return None;
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_branch_forms() {
        assert_eq!(team_branch(Some("### Team branch\n\nteam/photonics\n")).as_deref(), Some("team/photonics"));
        assert_eq!(team_branch(Some("Team branch: `team/x`")).as_deref(), Some("team/x"));
        assert_eq!(team_branch(Some("base branch = **release/1.2**")).as_deref(), Some("release/1.2"));
        assert_eq!(team_branch(Some("### Team branch\n\n_No response_")), None);
        assert_eq!(team_branch(Some("Team branch: -rf")), None);
        assert_eq!(team_branch(Some("rebase branch: x")), None);
        assert_eq!(team_branch(Some("codebase branch: x")), None);
        assert_eq!(team_branch(None), None);
        assert_eq!(team_branch(Some("  ")), None);
    }

    #[test]
    fn work_branch_forms() {
        assert_eq!(work_branch(Some("Work on branch: feature/xyz")).as_deref(), Some("feature/xyz"));
        assert_eq!(work_branch(Some("branch:\n```\nfeature/fenced\n```")).as_deref(), Some("feature/fenced"));
        assert_eq!(work_branch(Some("use branch = fix/1")).as_deref(), Some("fix/1"));
        assert_eq!(work_branch(Some("Team branch: team/x")), None, "team declarations never leak");
        assert_eq!(work_branch(Some("Team branch: team/x\nWork on branch: feature/y")).as_deref(), Some("feature/y"));
        assert_eq!(work_branch(Some("branch: -evil")), None);
        assert_eq!(work_branch(Some("nothing here")), None);
    }
}
