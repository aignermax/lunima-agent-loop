//! Landing pages for validation experiments: a spec (`discovery/pages/<slug>/page.json`,
//! written by the strategist, checked by `model::check_page`) rendered into one static HTML
//! file with the template in `assets/experiment-page.html`.
//!
//! Previews go to the agent's data folder and are never served. Lunima's GitHub Pages
//! publishes `docs/` of `main`, so a page goes public only when the maintainer runs
//! `lunima-team-agent experiment publish`, which writes `docs/x/<slug>/index.html` and stamps
//! the approval into the spec; the change then goes through a normal reviewed merge.

use super::model::{check_page, PageSpec};
use std::path::{Path, PathBuf};

pub const TEMPLATE: &str = include_str!("../../assets/experiment-page.html");
/// Folder below `docs/` that holds published experiment pages (`lunima.org/x/<slug>/`).
pub const PUBLISHED_DIR: &str = "x";

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

fn url_encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    /// local preview: images straight from the clone, a "PREVIEW" ribbon, no analytics
    Preview { repo: PathBuf },
    /// the public page below `docs/x/<slug>/`
    Publish { contact: String, goatcounter: Option<String> },
}

fn badge(status: &str) -> &'static str {
    match status {
        "available" => "Available today",
        "planned" => "Planned",
        _ => "Research idea",
    }
}

/// The page as HTML. Values are escaped; only fragments built here are inserted raw.
pub fn render(spec: &PageSpec, mode: &Mode) -> Result<String, String> {
    let (contact, analytics, banner) = match mode {
        Mode::Preview { .. } => ("pilot@lunima.invalid".to_string(), String::new(), r#"<div class="preview-banner" role="status">PREVIEW — not published. Content is a draft awaiting the maintainer's approval.</div>"#.to_string()),
        Mode::Publish { contact, goatcounter } => {
            if contact.trim().is_empty() || !contact.contains('@') {
                return Err("Für die Veröffentlichung wird eine Kontaktadresse gebraucht (--contact)".into());
            }
            let analytics = match goatcounter {
                Some(code) if code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') && !code.is_empty() => format!(r#"<script data-goatcounter="https://{code}.goatcounter.com/count" async src="https://gc.zgo.at/count.js"></script>"#),
                Some(code) => return Err(format!("GoatCounter-Code „{code}“ ungültig")),
                None => String::new(),
            };
            (contact.clone(), analytics, String::new())
        }
    };
    let problem: String = spec.problem.iter().map(|p| format!("<p>{}</p>", escape(p))).collect();
    let caps: String = spec.capabilities.iter().map(|c| format!(r#"<li class="cap cap-{}"><span class="badge">{}</span> {}</li>"#, match c.status.as_str() { "available" | "planned" => c.status.as_str(), _ => "research" }, badge(&c.status), escape(&c.text))).collect();
    let mut figures = String::new();
    for f in &spec.figures {
        let src = match mode {
            Mode::Preview { repo } => format!("file://{}", repo.join(&f.src).display()),
            Mode::Publish { .. } => {
                let rel = f.src.strip_prefix("docs/").ok_or_else(|| format!("Bild {} liegt nicht unter docs/ und wäre öffentlich nicht erreichbar", f.src))?;
                format!("../../{rel}")
            }
        };
        figures.push_str(&format!(r#"<figure><img src="{}" alt="{}" loading="lazy"><figcaption>{}</figcaption></figure>"#, escape(&src), escape(&f.caption), escape(&f.caption)));
    }
    let cta: String = spec
        .cta
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let subject = url_encode(&format!("[Lunima {}] {}", c.kind, spec.experiment));
            format!(r#"<a class="btn {}" href="mailto:{}?subject={subject}" data-goatcounter-click="cta-{}-{}">{}</a>"#, if i == 0 { "btn-primary" } else { "btn-secondary" }, escape(&contact), escape(&c.kind), escape(&spec.slug), escape(&c.label))
        })
        .collect();
    let privacy = if analytics.is_empty() {
        "This page sets no cookies and counts no visits. If you write to us, we use your message only to reply and delete it on request."
    } else {
        "This page sets no cookies. Visits and button clicks are counted anonymously with GoatCounter (no personal data, no tracking across sites). If you write to us, we use your message only to reply and delete it on request."
    };
    let pairs: [(&str, String); 11] = [
        ("{{TITLE}}", escape(&spec.title)),
        ("{{TAGLINE}}", escape(&spec.tagline)),
        ("{{EXPERIMENT_ID}}", escape(&spec.experiment)),
        ("{{PROBLEM_HTML}}", problem),
        ("{{CAPABILITIES_HTML}}", format!("<ul>{caps}</ul>")),
        ("{{EVIDENCE_HTML}}", figures),
        ("{{CTA_HTML}}", cta),
        ("{{HONESTY_NOTE}}", escape(&spec.honesty_note)),
        ("{{PRIVACY_NOTE}}", escape(privacy)),
        ("{{ANALYTICS_HTML}}", analytics),
        ("{{PREVIEW_BANNER_HTML}}", banner),
    ];
    let mut html = TEMPLATE.to_string();
    for (k, v) in pairs {
        html = html.replace(k, &v);
    }
    if html.contains("{{") {
        return Err("Template enthält unbekannte Platzhalter".into());
    }
    Ok(html)
}

/// Renders every valid page spec of the clone into `<out>/<slug>/index.html`; returns the
/// slugs written. Invalid specs are skipped (the stage validation reports them).
pub fn render_previews(repo: &Path, out: &Path) -> Vec<String> {
    let mut done = Vec::new();
    let Ok(dirs) = std::fs::read_dir(repo.join(super::model::ROOT).join("pages")) else { return done };
    for d in dirs.flatten() {
        let file = d.path().join("page.json");
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        let rel = format!("{}/pages/{}/page.json", super::model::ROOT, d.file_name().to_string_lossy());
        if !check_page(&rel, &text, repo, None).is_empty() {
            continue;
        }
        let Ok(spec) = serde_json::from_str::<PageSpec>(&text) else { continue };
        if let Ok(html) = render(&spec, &Mode::Preview { repo: repo.to_path_buf() }) {
            let dir = out.join(&spec.slug);
            if std::fs::create_dir_all(&dir).is_ok() && std::fs::write(dir.join("index.html"), html).is_ok() {
                done.push(spec.slug);
            }
        }
    }
    done.sort();
    done
}

/// The maintainer's publish step: validates the draft, writes `docs/x/<slug>/index.html`
/// and records the approval in the spec. Nothing is committed or pushed.
pub fn publish(repo: &Path, slug: &str, approved_by: &str, contact: &str, goatcounter: Option<String>, today: &str) -> Result<PathBuf, String> {
    if approved_by.trim().is_empty() {
        return Err("--approved-by fehlt".into());
    }
    let rel = format!("{}/pages/{slug}/page.json", super::model::ROOT);
    let spec_file = repo.join(&rel);
    let text = std::fs::read_to_string(&spec_file).map_err(|e| format!("{}: {e}", spec_file.display()))?;
    let errs = check_page(&rel, &text, repo, None);
    if !errs.is_empty() {
        return Err(format!("Seite ist nicht veröffentlichungsfähig:\n- {}", errs.join("\n- ")));
    }
    let mut spec: PageSpec = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let html = render(&spec, &Mode::Publish { contact: contact.into(), goatcounter })?;
    let out = repo.join("docs").join(PUBLISHED_DIR).join(slug);
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    std::fs::write(out.join("index.html"), html).map_err(|e| e.to_string())?;
    spec.status = "approved".into();
    spec.approved_by = approved_by.trim().into();
    spec.approved_on = today.into();
    std::fs::write(&spec_file, serde_json::to_string_pretty(&spec).map_err(|e| e.to_string())? + "\n").map_err(|e| e.to_string())?;
    Ok(out.join("index.html"))
}

#[cfg(test)]
mod tests {
    use super::super::model::{Capability, Cta, Figure};
    use super::*;

    fn spec() -> PageSpec {
        PageSpec {
            experiment: "X-2026-w44-gds".into(),
            slug: "gds-reconstruction".into(),
            title: "Editable circuits from <foreign> GDS".into(),
            tagline: "Import a layout, get components back".into(),
            problem: vec!["Layouts arrive as flat polygons & labels.".into()],
            capabilities: vec![Capability { text: "Hierarchy import".into(), status: "available".into(), source: "docs/AGENT_BRIDGE.md".into() }, Capability { text: "Pin recovery".into(), status: "planned".into(), source: String::new() }],
            figures: vec![Figure { src: "docs/media/shot.png".into(), caption: "Imported ring".into() }],
            cta: vec![Cta { label: "Join the pilot".into(), kind: "pilot".into() }],
            honesty_note: "Pin recovery is not available yet; this page measures interest before we build it.".into(),
            status: "draft".into(),
            ..Default::default()
        }
    }

    #[test]
    fn preview_is_escaped_marked_and_untracked() {
        let html = render(&spec(), &Mode::Preview { repo: "/r".into() }).unwrap();
        assert!(html.contains("Editable circuits from &lt;foreign&gt; GDS"));
        assert!(html.contains("PREVIEW — not published"));
        assert!(html.contains(r#"class="cap cap-available"><span class="badge">Available today</span>"#));
        assert!(html.contains("cap-planned"));
        assert!(html.contains("file:///r/docs/media/shot.png"));
        assert!(html.contains("mailto:pilot@lunima.invalid?subject=%5BLunima%20pilot%5D%20X-2026-w44-gds"));
        assert!(!html.contains("goatcounter.com"), "no analytics in previews");
        assert!(!html.contains("{{"));
        assert!(html.contains("counts no visits"));
    }

    #[test]
    fn publishing_needs_contact_and_explicit_analytics() {
        assert!(render(&spec(), &Mode::Publish { contact: String::new(), goatcounter: None }).is_err());
        let html = render(&spec(), &Mode::Publish { contact: "hello@lunima.org".into(), goatcounter: None }).unwrap();
        assert!(!html.contains("PREVIEW") && !html.contains("<script"));
        assert!(html.contains(r#"src="../../media/shot.png""#));
        let with = render(&spec(), &Mode::Publish { contact: "hello@lunima.org".into(), goatcounter: Some("lunima".into()) }).unwrap();
        assert!(with.contains("https://lunima.goatcounter.com/count") && with.contains("counted anonymously"));
        assert!(render(&spec(), &Mode::Publish { contact: "a@b.org".into(), goatcounter: Some("x\" onload=\"".into()) }).is_err());
    }

    #[test]
    fn publish_writes_the_page_and_stamps_the_approval() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        for (p, c) in [("docs/AGENT_BRIDGE.md", "x"), ("docs/media/shot.png", "png")] {
            std::fs::create_dir_all(r.join(p).parent().unwrap()).unwrap();
            std::fs::write(r.join(p), c).unwrap();
        }
        let dir = r.join("discovery/pages/gds-reconstruction");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("page.json"), serde_json::to_string(&spec()).unwrap()).unwrap();
        assert_eq!(render_previews(r, &t.path().join("prev")), vec!["gds-reconstruction"]);
        assert!(publish(r, "gds-reconstruction", "", "a@b.org", None, "2026-11-02").is_err(), "approval name required");
        let out = publish(r, "gds-reconstruction", "Max", "hello@lunima.org", None, "2026-11-02").unwrap();
        assert_eq!(out, r.join("docs/x/gds-reconstruction/index.html"));
        let stamped: PageSpec = serde_json::from_str(&std::fs::read_to_string(dir.join("page.json")).unwrap()).unwrap();
        assert_eq!((stamped.status.as_str(), stamped.approved_by.as_str(), stamped.approved_on.as_str()), ("approved", "Max", "2026-11-02"));
    }
}
