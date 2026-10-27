#![cfg_attr(windows, windows_subsystem = "windows")]

mod config;
mod geom;
mod icon;
mod image;
mod regions;
mod select;
#[cfg(windows)]
mod win;

fn main() {
    #[cfg(windows)]
    win::run();
    #[cfg(not(windows))]
    eprintln!("Rengi Snap runs on Windows.");
}
