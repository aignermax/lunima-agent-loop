//! `--screenshot <file.png>`: render the window, save the app's own frame as PNG and quit.
//! Captures only what egui drew (never the desktop behind it), so documentation screenshots
//! can be made on a machine that is in use.

use eframe::egui;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The collector's slow probes (GitHub, tools) need a moment before the pages are complete.
const SETTLE: Duration = Duration::from_secs(6);

pub struct Screenshot {
    path: PathBuf,
    started: Instant,
    requested: bool,
}

impl Screenshot {
    pub fn new(path: PathBuf) -> Self {
        Self { path, started: Instant::now(), requested: false }
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
    writer.write_image_data(&bytes).map_err(|e| e.to_string())
}
