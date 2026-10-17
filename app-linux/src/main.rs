//! Writan — native GTK4 shell for Linux (spike for ADR-0004).
//!
//! UI-thin: vault, index, trash and search all come from `writan-core`.
//! This crate is Linux-only; on other targets it compiles to a stub so
//! `cargo clippy/test --workspace` stays green on Windows, and the pure
//! modules (`spans`, `config`, `keymap`) are unit-tested everywhere.

#![forbid(unsafe_code)]

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod config;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod dates;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod keymap;
#[cfg(any(target_os = "linux", test))]
mod spans;

#[cfg(target_os = "linux")]
mod app;
#[cfg(target_os = "linux")]
mod preview;

#[cfg(target_os = "linux")]
fn main() -> gtk::glib::ExitCode {
    app::run()
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("writan-linux is a Linux-only shell (docs/adr/0004); build it on Linux or WSL.");
}
