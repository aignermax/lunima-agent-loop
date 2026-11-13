//! Per-role setup of a repository: GitHub handle, the role's own clone, branches.

use crate::ctx::Ctx;
use crate::forge::Forge;
use std::sync::Arc;
use team_core::git::RepoClone;

pub struct RepoSetup {
    pub repo: String,
    pub forge: Arc<dyn Forge>,
    /// the role's clone (`work/<role>/<owner_repo>`)
    pub clone: RepoClone,
    /// GitHub default branch
    pub default_branch: String,
    /// integration branch (`dev`, or the PO's) if it exists, else the default branch
    pub working_branch: String,
}

/// Clones (or fetches) `repo` for `role`.
pub fn setup(ctx: &Ctx, role: &str, repo: &str) -> Result<RepoSetup, String> {
    let forge = (ctx.forge)(repo);
    let default_branch = forge.default_branch()?;
    let clone = RepoClone::ensure_from(&ctx.paths.clone_dir(role, repo), repo, &(ctx.remote)(repo))?;
    let mut working_branch = clone.working_branch(&default_branch, &ctx.cfg.integration_branch(repo));
    // Outside the PO repo `dev` is only a convention. Work based on an abandoned dev would
    // miss everything merged since, and its PR would target a branch nobody merges.
    if !ctx.cfg.is_po_repo(repo) && clone.is_bypassed(&working_branch, &default_branch) {
        working_branch = default_branch.clone();
    }
    Ok(RepoSetup { repo: repo.to_string(), forge, clone, default_branch, working_branch })
}
