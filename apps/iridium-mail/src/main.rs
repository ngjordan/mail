//! Mail desktop app: an eframe window around the shell.
//!
//! ```text
//! iridium-mail [--profile <dir>] [--store legacy|sqlite] [--sample] [--control-port <N>] [--version] [--help]
//!              [mailto:…]
//! ```
//!
//! A `mailto:` URL (RFC 6068) opens a new message to it (`mail.newFromMailto`). With File › Options ›
//! Advanced › "Enable troubleshooting logging" on, the profile's `logs/iridium-mail.log` records what the app
//! does (`iridium_mail_engine::troubleshoot`; size-capped, no secrets).
//!
//! The profile (mail store) defaults to the platform data folder: `$XDG_DATA_HOME/iridium-mail` or
//! `~/.local/share/iridium-mail` on Linux and other Unix systems, `~/Library/Application Support/iridium-mail` on
//! macOS, `%APPDATA%\iridium-mail` on Windows.
//!
//! `IRIDIUM_MAIL_RENDERER=wgpu` draws with wgpu (Direct3D 12 on Windows, Vulkan or Metal elsewhere) instead of
//! OpenGL, the default.
//!
//! The window's logic lives in [`iridium_mail_shell`], so the Office suite can host the app as one of its
//! modules. What stays here is the process: the arguments, the PST worker mode, the renderer and the window.

#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::process::ExitCode;

use iridium_mail_shell::{Launch, Shell};

const USAGE: &str = "\
Usage: iridium-mail [OPTIONS]

Options:
  --profile <DIR>       Data folder to open (created when missing)
  --store <KIND>        Storage of a new data folder: legacy (default) or sqlite; an existing folder
                        keeps its own (also IRIDIUM_MAIL_STORE=sqlite)
  --sample              Open an in-memory sample mailbox instead of a profile
  --control-port <N>    Accept control commands on 127.0.0.1:<N> (for tests and automation)
  --version             Print the version and exit
  -h, --help            Print this help and exit

Arguments:
  mailto:<address>      Open a new message (RFC 6068 mailto: link)

Environment:
  IRIDIUM_MAIL_RENDERER    opengl (default) or wgpu (Direct3D 12 on Windows, Vulkan or Metal elsewhere)
";

/// `git <hash>, <date>` from `build.rs` (`unknown` when the build had neither git nor the variables).
const BUILD_INFO: &str =
    concat!("git ", env!("IRIDIUM_MAIL_GIT_HASH"), ", ", env!("IRIDIUM_MAIL_BUILD_DATE"));

/// `--version` output: `iridium-mail X.Y.Z (git <hash>, <date>)`.
fn version_line() -> String {
    format!("iridium-mail {} ({BUILD_INFO})", env!("CARGO_PKG_VERSION"))
}

/// Parsed command line.
#[derive(Debug, Default, PartialEq)]
struct Args {
    profile: Option<PathBuf>,
    /// `--store`: the storage a new profile gets (`None`: `IRIDIUM_MAIL_STORE`, else legacy).
    store: Option<iridium_mail_engine::StoreKind>,
    sample: bool,
    control_port: Option<u16>,
    version: bool,
    help: bool,
    /// A `mailto:` link to open as a new message.
    mailto: Option<String>,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut out = Args::default();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let (flag, inline) = match a.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_owned(), Some(v.to_owned())),
            _ => (a.clone(), None),
        };
        let mut value =
            |name: &str| inline.clone().or_else(|| it.next()).ok_or_else(|| format!("{name} needs a value"));
        match flag.as_str() {
            "--profile" => out.profile = Some(PathBuf::from(value("--profile")?)),
            "--store" => {
                let v = value("--store")?;
                out.store = Some(
                    iridium_mail_engine::StoreKind::parse(&v)
                        .ok_or_else(|| format!("--store: `{v}` is not legacy or sqlite"))?,
                );
            }
            "--sample" => out.sample = true,
            "--control-port" => {
                let v = value("--control-port")?;
                let port =
                    v.parse::<u16>().map_err(|_| format!("--control-port: `{v}` is not a port number"))?;
                if port == 0 {
                    return Err("--control-port: port must be 1-65535".into());
                }
                out.control_port = Some(port);
            }
            "--version" | "-V" => out.version = true,
            "--help" | "-h" => out.help = true,
            m if m.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("mailto:")) => {
                out.mailto = Some(a.clone())
            }
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    Ok(out)
}

/// The renderer `IRIDIUM_MAIL_RENDERER` asks for: wgpu, else OpenGL.
fn renderer(var: Option<&str>) -> eframe::Renderer {
    match var.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("wgpu" | "d3d12" | "dx12" | "vulkan" | "metal") => eframe::Renderer::Wgpu,
        _ => eframe::Renderer::Glow,
    }
}

/// eframe adapter: the shell, wrapped.
struct Mail(Shell);

impl eframe::App for Mail {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.0.logic(ctx);
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.0.raw_input_hook(ctx, raw);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.ui(ui);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.0.save_state();
    }

    fn clear_color(&self, _visuals: &eframe::egui::Visuals) -> [f32; 4] {
        self.0.backdrop()
    }
}

fn main() -> ExitCode {
    iridium_mail_shell::mark_started();
    // PST files are read in a process of their own (see `iridium_mail_engine::pst`): this program, started
    // with `--pst-worker FILE`, is that reader.
    let argv: Vec<String> = std::env::args().collect();
    if argv.get(1).map(String::as_str) == Some(iridium_mail_engine::pst::WORKER_ARG) {
        let code = iridium_mail_engine::pst::worker_main(argv.get(2..).unwrap_or(&[]));
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }
    if let Ok(exe) = std::env::current_exe() {
        iridium_mail_engine::pst::set_worker(exe);
    }
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("iridium-mail: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if args.help {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if args.version {
        println!("{}", version_line());
        return ExitCode::SUCCESS;
    }
    iridium_mail_ui_egui::backstage::set_build_info(BUILD_INFO);

    let launch = Launch {
        files: Vec::new(),
        profile: args.profile,
        store: args.store,
        sample: args.sample,
        control_port: args.control_port.or_else(|| {
            std::env::var("IRIDIUM_MAIL_CONTROL_PORT")
                .ok()
                .and_then(|v| v.trim().parse::<u16>().ok())
                .filter(|p| *p != 0)
        }),
        mailto: args.mailto,
    };

    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title(iridium_mail_ui_egui::compose::MAIN_WINDOW_TITLE)
        .with_app_id("ph.mail")
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size(iridium_mail_ui_egui::theme::size::MIN_WINDOW)
        .with_decorations(true);
    if let Some((rgba, width, height)) = iridium_mail_ui_egui::app_icon_rgba(256) {
        viewport = viewport.with_icon(eframe::egui::IconData { rgba, width, height });
    }
    let renderer = renderer(std::env::var("IRIDIUM_MAIL_RENDERER").ok().as_deref());
    let options = eframe::NativeOptions { viewport, renderer, ..Default::default() };
    log::debug!(
        "start-up: opening the window at {} ms ({renderer:?} renderer)",
        iridium_mail_shell::since_start()
    );
    let result = eframe::run_native(
        "Mail",
        options,
        Box::new(move |cc| {
            log::debug!("start-up: window and renderer ready at {} ms", iridium_mail_shell::since_start());
            let shell = Shell::new(cc, launch);
            log::debug!("start-up: fonts installed at {} ms", iridium_mail_shell::since_start());
            Ok(Box::new(Mail(shell)))
        }),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("iridium-mail: could not open the window: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_renderer_is_opengl_unless_wgpu_is_asked_for() {
        use eframe::Renderer;
        assert_eq!(super::renderer(None), Renderer::Glow);
        assert_eq!(super::renderer(Some("opengl")), Renderer::Glow);
        assert_eq!(super::renderer(Some("nonsense")), Renderer::Glow);
        assert_eq!(super::renderer(Some(" WGPU ")), Renderer::Wgpu);
        assert_eq!(super::renderer(Some("dx12")), Renderer::Wgpu);
    }

    use super::*;

    fn parse(v: &[&str]) -> Result<Args, String> {
        parse_args(v.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn flags() {
        assert_eq!(parse(&[]).unwrap(), Args::default());
        let a = parse(&["--profile", "/tmp/x", "--sample", "--control-port", "4321"]).unwrap();
        assert_eq!(a.profile, Some(PathBuf::from("/tmp/x")));
        assert!(a.sample);
        assert_eq!(a.control_port, Some(4321));
        assert_eq!(parse(&["--control-port=99"]).unwrap().control_port, Some(99));
        assert!(parse(&["--version"]).unwrap().version);
        assert!(parse(&["--control-port", "0"]).is_err());
        assert!(parse(&["--control-port", "x"]).is_err());
        assert!(parse(&["--profile"]).is_err());
        assert_eq!(
            parse(&["--store", "sqlite"]).unwrap().store,
            Some(iridium_mail_engine::StoreKind::Sqlite)
        );
        assert_eq!(parse(&["--store=legacy"]).unwrap().store, Some(iridium_mail_engine::StoreKind::Legacy));
        assert!(parse(&["--store", "lmdb"]).is_err());
        assert!(parse(&["--store"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
        let m = parse(&["--sample", "mailto:ann@example.org?subject=Hi"]).unwrap();
        assert_eq!(m.mailto.as_deref(), Some("mailto:ann@example.org?subject=Hi"));
        assert!(parse(&["ann@example.org"]).is_err(), "only mailto: links are arguments");
    }

    #[test]
    fn version_line_names_version_and_build() {
        let v = version_line();
        assert!(v.starts_with(&format!("iridium-mail {} (git ", env!("CARGO_PKG_VERSION"))), "{v}");
        assert!(v.ends_with(')'), "{v}");
    }
}
