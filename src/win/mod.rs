mod app;
mod card;
mod clipboard;
mod d2d;
mod editor;
mod elements;
mod files;
mod grab;
mod hook;
pub mod mp4read;
mod overlay;
mod paths;
mod recbar;
mod record;
mod toolstrip;
mod tray;
mod util;
mod vaudio;
mod vexport;
mod veditor;
mod vplayer;
mod wins;

pub use app::run;

use windows::Win32::UI::WindowsAndMessaging::WM_APP;

/// Start a capture (posted by the hook, the tray and second instances).
pub const WM_APP_CAPTURE: u32 = WM_APP + 1;
pub const WM_APP_TRAY: u32 = WM_APP + 2;
/// A key routed to the overlay by the hook. wParam = vk, lParam = shift held.
pub const WM_APP_KEY: u32 = WM_APP + 3;
/// The element worker has an answer for the overlay.
pub const WM_APP_ELEMENTS: u32 = WM_APP + 4;
/// GIF export progress: wParam = percent, 1000 = done, 1001 = failed.
pub const WM_APP_GIF: u32 = WM_APP + 5;
/// Open the video handed over by another instance.
pub const WM_APP_EDIT: u32 = WM_APP + 6;
/// The editor changed the image: copy it after a short pause.
pub const AUTOCOPY_TIMER: usize = 7;

pub mod class_name {
    use windows::core::{PCWSTR, w};
    pub const MSG: PCWSTR = w!("RengiSnapMsg");
    pub const OVERLAY: PCWSTR = w!("RengiSnapOverlay");
    pub const CARD: PCWSTR = w!("RengiSnapCard");
    pub const BAR: PCWSTR = w!("RengiSnapBar");
    pub const OUTLINE: PCWSTR = w!("RengiSnapOutline");
    pub const EDITOR: PCWSTR = w!("RengiSnapEditor");
    pub const VEDITOR: PCWSTR = w!("RengiSnapVideoEditor");
}
