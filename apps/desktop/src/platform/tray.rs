//! The tray icon and its menu: show/hide, quick add, Start with Windows, quit.

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// What the user picked in the tray.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAction {
    ToggleWindow,
    QuickAdd,
    ToggleAutostart,
    Quit,
}

#[derive(Clone, Debug)]
struct Ids {
    show: MenuId,
    quick_add: MenuId,
    autostart: MenuId,
    quit: MenuId,
}

impl Ids {
    fn action(&self, id: &MenuId) -> Option<TrayAction> {
        if id == &self.show {
            Some(TrayAction::ToggleWindow)
        } else if id == &self.quick_add {
            Some(TrayAction::QuickAdd)
        } else if id == &self.autostart {
            Some(TrayAction::ToggleAutostart)
        } else if id == &self.quit {
            Some(TrayAction::Quit)
        } else {
            None
        }
    }
}

/// Keeps the tray icon alive.
#[allow(missing_debug_implementations)] // TrayIcon isn't Debug.
pub struct Tray {
    _icon: TrayIcon,
    autostart: CheckMenuItem,
}

impl Tray {
    /// Reflects the Start with Windows setting in the menu.
    pub fn set_autostart_checked(&self, checked: bool) {
        self.autostart.set_checked(checked);
    }
}

/// The Quick add item advertises the shortcut only when it is actually registered; another app may own it.
pub fn quick_add_label(hotkey_registered: bool) -> &'static str {
    if hotkey_registered {
        "Quick add\tCtrl+Alt+Space"
    } else {
        "Quick add (Ctrl+Alt+Space is taken by another app)"
    }
}

/// Creates the tray icon. `on_action` runs on the UI thread, inside the message loop.
pub fn create(
    autostart_enabled: bool,
    hotkey_registered: bool,
    on_action: impl Fn(TrayAction) + Send + Sync + 'static,
) -> anyhow::Result<Tray> {
    let show = MenuItem::new("Show or hide To-do", true, None);
    let quick_add = MenuItem::new(quick_add_label(hotkey_registered), true, None);
    let autostart = CheckMenuItem::new("Start with Windows", true, autostart_enabled, None);
    let quit = MenuItem::new("Quit", true, None);
    let menu = Menu::new();
    menu.append_items(&[
        &show,
        &quick_add,
        &PredefinedMenuItem::separator(),
        &autostart,
        &PredefinedMenuItem::separator(),
        &quit,
    ])?;
    let ids = Ids {
        show: show.id().clone(),
        quick_add: quick_add.id().clone(),
        autostart: autostart.id().clone(),
        quit: quit.id().clone(),
    };

    let icon = load_icon()?;
    let tray = TrayIconBuilder::new()
        .with_tooltip("To-do")
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()?;

    let on_action = std::sync::Arc::new(on_action);
    let menu_action = on_action.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if let Some(action) = ids.action(event.id()) {
            menu_action(action);
        }
    }));
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            on_action(TrayAction::ToggleWindow);
        }
    }));
    Ok(Tray {
        _icon: tray,
        autostart,
    })
}

/// The id of the icon resource `build.rs` embeds from `assets/brand/generated/todo.ico`.
const ICON_RESOURCE: u16 = 1;

/// The 32 px render of the app icon, for when the exe has no icon resource (a test binary, say).
const FALLBACK_PNG: &[u8] = include_bytes!("../../../../assets/brand/generated/icon-32.png");

/// The tray's icon size in pixels: 16 at 100 % scaling, 24 at 150 %, 32 at 200 %. The .ico holds a hand-tuned
/// image for each, so Windows never has to scale one.
#[allow(unsafe_code)] // One Win32 metrics call.
fn tray_icon_size() -> u32 {
    // SAFETY: GetSystemMetrics has no preconditions.
    let size = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
            windows_sys::Win32::UI::WindowsAndMessaging::SM_CXSMICON,
        )
    };
    u32::try_from(size).ok().filter(|s| *s > 0).unwrap_or(16)
}

/// The app icon from the exe's resource at the tray's size, or the embedded 32 px render.
fn load_icon() -> anyhow::Result<Icon> {
    let size = tray_icon_size();
    match Icon::from_resource(ICON_RESOURCE, Some((size, size))) {
        Ok(icon) => Ok(icon),
        Err(error) => {
            tracing::debug!("no icon resource ({error}); using the embedded render");
            let (rgba, width, height) = fallback_rgba()?;
            Ok(Icon::from_rgba(rgba, width, height)?)
        }
    }
}

/// The embedded render as RGBA pixels, with its width and height.
fn fallback_rgba() -> anyhow::Result<(Vec<u8>, u32, u32)> {
    let image =
        image::load_from_memory_with_format(FALLBACK_PNG, image::ImageFormat::Png)?.into_rgba8();
    let (width, height) = image.dimensions();
    Ok((image.into_raw(), width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_render_has_transparent_corners_and_an_opaque_centre() {
        let (px, width, height) = fallback_rgba().expect("decodes");
        assert_eq!((width, height), (32, 32));
        let alpha = |x: usize, y: usize| px[(y * 32 + x) * 4 + 3];
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(31, 31), 0);
        assert_eq!(alpha(16, 16), 255);
    }

    #[test]
    fn the_ico_has_an_image_for_every_scale() {
        // The window class and the tray ask Windows for 16, 24 (150 %), 32 (200 %) and 48 px images.
        let ico = include_bytes!("../../../../assets/brand/generated/todo.ico");
        assert_eq!(u16::from_le_bytes([ico[2], ico[3]]), 1, "an icon file");
        let count = usize::from(u16::from_le_bytes([ico[4], ico[5]]));
        let sizes: Vec<u32> = (0..count)
            .map(|i| match ico[6 + 16 * i] {
                0 => 256,
                n => u32::from(n),
            })
            .collect();
        assert_eq!(sizes, [16, 24, 32, 48, 64, 256]);
    }

    #[test]
    fn the_menu_only_promises_a_registered_shortcut() {
        assert!(quick_add_label(true).ends_with("Ctrl+Alt+Space"));
        assert!(quick_add_label(true).contains("\tCtrl"));
        assert!(!quick_add_label(false).contains("\tCtrl"));
        assert!(quick_add_label(false).contains("taken"));
    }
}
