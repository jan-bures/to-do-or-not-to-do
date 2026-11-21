//! The global quick-add hotkey, Ctrl+Alt+Space.

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

/// The quick-add hotkey.
pub fn quick_add_hotkey() -> HotKey {
    HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::Space)
}

/// Keeps the hotkey registered while alive.
#[allow(missing_debug_implementations)] // GlobalHotKeyManager isn't Debug.
pub struct Hotkey {
    _manager: GlobalHotKeyManager,
}

/// Registers Ctrl+Alt+Space and calls `on_press` on the UI thread each time it is pressed.
///
/// The handler runs inside the message loop, straight from `WM_HOTKEY`, so it can show the quick-add window
/// without waiting for GPUI's next frame. It fails when another app already owns the combination.
pub fn register(on_press: impl Fn() + Send + Sync + 'static) -> anyhow::Result<Hotkey> {
    let manager = GlobalHotKeyManager::new()?;
    let hotkey = quick_add_hotkey();
    manager.register(hotkey)?;
    let id = hotkey.id();
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.id == id && event.state == HotKeyState::Pressed {
            on_press();
        }
    }));
    Ok(Hotkey { _manager: manager })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_quick_add_hotkey_is_ctrl_alt_space() {
        let hk = quick_add_hotkey();
        assert_eq!(hk.key, Code::Space);
        assert_eq!(hk.mods, Modifiers::CONTROL | Modifiers::ALT);
        assert_eq!(hk.into_string(), "control+alt+Space");
    }
}
