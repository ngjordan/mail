//! Mail desktop shell: the app's window logic as a library.
//!
//! The app binary (`apps/iridium-mail`) and the Office suite both drive this. The shape
//! is the one the other five apps use: a [`Launch`] says what to open, and the [`Shell`]
//! exposes the same handful of methods, so a host can build the module later and call
//! its background work every frame while it draws only when it is the active one.
//!
//! The platform glue (file dialogs, clipboard, screen capture, speech, the installed-font
//! index) lives here too: it is installed into the UI, and the UI is built here.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Instant;

use iridium_mail_engine::Session;
use iridium_mail_ui_egui::App;

pub mod capture;
pub mod clipboard;
pub mod file_dialogs;
pub mod installed_fonts;
pub mod speech;

/// When the process started (the start-up times in the troubleshooting log).
static STARTED: OnceLock<Instant> = OnceLock::new();

/// Note that the process started. The binary calls this first thing.
pub fn mark_started() {
    STARTED.get_or_init(Instant::now);
}

/// Milliseconds since the process started.
pub fn since_start() -> u128 {
    STARTED.get().map_or(0, |t| t.elapsed().as_millis())
}

/// How the app was started.
#[derive(Default)]
pub struct Launch {
    /// Files to open, already routed to this app.
    pub files: Vec<String>,
    /// The profile folder. `None` = the default folder for the platform.
    pub profile: Option<PathBuf>,
    /// The storage a new profile gets. `None` = what the environment asks for.
    pub store: Option<iridium_mail_engine::StoreKind>,
    /// Open the in-memory sample mailbox instead of a profile.
    pub sample: bool,
    /// The control channel's port. `None` = the control channel is off.
    pub control_port: Option<u16>,
    /// A `mailto:` URL to open in a new message.
    pub mailto: Option<String>,
}

/// The app, its session, and the platform glue installed into it.
pub struct Shell {
    app: App,
    /// The fonts the app installed, for a host that has to put them back.
    fonts: egui::FontDefinitions,
    /// The first frame is still to come (its time goes to the troubleshooting log).
    first_frame: bool,
}

impl Shell {
    /// Build the shell for a binary that owns the event loop.
    pub fn new(cc: &eframe::CreationContext<'_>, launch: Launch) -> Self {
        Self::new_for_host(&cc.egui_ctx, cc.storage, launch)
    }

    /// Host-facing constructor: no `CreationContext`, so a host can build this module
    /// later. `storage` is unused: Mail keeps its settings in the profile, not in
    /// eframe's storage.
    pub fn new_for_host(ctx: &egui::Context, _storage: Option<&dyn eframe::Storage>, launch: Launch) -> Self {
        let mut session = open_session(&launch);
        // Quick Print goes straight to the default printer in the desktop app.
        session.direct_print = true;
        let mut app = App::new(session);
        if let Some(url) = &launch.mailto {
            app.run("mail.newFromMailto", serde_json::json!({ "url": url }));
        }
        file_dialogs::install(&mut app);
        clipboard::install();
        installed_fonts::install();
        capture::install();
        speech::install();
        app.ui.control_port = launch.control_port;
        // The fonts the app installs, for a host that has to put them back after another
        // module has drawn with its own.
        App::install_fonts(ctx);
        let fonts = iridium_mail_ui_egui::font_definitions();
        // The files named on the command line (a double-click on Windows and Linux) open
        // as File › Open & Export › Import opens them.
        let files = launch.files.clone();
        let mut shell = Self { app, fonts, first_frame: true };
        for f in &files {
            shell.open_path(f);
        }
        shell
    }

    /// Background work. Runs every frame for every started module: the background sync
    /// and the account polling live here.
    pub fn logic(&mut self, ctx: &egui::Context) {
        self.app.poll_sync(ctx);
    }

    /// Raw input. Mail has none: the keys go through egui.
    pub fn raw_input_hook(&mut self, _ctx: &egui::Context, _raw: &mut egui::RawInput) {}

    /// Draw. Runs only for the active module.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let first = std::mem::take(&mut self.first_frame);
        let start = since_start();
        self.app.ui_root(ui);
        if first {
            log::debug!(
                "start-up: first frame at {start} ms, drawn in {} ms",
                since_start().saturating_sub(start)
            );
        }
    }

    /// The window's backdrop colour.
    pub fn backdrop(&self) -> [f32; 4] {
        self.app.theme.backdrop.to_normalized_gamma_f32()
    }

    /// Open a file. The type comes from the extension, the same way File › Open & Export
    /// › Import works it out.
    pub fn open_path(&mut self, path: &str) {
        self.app.run("file.openExport", serde_json::json!({ "action": "import", "path": path }));
    }

    /// Persist what outlives the window: queued changes folded into the profile's
    /// snapshot, and the data files kept in files of their own.
    pub fn save_state(&mut self) {
        save_on_exit(&mut self.app.s);
    }

    /// Whether the module is safe to close. Mail saves on exit and keeps the draft in the
    /// session, so it is.
    pub fn can_close(&self) -> bool {
        true
    }

    /// The zoom the app uses.
    ///
    /// Mail's interface has no zoom of its own, so it is always the default one.
    pub fn zoom(&self) -> f32 {
        1.0
    }

    /// The font set the app installed.
    pub fn fonts(&self) -> egui::FontDefinitions {
        self.fonts.clone()
    }

    /// Re-install this app's native menu (macOS). Mail has none: its commands are in the
    /// ribbon, like the web build.
    pub fn install_native_menu(&mut self) {}

    /// The app's session, for a host that needs to run a command.
    pub fn session(&self) -> &Session {
        &self.app.s
    }
}

/// Opens the session; a profile that cannot be opened falls back to an empty in-memory
/// mailbox with the reason shown as an error notice.
fn open_session(launch: &Launch) -> Session {
    if launch.sample {
        let now = iridium_mail_engine::model::DateTime::now_utc();
        return iridium_mail_engine::sample::sample_session_at(now);
    }
    let Some(dir) = launch.profile.clone().or_else(default_profile) else {
        let mut s = Session::new(iridium_mail_engine::model::Mailbox::new_local(
            iridium_mail_engine::model::mailbox::LOCAL_ACCOUNT_FALLBACK_NAME,
        ));
        s.notify(iridium_mail_engine::Notice::Error(
            "No data folder could be found (HOME is not set); changes will not be saved. Use --profile <dir>."
                .into(),
        ));
        return s;
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("cannot create {}: {e}", dir.display());
    }
    match Session::open_with(&dir, launch.store.unwrap_or_else(iridium_mail_engine::StoreKind::from_env)) {
        Ok(mut s) => {
            // File › Options › Advanced › "Enable troubleshooting logging" (read at start-up).
            match iridium_mail_engine::troubleshoot::start(&s.settings, &dir) {
                Ok(Some(path)) => log::info!("troubleshooting log: {}", path.display()),
                Ok(None) => {}
                Err(e) => log::warn!("troubleshooting log: {e}"),
            }
            // Account secrets live in the OS credential store where there is one; otherwise
            // the profile's unencrypted secrets file stays in use (Account Settings says so).
            match s.use_os_keyring() {
                Ok(at) => log::info!("account secrets: {at}"),
                Err(e) => log::warn!("OS keyring not used, secrets stay in the profile file: {e}"),
            }
            s
        }
        Err(e) => {
            let mut s = Session::new(iridium_mail_engine::model::Mailbox::new_local(
                iridium_mail_engine::model::mailbox::LOCAL_ACCOUNT_FALLBACK_NAME,
            ));
            s.notify(iridium_mail_engine::Notice::Error(format!("{e}. Changes will not be saved.")));
            s
        }
    }
}

/// Writes everything that outlives the window: queued changes folded into the profile's
/// snapshot, and the data files kept in files of their own. A session without a profile
/// (the sample, or a profile that could not be opened) still writes those data files, or
/// what was added to them would be lost.
fn save_on_exit(s: &mut Session) {
    let own_files = s.mailbox.accounts.iter().any(|a| a.data_file.is_some());
    if (s.store.is_some() || own_files)
        && let Err(e) = s.run("file.exit", &serde_json::json!({}))
    {
        log::error!("saving on exit failed: {e}");
    }
}

/// The folder the profile used before the rename. A profile written then is still the
/// user's profile: the folder is where their mail, drafts and rules live.
pub const PROFILE_FOLDER_LEGACY: &str = "MailCraft";

/// The profile folder: the platform's data folder, under the app's own name.
///
/// The folder before the rename is used when the new one does not exist, so a rename
/// does not silently start a new, empty profile.
pub fn default_profile() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = if cfg!(target_os = "windows") {
        env("APPDATA").or_else(|| env("USERPROFILE").map(|h| h.join("AppData").join("Roaming")))?
    } else if cfg!(target_os = "macos") {
        env("HOME")?.join("Library").join("Application Support")
    } else {
        env("XDG_DATA_HOME")
            .filter(|p| p.is_absolute())
            .or_else(|| env("HOME").map(|h| h.join(".local").join("share")))?
    };
    let current = base.join("iridium-mail");
    if current.exists() {
        return Some(current);
    }
    let legacy = base.join(PROFILE_FOLDER_LEGACY);
    if legacy.exists() {
        return Some(legacy);
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_the_window_writes_data_files_of_their_own_without_a_profile() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Projects.mcdata");
        let mut s = Session::new(iridium_mail_engine::model::Mailbox::new_local("Data"));
        assert!(s.store.is_none());
        let r = s.run("file.newDataFile", &serde_json::json!({ "path": file.to_str().unwrap() })).unwrap();
        let id =
            iridium_mail_engine::model::AccountId(u32::try_from(r["account"].as_u64().unwrap()).unwrap());
        let inbox = s.mailbox.role_folder(id, iridium_mail_engine::model::FolderRole::Inbox).unwrap();
        s.mailbox.add_mail(iridium_mail_engine::model::MailItem {
            folder: inbox,
            subject: "Added after it was made".into(),
            ..Default::default()
        });
        save_on_exit(&mut s);
        let saved = String::from_utf8(std::fs::read(&file).unwrap()).unwrap();
        assert!(saved.contains("Added after it was made"), "what was added reaches the file");
    }

    #[test]
    fn the_profile_folder_is_the_apps_own() {
        if let Some(p) = default_profile() {
            assert!(p.ends_with("iridium-mail"), "{p:?}");
        }
    }
}
