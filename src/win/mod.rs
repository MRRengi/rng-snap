mod app;
mod card;
mod clipboard;
mod d2d;
mod files;
mod grab;
mod hook;
mod overlay;
mod paths;
mod tray;
mod util;
mod wins;

pub use app::run;

use windows::Win32::UI::WindowsAndMessaging::WM_APP;

/// Start a capture (posted by the hook, the tray and second instances).
pub const WM_APP_CAPTURE: u32 = WM_APP + 1;
pub const WM_APP_TRAY: u32 = WM_APP + 2;
/// A key routed to the overlay by the hook. wParam = vk, lParam = shift held.
pub const WM_APP_KEY: u32 = WM_APP + 3;

pub mod class_name {
    use windows::core::{PCWSTR, w};
    pub const MSG: PCWSTR = w!("RengiSnapMsg");
    pub const OVERLAY: PCWSTR = w!("RengiSnapOverlay");
    pub const CARD: PCWSTR = w!("RengiSnapCard");
}
