//! Developer aid: `S_TERM_SCREENSHOT=out.png [S_TERM_DEMO=ai,split,settings,palette,files,launch,folder=/some/dir] s-term`
//! sets up the requested views, saves a screenshot of the window as PNG and exits.

use std::path::PathBuf;

use super::STerm;
use crate::input::Action;
use crate::sidebar::Tab as View;

pub struct DevShot {
    path: PathBuf,
    demo: Vec<String>,
    frame: u32,
    requested: bool,
}

impl DevShot {
    pub fn from_env() -> Option<Self> {
        let path = std::env::var_os("S_TERM_SCREENSHOT")?;
        let demo = std::env::var("S_TERM_DEMO").unwrap_or_default().split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        Some(Self { path: path.into(), demo, frame: 0, requested: false })
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (n, t) in table.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb88320 ^ (c >> 1) } else { c >> 1 };
        }
        *t = c;
    }
    !data.iter().fold(!0u32, |c, &b| table[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8))
}

/// Minimal PNG writer (RGBA8, stored/uncompressed deflate).
fn write_png(path: &std::path::Path, w: usize, h: usize, rgba: &[u8]) -> std::io::Result<()> {
    let mut raw = Vec::with_capacity((w * 4 + 1) * h);
    for row in rgba.chunks(w * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = vec![0x78, 0x01];
    let chunks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, c) in chunks.iter().enumerate() {
        z.push(u8::from(i + 1 == chunks.len()));
        let len = c.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(c);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut chunk = |kind: &[u8], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    std::fs::write(path, out)
}

/// Sample launchers for screenshots (the config is never saved in screenshot mode).
fn demo_launchers() -> Vec<crate::config::Launcher> {
    use crate::config::{Launcher, RunIn};
    let here = std::env::current_dir().map(|d| crate::launchpad::tilde(&d)).unwrap_or_default();
    let l = |id, name: &str, dir: &str, cmds: &[&str], run_in| Launcher { id, name: name.into(), dir: dir.into(), commands: cmds.iter().map(|c| c.to_string()).collect(), run_in };
    vec![
        l(1, "Dev server", &here, &["npm install", "npm run dev"], RunIn::Tab),
        l(2, "Release build", &here, &["cargo build --release", "./scripts/install.sh"], RunIn::Tab),
        l(3, "Ping localhost", "", &["ping -i 2 127.0.0.1"], RunIn::Split),
    ]
}

impl STerm {
    pub(super) fn devshot(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.devshot.take() else { return };
        d.frame += 1;
        ctx.request_repaint();
        if d.frame == 3 {
            for step in d.demo.clone() {
                match step.as_str() {
                    "ai" => self.sidebar.show_view(View::Ai),
                    "changes" => self.sidebar.show_view(View::Changes),
                    "files" => self.sidebar.show_view(View::Files),
                    "launch" => self.sidebar.show_view(View::Launch),
                    "history" => self.sidebar.show_view(View::History),
                    // Sample History entries (in memory), and the startup popup with `restoreoffer`.
                    "historydemo" => {
                        self.history_demo(false);
                        self.sidebar.show_view(View::History);
                    }
                    "restoreoffer" => self.history_demo(true),
                    "launchdemo" => {
                        self.config.launchers = demo_launchers();
                        self.sidebar.show_view(View::Launch);
                    }
                    "launchnew" => {
                        let cwd = self.focused_cwd();
                        self.sidebar.new_launcher(cwd);
                    }
                    // Runs the harmless sample launcher (a ping in a split); needs `launchdemo` first.
                    "launchrun" => self.launch_op(3, crate::launchpad::Op::Run),
                    "settings" => self.settings_open = true,
                    "palette" => self.palette.toggle(),
                    "setup" => self.setup = Some(Default::default()),
                    s if s.starts_with("setup=") => self.setup = Some(super::setup::Setup::at(s[6..].parse().unwrap_or(0))),
                    "ask" => {
                        self.ask_demo();
                        self.palette.open_ask();
                    }
                    "commands" => self.palette.show_commands(),
                    s if s.starts_with("asktyped=") => {
                        self.palette.open_commands();
                        self.palette.ask_first = true;
                        self.palette.set_query(&s[9..]);
                    }
                    "split" => self.do_action(Action::SplitSideBySide, ctx),
                    "splitdown" => self.do_action(Action::SplitStacked, ctx),
                    "tab" => self.do_action(Action::NewTab, ctx),
                    "zoom" => self.do_action(Action::FontBigger, ctx),
                    s if s.starts_with("view=") => self.sidebar.open_preview(PathBuf::from(&s[5..])),
                    s if s.starts_with("folder=") => crate::sidebar::Sidebar::add_folder(&mut self.config, PathBuf::from(&s[7..])),
                    s if s.starts_with("theme=") => self.config.theme = s[6..].to_string(),
                    s if s.starts_with("section=") => self.settings_section_by_name(&s[8..]),
                    _ => {}
                }
            }
        }
        if d.frame == 90 && !d.requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
            d.requested = true;
        }
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = shot {
            let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            match write_png(&d.path, img.size[0], img.size[1], &rgba) {
                Ok(()) => eprintln!("s-term: screenshot saved to {}", d.path.display()),
                Err(e) => eprintln!("s-term: screenshot failed: {e}"),
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        self.devshot = Some(d);
    }
}
