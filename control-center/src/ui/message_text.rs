//! Small, local chat formatter: no HTML, image loading or new dependencies.
use super::theme;
use eframe::egui::{self, text::LayoutJob, Color32, FontId, Rect, Sense, Stroke, TextFormat, Ui};
use regex::Regex;
use std::{ops::Range, sync::OnceLock};

#[derive(Debug, Clone, PartialEq)]
struct Span {
    text: String,
    url: Option<String>,
    strong: bool,
    code: bool,
}

fn parse(text: &str) -> Vec<Span> {
    let normalized = text.replace("\r\n", "\n");
    parse_inline(&normalized, false)
}

fn tokens() -> &'static Regex {
    static TOKENS: OnceLock<Regex> = OnceLock::new();
    TOKENS.get_or_init(|| Regex::new(r#"(?s:```[^\r\n`]*\r?\n(?P<fence>.*?)```)|(?P<code>`[^`\r\n]+`)|\[(?P<label>[^\]\r\n]+)\]\((?P<target>(?i:https?://)[^\s<>]+)\)|(?P<url>(?i:https?://)[^\s<>"`]+)|(?P<strong>\*\*[^*\r\n]+\*\*)"#).expect("chat format regex"))
}

fn plain(text: &str, strong: bool) -> Span {
    Span {
        text: text.into(),
        url: None,
        strong,
        code: false,
    }
}

fn parse_inline(text: &str, strong: bool) -> Vec<Span> {
    let mut out = Vec::new();
    let mut end = 0;
    for captures in tokens().captures_iter(text) {
        let matched = captures.get(0).expect("full match");
        if matched.start() > end {
            out.push(plain(&text[end..matched.start()], strong));
        }
        if let Some(code) = captures.name("fence") {
            out.push(Span {
                text: code.as_str().into(),
                url: None,
                strong,
                code: true,
            });
        } else if let Some(code) = captures.name("code") {
            out.push(Span {
                text: code.as_str()[1..code.as_str().len() - 1].into(),
                url: None,
                strong,
                code: true,
            });
        } else if let Some(bold) = captures.name("strong") {
            out.extend(parse_inline(
                &bold.as_str()[2..bold.as_str().len() - 2],
                true,
            ));
        } else if let (Some(label), Some(target)) =
            (captures.name("label"), captures.name("target"))
        {
            if web_url(target.as_str()) {
                out.push(Span {
                    text: label.as_str().into(),
                    url: Some(target.as_str().into()),
                    strong,
                    code: false,
                });
            } else {
                out.push(plain(matched.as_str(), strong));
            }
        } else if let Some(raw) = captures.name("url") {
            let url = trim_prose(raw.as_str());
            if web_url(url) {
                out.push(Span {
                    text: url.into(),
                    url: Some(url.into()),
                    strong,
                    code: false,
                });
                if url.len() < raw.as_str().len() {
                    out.push(plain(&raw.as_str()[url.len()..], strong));
                }
            } else {
                out.push(plain(raw.as_str(), strong));
            }
        }
        end = matched.end();
    }
    if end < text.len() {
        out.push(plain(&text[end..], strong));
    }
    out
}

fn web_url(url: &str) -> bool {
    let Some((scheme, address)) = url.split_once("://") else {
        return false;
    };
    (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
        && !address
            .split(['/', '?', '#'])
            .next()
            .unwrap_or_default()
            .is_empty()
        && !url
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '"'))
}

fn trim_prose(mut url: &str) -> &str {
    loop {
        let trimmed = url.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'', '’', '”']);
        if trimmed.len() < url.len() {
            url = trimmed;
            continue;
        }
        let unmatched = [('(', ')'), ('[', ']'), ('{', '}')]
            .iter()
            .any(|(open, close)| {
                url.ends_with(*close)
                    && url.chars().filter(|c| c == close).count()
                        > url.chars().filter(|c| c == open).count()
            });
        if unmatched {
            url = &url[..url.len() - 1];
        } else {
            return url;
        }
    }
}

#[derive(Debug)]
struct Link {
    chars: Range<usize>,
    url: String,
}

fn layout(spans: &[Span], body: FontId, mono: FontId, width: f32) -> (LayoutJob, Vec<Link>) {
    let mut job = LayoutJob::default();
    job.wrap.max_width = width;
    let mut links = Vec::new();
    let mut chars = 0;
    for span in spans {
        let count = span.text.chars().count();
        let mut font = if span.code {
            mono.clone()
        } else {
            body.clone()
        };
        if span.strong {
            font.size += 0.5;
        }
        let color = if span.url.is_some() {
            theme::INFO
        } else if span.strong {
            Color32::WHITE
        } else {
            theme::TEXT
        };
        job.append(
            &span.text,
            0.0,
            TextFormat {
                font_id: font,
                color,
                background: if span.code {
                    theme::BG
                } else {
                    Color32::TRANSPARENT
                },
                underline: if span.url.is_some() {
                    Stroke::new(1.0_f32, color)
                } else {
                    Stroke::NONE
                },
                ..Default::default()
            },
        );
        if let Some(url) = &span.url {
            links.push(Link {
                chars: chars..chars + count,
                url: url.clone(),
            });
        }
        chars += count;
    }
    (job, links)
}

/// Link hitboxes follow actual laid-out glyphs, including wrapping and Unicode.
fn link_rects(galley: &egui::Galley, chars: Range<usize>) -> Vec<Rect> {
    let mut offset = 0;
    let mut rects = Vec::new();
    for row in &galley.rows {
        let start = chars.start.saturating_sub(offset).min(row.glyphs.len());
        let end = chars.end.saturating_sub(offset).min(row.glyphs.len());
        if start < end {
            rects.push(Rect::from_min_max(
                egui::pos2(row.glyphs[start].logical_rect().left(), row.rect.top()),
                egui::pos2(
                    row.glyphs[end - 1].logical_rect().right(),
                    row.rect.bottom(),
                ),
            ));
        }
        offset += row.char_count_including_newline();
    }
    rects
}

/// Returns a clicked HTTP(S) URL; the caller opens it through the existing OS helper.
pub fn show(ui: &mut Ui, text: &str) -> Option<String> {
    let spans = parse(text);
    let (job, links) = layout(
        &spans,
        egui::TextStyle::Body.resolve(ui.style()),
        egui::TextStyle::Monospace.resolve(ui.style()),
        ui.available_width(),
    );
    let galley = ui.fonts(|fonts| fonts.layout_job(job));
    let response = ui.add(egui::Label::new(galley.clone()).selectable(true));
    let mut clicked = None;
    for (link_index, link) in links.iter().enumerate() {
        for (row_index, rect) in link_rects(&galley, link.chars.clone()).iter().enumerate() {
            if ui
                .interact(
                    rect.translate(response.rect.min.to_vec2()),
                    response.id.with((link_index, row_index)),
                    Sense::click(),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(&link.url)
                .clicked()
            {
                clicked = Some(link.url.clone());
            }
        }
    }
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_named_links_are_clickable_without_swallowing_prose() {
        let spans = parse(
            "Hier: https://example.test/pr/42.\n\n[Änderung](https://example.test/a_(b)?x=1&y=2)",
        );
        let links: Vec<_> = spans.iter().filter_map(|s| s.url.as_deref()).collect();
        assert_eq!(
            links,
            [
                "https://example.test/pr/42",
                "https://example.test/a_(b)?x=1&y=2"
            ]
        );
        assert_eq!(
            spans.iter().map(|s| s.text.as_str()).collect::<String>(),
            "Hier: https://example.test/pr/42.\n\nÄnderung"
        );
    }

    #[test]
    fn paragraphs_lists_unicode_and_literal_windows_paths_are_preserved() {
        let text = "Übersicht 😀\r\n\r\n- Erster Punkt\r\n- Zweiter Punkt\r\nC:\\new-folder\\test";
        assert_eq!(
            parse(text)
                .iter()
                .map(|s| s.text.as_str())
                .collect::<String>(),
            text.replace("\r\n", "\n")
        );
        assert_eq!(
            parse("kaputtes [Markdown und **offene Sterne")
                .iter()
                .map(|s| s.text.as_str())
                .collect::<String>(),
            "kaputtes [Markdown und **offene Sterne"
        );
    }

    #[test]
    fn code_is_not_clickable_and_unsupported_schemes_remain_plain_text() {
        let text = "`https://example.test/command`\n```sh\nhttps://example.test/example\n```\n[Start](javascript:alert(1)) [Datei](file:///C:/test.exe)";
        let spans = parse(text);
        assert!(spans.iter().all(|s| s.url.is_none()));
        assert!(spans.iter().filter(|s| s.code).count() == 2);
        assert!(!web_url("javascript://example"));
        assert!(!web_url("file:///C:/test.exe"));
        assert!(!web_url("https://"));
        assert!(!web_url("https://example.test/\ncommand"));
    }

    #[test]
    fn emphasis_can_contain_a_link_and_balanced_url_parentheses_are_retained() {
        let spans = parse(
            "**Wichtig: [PR](https://example.test/a_(b))** und (https://example.test/a_(b)).",
        );
        let links: Vec<_> = spans.iter().filter(|s| s.url.is_some()).collect();
        assert_eq!(links.len(), 2);
        assert!(links[0].strong);
        assert_eq!(links[0].text, "PR");
        assert_eq!(links[0].url.as_deref(), Some("https://example.test/a_(b)"));
        assert_eq!(links[1].url.as_deref(), Some("https://example.test/a_(b)"));
    }

    #[test]
    fn wrapped_links_have_clickable_glyph_rects_after_unicode_and_paragraph_breaks() {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let spans = parse("Ä 😀\n\nhttps://example.test/a-very-long-link-that-wraps-across-multiple-lines");
                let (job, links) = layout(&spans, egui::TextStyle::Body.resolve(ui.style()), egui::TextStyle::Monospace.resolve(ui.style()), 120.0);
                let galley = ui.fonts(|fonts| fonts.layout_job(job));
                let rects = link_rects(&galley, links[0].chars.clone());
                assert!(rects.len() > 1);
                assert!(rects.iter().all(|r| r.width() > 0.0 && r.height() > 0.0));
                assert!(rects[0].top() >= galley.rows[2].rect.top());
            });
        });
    }

    #[test]
    fn clicking_a_link_returns_its_target_without_opening_anything_during_render() {
        let ctx = egui::Context::default();
        let text = "Änderung: [PR](https://example.test/pr/42).";
        let mut target = None;
        let mut clicked = None;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let spans = parse(text);
                let (job, links) = layout(
                    &spans,
                    egui::TextStyle::Body.resolve(ui.style()),
                    egui::TextStyle::Monospace.resolve(ui.style()),
                    ui.available_width(),
                );
                let galley = ui.fonts(|fonts| fonts.layout_job(job));
                target = Some(
                    link_rects(&galley, links[0].chars.clone())[0]
                        .translate(ui.next_widget_position().to_vec2())
                        .center(),
                );
                assert!(show(ui, text).is_none());
            });
        });
        let pos = target.unwrap();
        for pressed in [true, false] {
            let input = egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    clicked = show(ui, text).or(clicked.take());
                });
            });
        }
        assert_eq!(clicked.as_deref(), Some("https://example.test/pr/42"));
    }
}
