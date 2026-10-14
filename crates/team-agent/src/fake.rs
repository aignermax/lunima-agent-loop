//! In-memory test doubles: a GitHub repository (`FakeForge`) and a scripted Claude (`FakeLlm`).

use crate::forge::Forge;
use crate::llm::{Job, Llm, LlmError};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Mutex;
use team_core::claude::Outcome;
use team_core::gh::{Comment, Issue, Label, PullRequest, Ref, User};

#[derive(Default)]
pub struct Store {
    pub issues: BTreeMap<u64, Issue>,
    pub prs: BTreeMap<u64, PullRequest>,
    pub comments: BTreeMap<u64, Vec<Comment>>,
    pub locks: BTreeSet<String>,
    /// every mutating call, e.g. "label+ 7 agent-running"
    pub log: Vec<String>,
    next_id: u64,
}

pub struct FakeForge {
    pub repo: String,
    pub default: String,
    pub s: Mutex<Store>,
}

pub fn issue(number: u64, title: &str, body: &str, labels: &[&str]) -> Issue {
    Issue {
        number,
        title: title.into(),
        body: Some(body.into()),
        state: "open".into(),
        labels: labels.iter().map(|l| Label { name: l.to_string() }).collect(),
        assignees: vec![],
        pull_request: None,
        html_url: format!("https://github.com/o/r/issues/{number}"),
    }
}

pub fn pr(number: u64, title: &str, branch: &str, labels: &[&str]) -> PullRequest {
    PullRequest {
        number,
        title: title.into(),
        body: Some(String::new()),
        html_url: format!("https://github.com/o/r/pull/{number}"),
        head: Ref { name: branch.into(), sha: String::new(), repo: None },
        base: Ref { name: "main".into(), sha: String::new(), repo: None },
        labels: labels.iter().map(|l| Label { name: l.to_string() }).collect(),
        user: None,
        updated_at: "2026-10-14T10:00:00Z".into(),
    }
}

impl FakeForge {
    pub fn new(repo: &str) -> Self {
        Self { repo: repo.into(), default: "main".into(), s: Mutex::new(Store { next_id: 1000, ..Default::default() }) }
    }
    pub fn with_issue(self, i: Issue) -> Self {
        self.s.lock().unwrap().issues.insert(i.number, i);
        self
    }
    pub fn with_pr(self, p: PullRequest) -> Self {
        self.s.lock().unwrap().prs.insert(p.number, p);
        self
    }
    pub fn add_comment(&self, number: u64, body: &str) -> u64 {
        let mut s = self.s.lock().unwrap();
        s.next_id += 1;
        let id = s.next_id;
        let c = Comment { id, body: body.into(), user: Some(User { login: "someone".into() }), created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true) };
        s.comments.entry(number).or_default().push(c);
        id
    }
    /// Backdates a comment (e.g. an abandoned claim from hours ago).
    pub fn set_comment_time(&self, number: u64, id: u64, created_at: &str) {
        let mut s = self.s.lock().unwrap();
        if let Some(c) = s.comments.get_mut(&number).and_then(|l| l.iter_mut().find(|c| c.id == id)) {
            c.created_at = created_at.into();
        }
    }
    pub fn log(&self) -> Vec<String> {
        self.s.lock().unwrap().log.clone()
    }
    pub fn labels(&self, number: u64) -> Vec<String> {
        let s = self.s.lock().unwrap();
        let labels = s.issues.get(&number).map(|i| &i.labels).or(s.prs.get(&number).map(|p| &p.labels));
        labels.map(|l| l.iter().map(|l| l.name.clone()).collect()).unwrap_or_default()
    }
    pub fn comment_bodies(&self, number: u64) -> Vec<String> {
        self.s.lock().unwrap().comments.get(&number).map(|c| c.iter().map(|c| c.body.clone()).collect()).unwrap_or_default()
    }
    fn labels_mut<'a>(s: &'a mut Store, number: u64) -> Option<&'a mut Vec<Label>> {
        if s.issues.contains_key(&number) {
            return s.issues.get_mut(&number).map(|i| &mut i.labels);
        }
        s.prs.get_mut(&number).map(|p| &mut p.labels)
    }
}

impl Forge for FakeForge {
    fn name(&self) -> &str {
        &self.repo
    }
    fn default_branch(&self) -> Result<String, String> {
        Ok(self.default.clone())
    }
    fn open_issues(&self, label: &str) -> Result<Vec<Issue>, String> {
        let s = self.s.lock().unwrap();
        Ok(s.issues.values().filter(|i| i.state == "open" && i.has_label(label)).cloned().collect())
    }
    fn issue(&self, number: u64) -> Result<Issue, String> {
        self.s.lock().unwrap().issues.get(&number).cloned().ok_or_else(|| format!("404 issue {number}"))
    }
    fn add_labels(&self, number: u64, labels: &[&str]) -> Result<(), String> {
        let mut s = self.s.lock().unwrap();
        for l in labels {
            s.log.push(format!("label+ {number} {l}"));
            if let Some(list) = Self::labels_mut(&mut s, number) {
                if !list.iter().any(|x| x.name.eq_ignore_ascii_case(l)) {
                    list.push(Label { name: l.to_string() });
                }
            }
        }
        Ok(())
    }
    fn remove_label(&self, number: u64, label: &str) -> Result<(), String> {
        let mut s = self.s.lock().unwrap();
        s.log.push(format!("label- {number} {label}"));
        if let Some(list) = Self::labels_mut(&mut s, number) {
            list.retain(|x| !x.name.eq_ignore_ascii_case(label));
        }
        Ok(())
    }
    fn add_assignee(&self, number: u64, login: &str) -> Result<(), String> {
        let mut s = self.s.lock().unwrap();
        s.log.push(format!("assign {number} {login}"));
        if let Some(i) = s.issues.get_mut(&number) {
            i.assignees.push(User { login: login.into() });
        }
        Ok(())
    }
    fn remove_assignees(&self, number: u64, logins: &[String]) -> Result<(), String> {
        let mut s = self.s.lock().unwrap();
        s.log.push(format!("unassign {number}"));
        if let Some(i) = s.issues.get_mut(&number) {
            i.assignees.retain(|a| !logins.contains(&a.login));
        }
        Ok(())
    }
    fn comment(&self, number: u64, body: &str) -> Result<(), String> {
        self.s.lock().unwrap().log.push(format!("comment {number}"));
        self.add_comment(number, body);
        Ok(())
    }
    fn comments(&self, number: u64) -> Result<Vec<Comment>, String> {
        Ok(self.s.lock().unwrap().comments.get(&number).cloned().unwrap_or_default())
    }
    fn close_issue(&self, number: u64) -> Result<(), String> {
        let mut s = self.s.lock().unwrap();
        s.log.push(format!("close {number}"));
        if let Some(i) = s.issues.get_mut(&number) {
            i.state = "closed".into();
        }
        Ok(())
    }
    fn open_prs(&self) -> Result<Vec<PullRequest>, String> {
        Ok(self.s.lock().unwrap().prs.values().cloned().collect())
    }
    fn pr(&self, number: u64) -> Result<PullRequest, String> {
        self.s.lock().unwrap().prs.get(&number).cloned().ok_or_else(|| format!("404 pr {number}"))
    }
    fn pr_by_branch(&self, branch: &str) -> Result<Option<PullRequest>, String> {
        Ok(self.s.lock().unwrap().prs.values().find(|p| p.head.name == branch).cloned())
    }
    fn create_pr(&self, title: &str, body: &str, head: &str, base: &str) -> Result<PullRequest, String> {
        let mut s = self.s.lock().unwrap();
        let number = s.issues.keys().chain(s.prs.keys()).max().copied().unwrap_or(0) + 1;
        let mut p = pr(number, title, head, &[]);
        p.body = Some(body.into());
        p.base.name = base.into();
        s.log.push(format!("pr {number} {head}->{base}"));
        s.prs.insert(number, p.clone());
        Ok(p)
    }
    fn edit_pr_body(&self, number: u64, body: &str) -> Result<(), String> {
        let mut s = self.s.lock().unwrap();
        s.log.push(format!("edit-pr {number}"));
        if let Some(p) = s.prs.get_mut(&number) {
            p.body = Some(body.into());
        }
        Ok(())
    }
    fn try_lock(&self, key: &str) -> Result<bool, String> {
        let mut s = self.s.lock().unwrap();
        s.log.push(format!("lock {key}"));
        Ok(s.locks.insert(key.into()))
    }
    fn unlock(&self, key: &str) -> Result<(), String> {
        let mut s = self.s.lock().unwrap();
        s.log.push(format!("unlock {key}"));
        s.locks.remove(key);
        Ok(())
    }
}

type Step = Box<dyn Fn(&Job) -> Result<Outcome, LlmError> + Send + Sync>;

/// Scripted Claude: each call pops the next step (the last one repeats); records every job.
pub struct FakeLlm {
    steps: Mutex<Vec<Step>>,
    pub jobs: Mutex<Vec<Job>>,
}

impl FakeLlm {
    pub fn new() -> Self {
        Self { steps: Mutex::new(vec![]), jobs: Mutex::new(vec![]) }
    }
    pub fn then(self, step: impl Fn(&Job) -> Result<Outcome, LlmError> + Send + Sync + 'static) -> Self {
        self.steps.lock().unwrap().push(Box::new(step));
        self
    }
    /// replies with `text`, optionally writing `file` into the job's cwd first
    pub fn reply(self, text: &str, file: Option<&str>) -> Self {
        let (text, file) = (text.to_string(), file.map(String::from));
        self.then(move |job| {
            if let Some(f) = &file {
                write(&job.cwd, f);
            }
            Ok(Outcome { text: text.clone(), subtype: "success".into(), num_turns: 3, ..Default::default() })
        })
    }
    pub fn calls(&self) -> usize {
        self.jobs.lock().unwrap().len()
    }
    pub fn prompt(&self, i: usize) -> String {
        self.jobs.lock().unwrap()[i].prompt.clone()
    }
}

fn write(dir: &Path, name: &str) {
    let p = dir.join(name);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).unwrap();
    }
    let old = std::fs::read_to_string(&p).unwrap_or_default();
    std::fs::write(p, format!("{old}x")).unwrap();
}

impl Llm for FakeLlm {
    fn run(&self, job: &Job) -> Result<Outcome, LlmError> {
        self.jobs.lock().unwrap().push(job.clone());
        let mut steps = self.steps.lock().unwrap();
        let step = if steps.len() > 1 { steps.remove(0) } else { return steps.first().map(|s| s(job)).unwrap_or_else(|| Err(LlmError::Failed("no script".into()))) };
        drop(steps);
        step(job)
    }
}

/// A local bare "origin" with `main` + `dev` (seeded with `files`), usable as clone URL.
pub fn origin(tmp: &Path, files: &[(&str, &str)]) -> std::path::PathBuf {
    use team_core::git::run_git;
    let origin = tmp.join("origin.git");
    let seed = tmp.join("seed");
    run_git(None, &["init", "-q", "--bare", "-b", "main", &origin.display().to_string()], false).unwrap();
    run_git(None, &["clone", "-q", &origin.display().to_string(), &seed.display().to_string()], false).unwrap();
    std::fs::write(seed.join("README.md"), "seed").unwrap();
    for (name, text) in files {
        std::fs::write(seed.join(name), text).unwrap();
    }
    let g = |a: &[&str]| run_git(Some(&seed), a, false).unwrap();
    g(&["add", "-A"]);
    g(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "init"]);
    g(&["push", "-q", "origin", "main"]);
    g(&["push", "-q", "origin", "main:dev"]);
    origin
}

/// Files of `branch` in the bare origin (to assert what was pushed).
pub fn pushed_file(origin: &Path, branch: &str, file: &str) -> Option<String> {
    team_core::git::run_git(None, &["--git-dir", &origin.display().to_string(), "show", &format!("{branch}:{file}")], false).ok()
}

/// A context wired to fakes: data folder under `tmp/home`, every repo cloned from `origin`.
pub fn ctx(tmp: &Path, origin: &Path, forge: std::sync::Arc<FakeForge>, llm: std::sync::Arc<FakeLlm>) -> crate::ctx::Ctx {
    use std::sync::Arc;
    let paths = team_core::paths::Paths::new(tmp.join("home"));
    paths.ensure().unwrap();
    let url = origin.display().to_string();
    let mut cfg = team_core::config::TeamConfig::default();
    cfg.machine = "TESTBOX".into();
    cfg.repos = vec![team_core::config::RepoConfig { name: forge.repo.clone(), ..Default::default() }];
    cfg.discovery.org.clear();
    crate::ctx::Ctx {
        cfg,
        heartbeat: Arc::new(team_core::state::Heartbeat::new(&paths.state(), "test")),
        paths,
        machine: "TESTBOX".into(),
        login: "bot".into(),
        llm,
        forge: Arc::new(move |_| forge.clone() as Arc<dyn Forge>),
        remote: Arc::new(move |_| url.clone()),
        tools: crate::ctx::Tools::default(),
    }
}
