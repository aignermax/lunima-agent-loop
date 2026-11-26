//! Validated project policy shared by the daemon and Control Center.
use crate::config::{Backend, Budgets, Routing, TeamConfig};
use crate::provider::{self, Choice};
use serde::{Deserialize, Serialize};

#[path = "project_budgets.rs"]
mod budgets;
pub use budgets::ProjectBudgets;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
    Ultra,
}
impl ReasoningEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
            Self::Ultra => "ultra",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelSpec {
    pub backend: Option<Backend>,
    /// None inherits; Some("") explicitly requests the CLI default.
    pub model: Option<String>,
    pub effort: Option<ReasoningEffort>,
}
impl ModelSpec {
    fn overlay(&self, parent: &Self) -> Self {
        let same_backend = self.backend.is_none() || self.backend == parent.backend;
        Self {
            backend: self.backend.or(parent.backend),
            model: self
                .model
                .clone()
                .or_else(|| same_backend.then(|| parent.model.clone()).flatten()),
            effort: self
                .effort
                .or_else(|| same_backend.then_some(parent.effort).flatten()),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectProfile {
    pub allowed_backends: Vec<Backend>,
    pub standard: ModelSpec,
    pub complex: Option<ModelSpec>,
    pub premium: Option<ModelSpec>,
    pub eco: Option<ModelSpec>,
    pub reviewer: Option<ModelSpec>,
    pub critical: Option<ModelSpec>,
    pub qa: Option<ModelSpec>,
    pub feedback: Option<ModelSpec>,
    pub product_owner: Option<ModelSpec>,
    pub discovery: Option<ModelSpec>,
    pub fallback: Option<ModelSpec>,
    pub budgets: ProjectBudgets,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SessionRole {
    #[default]
    Coder,
    Reviewer,
    Qa,
    Feedback,
    ProductOwner,
    Discovery,
}

#[derive(Debug, Clone)]
pub struct Decision {
    pub backend: Backend,
    pub choice: Choice,
    pub effort: Option<ReasoningEffort>,
}

pub fn routing_backend(routing: Routing) -> Backend {
    match routing {
        Routing::Claude => Backend::Claude,
        Routing::Codex => Backend::Codex,
        Routing::Kimi | Routing::KimiOnly => Backend::Openrouter,
        Routing::OpenAi => Backend::OpenAi,
        Routing::Eco => Backend::Eco,
    }
}

impl ProjectProfile {
    pub fn from_legacy(cfg: &TeamConfig, repo: &str) -> Self {
        let routing = cfg.repo(repo).map(|r| r.routing).unwrap_or_default();
        let backend = routing_backend(routing);
        let spec = |backend| ModelSpec {
            backend: Some(backend),
            ..Default::default()
        };
        let mut p = Self {
            allowed_backends: vec![backend],
            standard: spec(backend),
            ..Default::default()
        };
        if matches!(routing, Routing::Kimi | Routing::KimiOnly) {
            p.allowed_backends.push(Backend::Claude);
            p.reviewer = Some(spec(Backend::Claude));
            p.product_owner = Some(spec(Backend::Claude));
            p.discovery = Some(spec(Backend::Claude));
            p.premium = Some(spec(Backend::Claude));
            if routing == Routing::Kimi {
                p.allowed_backends.push(Backend::Eco);
                p.eco = Some(spec(Backend::Eco));
                p.fallback = Some(spec(Backend::Claude));
                if cfg.providers.complex_uses_premium {
                    p.complex = Some(spec(Backend::Claude));
                }
            }
        } else if backend == Backend::Claude && cfg.providers.complex_uses_premium {
            p.complex = Some(spec(backend));
        }
        if backend == Backend::Claude {
            p.premium = Some(spec(backend));
        }
        p.critical = Some(ModelSpec::default());
        p
    }
}

fn role_spec(p: &ProjectProfile, role: SessionRole) -> Option<&ModelSpec> {
    match role {
        SessionRole::Coder => None,
        SessionRole::Reviewer => p.reviewer.as_ref(),
        SessionRole::Qa => p.qa.as_ref().or(p.reviewer.as_ref()),
        SessionRole::Feedback => p.feedback.as_ref(),
        SessionRole::ProductOwner => p.product_owner.as_ref(),
        SessionRole::Discovery => p.discovery.as_ref().or(p.product_owner.as_ref()),
    }
}

fn standard_spec(cfg: &TeamConfig, repo: &str, p: &ProjectProfile) -> ModelSpec {
    ModelSpec {
        backend: Some(backend_for(cfg, repo, &p.standard)),
        ..p.standard.clone()
    }
}

fn selection(
    cfg: &TeamConfig,
    repo: &str,
    p: &ProjectProfile,
    role: SessionRole,
    labels: &[String],
) -> (ModelSpec, &'static str) {
    let standard = standard_spec(cfg, repo, p);
    let base = role_spec(p, role)
        .map(|s| s.overlay(&standard))
        .unwrap_or(standard);
    let has = |tag: &str| !tag.is_empty() && labels.iter().any(|l| l.eq_ignore_ascii_case(tag));
    let variant =
        if matches!(role, SessionRole::Reviewer | SessionRole::Qa) && has(&cfg.labels.critical) {
            (p.critical.as_ref(), "critical")
        } else if matches!(role, SessionRole::Coder | SessionRole::Feedback) {
            if has(&cfg.labels.claudeapi) {
                (p.premium.as_ref().or(p.complex.as_ref()), "premium")
            } else if has(&cfg.labels.complex) {
                (p.complex.as_ref(), "complex")
            } else if has(&cfg.labels.eco) {
                (p.eco.as_ref(), "eco")
            } else {
                (None, "standard")
            }
        } else {
            (None, "standard")
        };
    (
        variant.0.map(|s| s.overlay(&base)).unwrap_or(base),
        if variant.0.is_some() {
            variant.1
        } else {
            "standard"
        },
    )
}

fn backend_for(cfg: &TeamConfig, repo: &str, spec: &ModelSpec) -> Backend {
    spec.backend
        .unwrap_or_else(|| routing_backend(cfg.repo(repo).map(|r| r.routing).unwrap_or_default()))
}

pub fn validate(cfg: &TeamConfig, repo: &str, p: &ProjectProfile) -> Result<(), String> {
    if p.allowed_backends.is_empty() {
        return Err(format!("{repo}: mindestens einen Anbieter freigeben"));
    }
    let check = |spec: ModelSpec| -> Result<(), String> {
        let backend = backend_for(cfg, repo, &spec);
        if !p.allowed_backends.contains(&backend) {
            return Err(format!(
                "{repo}: {} ist im Projekt nicht freigegeben",
                backend.label()
            ));
        }
        if spec.effort.is_some() && !matches!(backend, Backend::Codex | Backend::OpenAi) {
            return Err(format!(
                "{repo}: Denkaufwand ist nur für Codex/Responses einstellbar"
            ));
        }
        Ok(())
    };
    let standard = standard_spec(cfg, repo, p);
    check(standard.clone())?;
    for role in [
        SessionRole::Coder,
        SessionRole::Reviewer,
        SessionRole::Qa,
        SessionRole::Feedback,
        SessionRole::ProductOwner,
        SessionRole::Discovery,
    ] {
        let base = role_spec(p, role)
            .map(|s| s.overlay(&standard))
            .unwrap_or_else(|| standard.clone());
        check(base.clone())?;
        let variants: Vec<&ModelSpec> = if matches!(role, SessionRole::Reviewer | SessionRole::Qa) {
            p.critical.iter().collect()
        } else if matches!(role, SessionRole::Coder | SessionRole::Feedback) {
            [p.complex.as_ref(), p.premium.as_ref(), p.eco.as_ref()]
                .into_iter()
                .flatten()
                .collect()
        } else {
            Vec::new()
        };
        for spec in variants {
            check(spec.overlay(&base))?;
        }
    }
    if let Some(spec) = &p.fallback {
        check(spec.overlay(&standard))?;
    }
    let budgets = p.budgets.apply(&cfg.budgets);
    if [
        p.budgets.product_owner_timeout_minutes,
        p.budgets.discovery_timeout_minutes,
    ]
    .into_iter()
    .flatten()
    .any(|v| v == 0)
    {
        return Err(format!(
            "{repo}: Rollen-Zeitlimits müssen größer als null sein"
        ));
    }
    if budgets.timeout_minutes_regular == 0
        || budgets.timeout_minutes_complex == 0
        || budgets.max_sessions == 0
        || budgets.max_attempts == 0
        || budgets.inactivity_minutes == 0
    {
        return Err(format!(
            "{repo}: Zeitlimit, Sitzungen und Versuche müssen größer als null sein"
        ));
    }
    if [budgets.max_cost_usd_regular, budgets.max_cost_usd_complex].iter().any(|usd| !usd.is_finite() || *usd <= 0.0) {
        return Err(format!("{repo}: Kostenbudgets müssen positive Beträge (USD) sein"));
    }
    Ok(())
}

pub fn budgets_for(cfg: &TeamConfig, repo: &str) -> Budgets {
    cfg.repo(repo)
        .and_then(|r| r.profile.as_ref())
        .map(|p| p.budgets.apply(&cfg.budgets))
        .unwrap_or_else(|| cfg.budgets.clone())
}

/// Providers which any configured role, label variant or fallback may actually use.
pub fn configured_backends(cfg: &TeamConfig, repo: &str) -> Result<Vec<Backend>, String> {
    let p = cfg
        .repo(repo)
        .and_then(|r| r.profile.as_ref())
        .ok_or("Projektprofil fehlt")?;
    validate(cfg, repo, p)?;
    let mut backends = Vec::new();
    for role in [
        SessionRole::Coder,
        SessionRole::Reviewer,
        SessionRole::Qa,
        SessionRole::Feedback,
        SessionRole::ProductOwner,
        SessionRole::Discovery,
    ] {
        for labels in [
            vec![],
            vec![cfg.labels.complex.clone()],
            vec![cfg.labels.critical.clone()],
            vec![cfg.labels.eco.clone()],
            vec![cfg.labels.claudeapi.clone()],
        ] {
            let (spec, _) = selection(cfg, repo, p, role, &labels);
            let backend = backend_for(cfg, repo, &spec);
            if !backends.contains(&backend) {
                backends.push(backend);
            }
        }
    }
    if let Some(fallback) = &p.fallback {
        let backend = backend_for(cfg, repo, &fallback.overlay(&standard_spec(cfg, repo, p)));
        if !backends.contains(&backend) {
            backends.push(backend);
        }
    }
    Ok(backends)
}

fn default_model(cfg: &TeamConfig, backend: Backend, role: SessionRole, variant: &str) -> String {
    match backend {
        Backend::Codex => cfg.runner.model.clone(),
        Backend::OpenAi => cfg.providers.openai.model.clone(),
        Backend::Openrouter => cfg.providers.openrouter.model.clone(),
        Backend::Eco => cfg.providers.eco.model.clone(),
        Backend::Claude => match (role, variant) {
            (SessionRole::Coder | SessionRole::Feedback, "complex" | "premium") => {
                cfg.models.premium.clone()
            }
            (SessionRole::Reviewer | SessionRole::Qa, "critical") => {
                cfg.models.reviewer_critical.clone()
            }
            (SessionRole::Reviewer | SessionRole::Qa, _) => cfg.models.reviewer.clone(),
            (SessionRole::ProductOwner, _) => cfg.models.product_owner.clone(),
            (SessionRole::Discovery, _) if !cfg.product_discovery.model.is_empty() => {
                cfg.product_discovery.model.clone()
            }
            (SessionRole::Discovery, _) => cfg.models.product_owner.clone(),
            _ => cfg.models.coder.clone(),
        },
    }
}

fn decision(
    cfg: &TeamConfig,
    repo: &str,
    spec: &ModelSpec,
    role: SessionRole,
    variant: &str,
) -> Result<Decision, String> {
    let backend = backend_for(cfg, repo, spec);
    let model = spec
        .model
        .clone()
        .unwrap_or_else(|| default_model(cfg, backend, role, variant));
    let mut providers = cfg.providers.clone();
    match backend {
        Backend::OpenAi => providers.openai.model = model.clone(),
        Backend::Openrouter => providers.openrouter.model = model.clone(),
        Backend::Eco => providers.eco.model = model.clone(),
        _ => {}
    }
    let choice = provider::runner_choice(
        &crate::config::Runner {
            backend,
            model: model.clone(),
        },
        &providers,
    )?
    .unwrap_or_else(|| Choice {
        model,
        env: provider::ChildEnv::subscription(),
        why: "project Claude profile".into(),
    });
    Ok(Decision {
        backend,
        choice: Choice {
            why: format!("Projekt {repo}: {role:?}/{variant} → {}", backend.label()),
            ..choice
        },
        effort: spec.effort,
    })
}

pub fn resolve(
    cfg: &TeamConfig,
    repo: &str,
    role: SessionRole,
    labels: &[String],
) -> Result<Decision, String> {
    let p = cfg
        .repo(repo)
        .and_then(|r| r.profile.as_ref())
        .ok_or("Projektprofil fehlt")?;
    validate(cfg, repo, p)?;
    let (spec, variant) = selection(cfg, repo, p, role, labels);
    match decision(cfg, repo, &spec, role, variant) {
        Ok(d) => Ok(d),
        Err(original) => {
            resolve_fallback(cfg, repo, role).map_err(|fallback| format!("{original}; {fallback}"))
        }
    }
}

pub fn resolve_fallback(
    cfg: &TeamConfig,
    repo: &str,
    role: SessionRole,
) -> Result<Decision, String> {
    let p = cfg
        .repo(repo)
        .and_then(|r| r.profile.as_ref())
        .ok_or("Projektprofil fehlt")?;
    validate(cfg, repo, p)?;
    let spec = p
        .fallback
        .as_ref()
        .ok_or("Kein Ausweichmodell konfiguriert — Sitzung wartet")?
        .overlay(&standard_spec(cfg, repo, p));
    decision(cfg, repo, &spec, role, "fallback")
}

#[cfg(test)]
#[path = "project_tests.rs"]
mod tests;
