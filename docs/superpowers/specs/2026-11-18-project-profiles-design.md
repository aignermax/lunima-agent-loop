# Project model and budget profiles

Approved by the user's “fixe es wie von dir vorgeschlagen” following the settings proposal.

Projects own an explicit provider allowlist, default model, optional role/label variants
(complex, premium/claudeapi, eco, reviewer, critical review, QA, feedback, PO, discovery),
optional fallback, and individual budget overrides. Empty overrides inherit the project
default and then provider/global defaults. Model changes and budget escalation are separate.
All sessions resolve these settings centrally; neither labels nor fallback can expand the
allowlist. Mixed-project chat remains Claude/Codex only. Secrets and machine resource limits
remain global. Old configurations without a profile preserve existing routing and models.

UI: expandable project profile editor; inherited values show their effective default.
Provider restrictions are independent from model selections. Unsupported reasoning/turn-limit
settings are explained. Fallback is only before execution (missing configured API credentials,
missing executable, or known provider cooldown); never replay a partially executed job.
Codex reasoning effort is optional. Claude/API transports reject unsupported effort settings.

Implementation choice: optional typed profile on each RepoConfig, central resolver in a
separate core module, explicit job role/labels, project budget context before each role's work.
Alternatives considered: global-only templates leave projects coupled; fully custom per-label
scripts would be harder to validate. Shared defaults plus explicit overrides meet the request.

Do not rewrite live team.toml or choose models/providers on the user's behalf. Do not push,
commit unrelated preexisting changes, or control desktop apps (the previous CUA handoff stopped).
