//! Experiment metrics for the strategist: visits and call-to-action clicks per published
//! experiment page. Providers: `none` (default — says so instead of inventing numbers),
//! `file` (a JSON file the maintainer exports or maintains) and `goatcounter` (cookieless
//! analytics; only used after the maintainer enabled it for a published page).

use serde::{Deserialize, Serialize};
use std::time::Duration;
use team_core::config::ProductDiscovery;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PageMetrics {
    pub slug: String,
    pub visits: Option<u64>,
    pub cta_clicks: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Metrics {
    pub available: bool,
    pub source: String,
    pub note: String,
    pub pages: Vec<PageMetrics>,
}

/// `(url, token)` → response body; the token travels on stdin, never on a command line.
pub type Fetch<'a> = &'a dyn Fn(&str, &str) -> Result<String, String>;

pub fn curl_fetch(url: &str, token: &str) -> Result<String, String> {
    let mut cmd = team_core::proc::command("curl");
    cmd.args(["-sSf", "--max-time", "60", "-H", "@-", url]);
    let header = format!("Authorization: Bearer {token}\n");
    let out = team_core::proc::run_with_input(cmd, None, Some(header.as_bytes()), Duration::from_secs(90)).map_err(|e| format!("curl: {e}"))?;
    out.into_result().map_err(|e| format!("Analytics-Abruf fehlgeschlagen: {e}"))
}

fn unavailable(source: &str, note: &str) -> Metrics {
    Metrics { available: false, source: source.into(), note: note.into(), pages: vec![] }
}

/// Metrics for `slugs` (published experiment pages) since `since` (YYYY-MM-DD).
pub fn collect(cfg: &ProductDiscovery, slugs: &[String], since: &str, fetch: Fetch) -> Metrics {
    match cfg.analytics.as_str() {
        "file" => match std::fs::read_to_string(&cfg.analytics_site) {
            Ok(text) => match serde_json::from_str::<Metrics>(&text) {
                Ok(mut m) => {
                    m.available = true;
                    m.source = format!("file {}", cfg.analytics_site);
                    m
                }
                Err(e) => unavailable("file", &format!("metrics file unreadable: {e}")),
            },
            Err(e) => unavailable("file", &format!("metrics file missing: {e}")),
        },
        "goatcounter" => {
            let token = std::fs::read_to_string(&cfg.analytics_token_file).map(|t| t.trim().to_string()).unwrap_or_default();
            if cfg.analytics_site.is_empty() || token.is_empty() {
                return unavailable("goatcounter", "GoatCounter not configured (analytics_site / analytics_token_file)");
            }
            let url = format!("https://{}.goatcounter.com/api/v0/stats/hits?start={since}&limit=200", cfg.analytics_site);
            match fetch(&url, &token).and_then(|body| parse_goatcounter(&body, slugs)) {
                Ok(pages) => Metrics { available: true, source: "goatcounter".into(), note: format!("since {since}"), pages },
                Err(e) => unavailable("goatcounter", &e),
            }
        }
        _ => unavailable("none", "No analytics configured. Do not infer visits, clicks or conversions."),
    }
}

/// GoatCounter `/stats/hits`: page paths `/x/<slug>/` count visits, events `cta-<kind>-<slug>` clicks.
pub fn parse_goatcounter(body: &str, slugs: &[String]) -> Result<Vec<PageMetrics>, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| format!("GoatCounter-Antwort unlesbar: {e}"))?;
    let hits = v.get("hits").and_then(|h| h.as_array()).ok_or("GoatCounter-Antwort ohne hits")?;
    Ok(slugs
        .iter()
        .map(|slug| {
            let mut visits = 0;
            let mut clicks = 0;
            for h in hits {
                let path = h.get("path").and_then(|p| p.as_str()).unwrap_or("");
                let count = h.get("count").and_then(|c| c.as_u64()).unwrap_or(0);
                if h.get("event").and_then(|e| e.as_bool()).unwrap_or(false) {
                    if path.starts_with("cta-") && path.ends_with(&format!("-{slug}")) {
                        clicks += count;
                    }
                } else if path.trim_matches('/') == format!("x/{slug}") {
                    visits += count;
                }
            }
            PageMetrics { slug: slug.clone(), visits: Some(visits), cta_clicks: Some(clicks) }
        })
        .collect())
}

/// Markdown for the strategist's context.
pub fn markdown(m: &Metrics) -> String {
    if !m.available {
        return format!("# Experiment metrics\n\nUNAVAILABLE ({}): {}\n", m.source, m.note);
    }
    let mut out = format!("# Experiment metrics\n\nSource: {} ({}).\n\n| page | visits | CTA clicks |\n|---|---|---|\n", m.source, m.note);
    for p in &m.pages {
        let n = |v: Option<u64>| v.map(|v| v.to_string()).unwrap_or_else(|| "unknown".into());
        out.push_str(&format!("| {} | {} | {} |\n", p.slug, n(p.visits), n(p.cta_clicks)));
    }
    if m.pages.is_empty() {
        out.push_str("| (no published experiment pages) | | |\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_says_unavailable_instead_of_numbers() {
        let m = collect(&ProductDiscovery::default(), &["a".into()], "2026-11-01", &|_, _| panic!("no fetch"));
        assert!(!m.available);
        assert!(markdown(&m).contains("UNAVAILABLE") && markdown(&m).contains("Do not infer"));
    }

    #[test]
    fn goatcounter_counts_visits_and_cta_events_per_page() {
        let t = tempfile::tempdir().unwrap();
        let token = t.path().join("tok");
        std::fs::write(&token, "secret\n").unwrap();
        let cfg = ProductDiscovery { analytics: "goatcounter".into(), analytics_site: "lunima".into(), analytics_token_file: token.display().to_string(), ..Default::default() };
        let body = r#"{"hits":[{"path":"/x/gds/","count":40},{"path":"/x/gds","count":2},{"path":"cta-pilot-gds","event":true,"count":3},{"path":"/x/edu/","count":9},{"path":"/","count":500}]}"#;
        let m = collect(&cfg, &["gds".into(), "edu".into()], "2026-11-01", &|url, tok| {
            assert_eq!(tok, "secret");
            assert!(url.starts_with("https://lunima.goatcounter.com/api/v0/stats/hits?start=2026-11-01"));
            Ok(body.into())
        });
        assert!(m.available);
        assert_eq!(m.pages[0], PageMetrics { slug: "gds".into(), visits: Some(42), cta_clicks: Some(3) });
        assert_eq!(m.pages[1].visits, Some(9));
        let failing = collect(&cfg, &["gds".into()], "2026-11-01", &|_, _| Err("timeout".into()));
        assert!(!failing.available && failing.note.contains("timeout"), "a failing service is reported, not hidden");
    }

    #[test]
    fn file_provider_reads_maintainer_export() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("m.json");
        std::fs::write(&f, r#"{"note":"exported 2026-11-20","pages":[{"slug":"gds","visits":12,"cta_clicks":1}]}"#).unwrap();
        let cfg = ProductDiscovery { analytics: "file".into(), analytics_site: f.display().to_string(), ..Default::default() };
        let m = collect(&cfg, &[], "x", &|_, _| unreachable!());
        assert!(m.available && m.pages[0].visits == Some(12));
        assert!(markdown(&m).contains("| gds | 12 | 1 |"));
    }
}
