#![cfg_attr(windows, windows_subsystem = "windows")]

mod annot;
mod config;
mod geom;
mod gifenc;
mod icon;
mod image;
mod mixer;
mod palette;
mod recent;
mod regions;
mod select;
mod vedit;
mod videoplan;
mod winres;
#[cfg(windows)]
mod win;

fn main() {
    #[cfg(windows)]
    win::run();
    #[cfg(not(windows))]
    eprintln!("Snap runs on Windows.");
}
