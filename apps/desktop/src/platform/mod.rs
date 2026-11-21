//! Windows integration: the tray, the global hotkey, single instance, Start with Windows and window
//! visibility. The app targets Windows for M1; see `docs/desktop.md`.

pub mod autostart;
pub mod dpapi;
pub mod hotkey;
pub mod icon;
pub mod single_instance;
pub mod tray;
pub mod win;
