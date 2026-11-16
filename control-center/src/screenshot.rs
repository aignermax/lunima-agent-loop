//! `--screenshot <file.png>`: render the window, save the app's own frame as PNG and quit.
//! Captures only what egui drew (never the desktop behind it), so documentation screenshots
//! can be made on a machine that is in use.

use eframe::egui;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The collector's slow probes (GitHub, tools) need a moment before the pages are complete.
const SETTLE: Duration = Duration::from_secs(6);
/// Window size of every screenshot (points), whatever size and state the user's window had
/// last: eframe restores those even with persistence off, and README images should match.
const SIZE: [f32; 2] = [1240.0, 820.0];
/// A script waiting for the PNG must not hang when the capture never arrives.
const GIVE_UP: Duration = Duration::from_secs(25);

pub struct Screenshot {
    path: PathBuf,
    started: Instant,
    sized: bool,
    requested: bool,
}

impl Screenshot {
    pub fn new(path: PathBuf) -> Self {
        Self { path, started: Instant::now(), sized: false, requested: false }
    }

    /// Call once per frame; exits the process after the PNG is written.
    pub fn tick(&mut self, ctx: &egui::Context) {
        let image = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let code = match save_png(&self.path, &image) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("screenshot: {e}");
                    1
                }
            };
            std::process::exit(code);
        }
        if !self.sized {
            self.sized = true;
            // a maximised window (restored from the user's last session) ignores InnerSize
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(SIZE[0], SIZE[1])));
        }
        if self.started.elapsed() >= GIVE_UP {
            eprintln!("screenshot: no image after {} s — window hidden or the graphics backend doesn't support capture", GIVE_UP.as_secs());
            std::process::exit(2);
        }
        if !self.requested && self.started.elapsed() >= SETTLE {
            self.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
        ctx.request_repaint_after(Duration::from_millis(200));
    }
}

fn save_png(path: &PathBuf, image: &egui::ColorImage) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), image.size[0] as u32, image.size[1] as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    let bytes: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
    writer.write_image_data(&bytes).map_err(|e| e.to_string())?;
    // finish() flushes and reports errors that dropping the writer would swallow
    writer.finish().map_err(|e| e.to_string())
}
