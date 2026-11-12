//! Mail in the browser: the desktop UI ([`iridium_mail_ui_egui::App`]) compiled to WebAssembly and drawn
//! into a `<canvas>` by eframe's web runner. Build and serve with Trunk (see `README.md`).
//!
//! The page opens the mailbox saved in the browser's local storage, or the synthetic sample mailbox when
//! there is none. `?reset` in the page address throws the saved mailbox away first. Nothing leaves the
//! browser: there is no mail server connection in this build.
//!
//! On the native target this binary only explains how to run the web build; the target-independent parts
//! ([`persist`], [`shell`]) are unit-tested natively.

// Native builds use these only from tests.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

mod persist;
mod shell;

#[cfg(target_arch = "wasm32")]
mod browser;

#[cfg(target_arch = "wasm32")]
fn main() {
    browser::main();
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!(
        "iridium-mail-web is the browser build of Mail.\n\
         Build and serve it with Trunk from apps/iridium-mail-web: `trunk serve` (see README.md).\n\
         For the desktop app run `cargo run -p iridium-mail`."
    );
}
