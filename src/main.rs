#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod auth;
mod launcher;
mod loaders;
mod settings;
mod ui;

fn main() {
    ui::run();
}
