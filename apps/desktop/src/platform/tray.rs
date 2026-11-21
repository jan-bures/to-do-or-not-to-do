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

    let icon = Icon::from_rgba(icon_rgba(ICON_SIZE), ICON_SIZE, ICON_SIZE)?;
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

const ICON_SIZE: u32 = 32;

/// The app mark, drawn in code: the sun over two hills on My Day's light sky, in a rounded square. The
/// colours are the design tokens'.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    let p = todo_design::palette(todo_design::Theme::Light, todo_design::hues::MY_DAY);
    let rgb = |c: todo_design::Rgb| [f32::from(c.r), f32::from(c.g), f32::from(c.b)];
    let (sky_c, sun_c, far_c, near_c) = (rgb(p.sky), rgb(p.sun), rgb(p.hill_far), rgb(p.sun_ink));
    const SAMPLES: u32 = 4;
    let s = size as f32;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            let (mut acc, mut cover) = ([0.0f32; 3], 0.0f32);
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    // Unit coordinates of the sub-sample.
                    let x = (px as f32 + (sx as f32 + 0.5) / SAMPLES as f32) / s;
                    let y = (py as f32 + (sy as f32 + 0.5) / SAMPLES as f32) / s;
                    if !in_rounded_square(x, y, 0.22) {
                        continue;
                    }
                    let near = y > 0.74 + 0.05 * (x * 5.2 + 0.6).sin();
                    let far = y > 0.62 + 0.06 * (x * 4.1 + 2.2).sin();
                    let sun = (x - 0.56).powi(2) + (y - 0.46).powi(2) < 0.16f32.powi(2);
                    let color = if near {
                        near_c
                    } else if far {
                        far_c
                    } else if sun {
                        sun_c
                    } else {
                        sky_c
                    };
                    for (a, c) in acc.iter_mut().zip(color) {
                        *a += c;
                    }
                    cover += 1.0;
                }
            }
            let n = (SAMPLES * SAMPLES) as f32;
            let alpha = cover / n;
            for a in acc {
                out.push(if cover > 0.0 {
                    (a / cover).round() as u8
                } else {
                    0
                });
            }
            out.push((alpha * 255.0).round() as u8);
        }
    }
    out
}

fn in_rounded_square(x: f32, y: f32, r: f32) -> bool {
    let cx = x.clamp(r, 1.0 - r);
    let cy = y.clamp(r, 1.0 - r);
    (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_is_rgba_with_transparent_corners_and_an_opaque_centre() {
        let px = icon_rgba(32);
        assert_eq!(px.len(), 32 * 32 * 4);
        let alpha = |x: usize, y: usize| px[(y * 32 + x) * 4 + 3];
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(31, 31), 0);
        assert_eq!(alpha(16, 16), 255);
    }

    #[test]
    fn the_menu_only_promises_a_registered_shortcut() {
        assert!(quick_add_label(true).ends_with("Ctrl+Alt+Space"));
        assert!(quick_add_label(true).contains("\tCtrl"));
        assert!(!quick_add_label(false).contains("\tCtrl"));
        assert!(quick_add_label(false).contains("taken"));
    }
}
