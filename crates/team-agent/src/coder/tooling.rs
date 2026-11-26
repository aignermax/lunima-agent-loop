//! The fixed tooling step of a coder worktree: the tools from a pinned catalog are on the
//! machine, the stack is detected from the tracked files, and the session's prompt gets a
//! short "Tooling for this repository" block. Deterministic on purpose: no session installs
//! anything, and nothing outside the catalog is ever installed.

use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;
use team_core::git::run_git;
use team_core::proc::{command, run, which};

/// One allowlisted tool, installed machine-wide at exactly this version.
struct Tool {
    /// the command on PATH
    bin: &'static str,
    /// npm package (the only source in use so far)
    npm: &'static str,
    version: &'static str,
}

/// The catalog. A version bump is a reviewed change of this list; CodeGraph sends no
/// telemetry from agent sessions (`CODEGRAPH_TELEMETRY=0`, `--no-telemetry`).
const CATALOG: [Tool; 1] = [Tool { bin: "codegraph", npm: "@colbymchenry/codegraph", version: "1.6.4" }];

const CHECK_TIMEOUT: Duration = Duration::from_secs(60);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Brings every catalog tool to its pinned version, once per agent process. Best effort: a
/// tool that cannot be installed is logged and the sessions run without it.
pub fn ensure_catalog() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        for tool in &CATALOG {
            ensure(tool);
        }
    });
}

fn ensure(tool: &Tool) {
    // the pin is a floor: a newer machine-wide install (Max's own) is never downgraded
    if installed_version(tool).is_some_and(|v| !older(&v, tool.version)) {
        return;
    }
    let Some(npm) = which("npm") else {
        crate::log::warn(&format!("{} {} fehlt und npm ist nicht installiert", tool.bin, tool.version));
        return;
    };
    let mut cmd = command(&npm);
    cmd.args(["install", "-g", "--no-audit", "--no-fund", &format!("{}@{}", tool.npm, tool.version)]);
    match run(cmd, None, INSTALL_TIMEOUT) {
        Ok(out) if out.ok() && installed_version(tool).is_some_and(|v| !older(&v, tool.version)) => crate::log::info(&format!("{} {} installiert", tool.bin, tool.version)),
        Ok(out) => crate::log::warn(&format!("{} {} nicht installiert: {}", tool.bin, tool.version, out.stderr.trim().chars().take(300).collect::<String>())),
        Err(e) => crate::log::warn(&format!("npm nicht gestartet: {e}")),
    }
}

/// `have` is an older release than `want` (numeric dotted versions; anything unparsable counts
/// as older, so a broken install is repaired).
fn older(have: &str, want: &str) -> bool {
    let parse = |v: &str| v.trim_start_matches('v').split('.').map(|p| p.parse::<u64>().ok()).collect::<Option<Vec<_>>>();
    match (parse(have), parse(want)) {
        (Some(h), Some(w)) => h < w,
        _ => true,
    }
}

fn installed_version(tool: &Tool) -> Option<String> {
    let bin = which(tool.bin)?;
    let mut cmd = command(&bin);
    cmd.arg("--version");
    let out = run(cmd, None, CHECK_TIMEOUT).ok().filter(|o| o.ok())?;
    out.stdout.split_whitespace().last().map(str::to_string)
}

/// What the tracked files say about a repository.
#[derive(Debug, Default, PartialEq)]
pub struct Stack {
    pub python: bool,
    pub pytest: bool,
    pub dotnet: bool,
    pub rust: bool,
    pub node: bool,
}

/// Reads the stack from `git ls-files` (tracked files only, so build output never counts).
pub fn detect(files: &[&str]) -> Stack {
    let name = |f: &str| f.rsplit('/').next().unwrap_or(f).to_ascii_lowercase();
    let any = |pred: &dyn Fn(&str) -> bool| files.iter().any(|f| pred(f));
    Stack {
        python: any(&|f| f.ends_with(".py")),
        pytest: any(&|f| matches!(name(f).as_str(), "conftest.py" | "pytest.ini")),
        dotnet: any(&|f| f.ends_with(".csproj") || f.ends_with(".sln") || f.ends_with(".slnx")),
        rust: any(&|f| name(f) == "cargo.toml"),
        node: any(&|f| name(f) == "package.json"),
    }
}

/// The prompt block for a worktree: the CodeGraph lines only with an index, a test line per
/// detected stack.
pub fn note(wt: &Path, base: &str) -> String {
    let listed = run_git(Some(wt), &["ls-files"], false).unwrap_or_default();
    let files: Vec<&str> = listed.lines().collect();
    render(&detect(&files), wt.join(".codegraph").is_dir(), base)
}

fn render(stack: &Stack, indexed: bool, base: &str) -> String {
    let mut lines = vec!["\n\n## Tooling for this repository\nThe repository's AGENTS.md / CLAUDE.md wins where it says otherwise.".to_string()];
    if indexed {
        lines.push("- Code: `codegraph_explore` (the index is ready).".into());
        lines.push(format!(
            "- Tests your change affects: `git diff --name-only origin/{base} | codegraph affected --stdin -q` lists the test files that depend on the changed files. Run those, not the whole suite: CI runs everything. It is an approximation."
        ));
    }
    if stack.pytest {
        lines.push("- Python tests: `python -m pytest -q --tb=short PATH::name` for one test; `--lf` reruns only the last failures.".into());
    } else if stack.python {
        lines.push("- Python: run single test files, never the whole tree.".into());
    }
    if stack.dotnet {
        lines.push("- .NET: `dotnet test --nologo -v q --filter FullyQualifiedName~Name`; build with `-v q` too.".into());
    }
    if stack.rust {
        lines.push("- Rust: `cargo test -q -p <crate> <name>`; build and clippy per crate (`-p`).".into());
    }
    if stack.node {
        lines.push("- Node: run one test file through the package's test script, not the whole suite.".into());
    }
    lines.push("- Keep output short: quiet flags, `| tail -40` on long logs; never print whole files, full diffs of generated files or full test suites.".into());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stack_comes_from_tracked_files() {
        let tc = detect(&["host/ls/ls_core.py", "tests/conftest.py", "rust/Cargo.toml", "README.md"]);
        assert_eq!(tc, Stack { python: true, pytest: true, rust: true, ..Default::default() });
        let lunima = detect(&["ConnectAPICPro.sln", "CAP-Core/CAP-Core.csproj", "scripts/run_tests.py"]);
        assert!(lunima.dotnet && lunima.python && !lunima.pytest && !lunima.rust);
        assert_eq!(detect(&[]), Stack::default());
    }

    #[test]
    fn the_note_names_affected_tests_only_with_an_index_and_the_commands_of_the_stack() {
        let stack = Stack { python: true, pytest: true, dotnet: true, ..Default::default() };
        let with = render(&stack, true, "main");
        assert!(with.contains("git diff --name-only origin/main | codegraph affected --stdin -q"));
        assert!(with.contains("python -m pytest -q --tb=short") && with.contains("dotnet test --nologo -v q"));
        assert!(!with.contains("cargo"), "no command for a stack the repo does not have");
        let without = render(&stack, false, "main");
        assert!(!without.contains("codegraph"), "no CodeGraph advice without an index");
    }

    #[test]
    fn the_pin_is_a_floor_never_a_downgrade() {
        assert!(older("0.7.12", "1.6.4") && older("1.6.3", "1.6.4") && older("garbage", "1.6.4"));
        assert!(!older("1.6.4", "1.6.4") && !older("1.10.0", "1.6.4") && !older("v2.0.0", "1.6.4"));
    }

    #[test]
    fn the_catalog_pins_exact_versions() {
        for tool in &CATALOG {
            assert!(tool.version.split('.').count() == 3 && tool.version.chars().all(|c| c.is_ascii_digit() || c == '.'), "{}: {}", tool.bin, tool.version);
        }
    }
}
