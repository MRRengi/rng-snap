#![cfg_attr(windows, windows_subsystem = "windows")]

mod config;
mod geom;
mod gifenc;
mod icon;
mod image;
mod mixer;
mod regions;
mod select;
mod videoplan;
#[cfg(windows)]
mod win;

fn main() {
    #[cfg(windows)]
    win::run();
    #[cfg(not(windows))]
    eprintln!("Rengi Snap runs on Windows.");
}
