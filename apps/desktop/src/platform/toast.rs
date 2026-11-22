//! Windows toast notifications for reminders (WinRT `ToastNotification`), and the app identity
//! Windows shows them under.
//!
//! An unpackaged app needs an AppUserModelID (AUMID) for toasts. The Velopack installer's Start menu
//! shortcut carries [`AUMID`] (`vpk pack --aumid`, see `scripts/package.ps1`), and `VelopackApp::run`
//! sets the same one on an installed process. [`set_identity`] sets it on every process too (a build
//! run from the repo has no shortcut), and registers it per user under
//! `HKCU\Software\Classes\AppUserModelId\<AUMID>` with the app's name and icon, which is what Windows
//! shows on a toast when no shortcut names it. Nothing needs admin rights; uninstalling removes the key
//! ([`unregister`]).
//!
//! The toasts' buttons are handled in process: Windows raises `Activated` on a thread of its own, and
//! the arguments go to the UI thread over a channel. A toast clicked after the app has quit does
//! nothing, so the app takes its reminder toasts away when it quits.

#![allow(unsafe_code)] // One Win32 call (`SetCurrentProcessExplicitAppUserModelID`), commented.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use futures::channel::mpsc::UnboundedSender;
use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::{
    ToastActivatedEventArgs, ToastNotification, ToastNotificationManager, ToastNotifier,
};
use windows::core::{HSTRING, IInspectable, Interface as _, h};

use crate::model::reminders::{GROUP, Toast, ToastSink};

/// The app's AUMID: the one Velopack gives an installed copy (`velopack.<packId>`), so an installed
/// process, its shortcut and a build run from the repo all share it.
pub const AUMID: &str = "velopack.todo-desktop";
/// The name Windows shows on the app's toasts.
pub const DISPLAY_NAME: &str = "To-do or not to do";
/// The per-user registration of [`AUMID`].
pub fn registry_key() -> String {
    format!(r"Software\Classes\AppUserModelId\{AUMID}")
}

const ICON_PNG: &[u8] = include_bytes!("../../../../assets/brand/generated/icon-128.png");
const ICON_FILE: &str = "toast-icon.png";

/// Gives this process the app's AUMID and registers it for the current user, with the icon written to
/// `icon_dir`. Call before any window opens (the taskbar groups windows by it). Failures are logged:
/// toasts may then show under a generic name, or not at all.
pub fn set_identity(icon_dir: Option<&Path>) {
    let id = HSTRING::from(AUMID);
    // SAFETY: `id` is a valid, NUL-terminated wide string that outlives the call.
    if let Err(e) =
        unsafe { windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(&id) }
    {
        tracing::warn!("setting the app's AUMID failed: {e}");
    }
    let icon = icon_dir.and_then(|dir| write_icon(dir).ok());
    let registered = windows_registry::CURRENT_USER
        .create(registry_key())
        .and_then(|key| {
            key.set_string("DisplayName", DISPLAY_NAME)?;
            match &icon {
                Some(path) => key.set_string("IconUri", path.display().to_string()),
                None => Ok(()),
            }
        });
    if let Err(e) = registered {
        tracing::warn!("registering the app for notifications failed: {e}");
    }
}

/// The toast icon, written once beside the app's data.
fn write_icon(dir: &Path) -> std::io::Result<PathBuf> {
    let path = dir.join(ICON_FILE);
    if std::fs::read(&path).is_ok_and(|b| b == ICON_PNG) {
        return Ok(path);
    }
    std::fs::create_dir_all(dir)?;
    std::fs::write(&path, ICON_PNG)?;
    Ok(path)
}

/// Uninstalling: the registration goes (one registry operation, for the fast uninstall hook).
pub fn unregister() {
    let _ = windows_registry::CURRENT_USER.remove_tree(registry_key());
}

/// The reminder toasts, shown through Windows. Each toast's buttons send their arguments to `sender`.
#[allow(missing_debug_implementations)] // Holds WinRT objects.
pub struct WinToasts {
    notifier: Option<ToastNotifier>,
    /// The toasts up, by tag: their `Activated` handlers live as long as they do.
    shown: HashMap<String, ToastNotification>,
    sender: UnboundedSender<String>,
}

impl WinToasts {
    pub fn new(sender: UnboundedSender<String>) -> Self {
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))
            .map_err(|e| tracing::warn!("reminder toasts are unavailable: {e}"))
            .ok();
        Self {
            notifier,
            shown: HashMap::new(),
            sender,
        }
    }

    fn try_show(&mut self, toast: &Toast) -> windows::core::Result<()> {
        let Some(notifier) = &self.notifier else {
            return Ok(());
        };
        let document = document(toast)?;
        let notification = ToastNotification::CreateToastNotification(&document)?;
        notification.SetTag(&HSTRING::from(toast.tag.as_str()))?;
        notification.SetGroup(&HSTRING::from(GROUP))?;
        let sender = self.sender.clone();
        notification.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(
            move |_, arguments| {
                let arguments = arguments
                    .as_ref()
                    .and_then(|a| a.cast::<ToastActivatedEventArgs>().ok())
                    .and_then(|a| a.Arguments().ok())
                    .map(|a| a.to_string())
                    .unwrap_or_default();
                let _ = sender.unbounded_send(arguments);
                Ok(())
            },
        ))?;
        if let Some(previous) = self.shown.remove(&toast.tag) {
            let _ = notifier.Hide(&previous);
        }
        notifier.Show(&notification)?;
        self.shown.insert(toast.tag.clone(), notification);
        Ok(())
    }
}

impl ToastSink for WinToasts {
    fn show(&mut self, toast: &Toast) {
        if let Err(e) = self.try_show(toast) {
            tracing::warn!("showing a reminder toast failed: {e}");
        }
    }

    fn hide(&mut self, tag: &str) {
        self.shown.remove(tag);
        // Also a toast left in the action center by an earlier run.
        if let Ok(history) = ToastNotificationManager::History() {
            let _ = history.RemoveGroupedTagWithId(
                &HSTRING::from(tag),
                &HSTRING::from(GROUP),
                &HSTRING::from(AUMID),
            );
        }
    }

    fn clear(&mut self) {
        self.shown.clear();
        if let Ok(history) = ToastNotificationManager::History() {
            let _ = history.ClearWithId(&HSTRING::from(AUMID));
        }
    }
}

/// The toast's XML: a reminder (it stays up until acted on, with the reminder sound), its title and
/// body, and its buttons. Built with the DOM, so a task's title is always text, never markup.
fn document(toast: &Toast) -> windows::core::Result<XmlDocument> {
    let doc = XmlDocument::new()?;
    let root = doc.CreateElement(h!("toast"))?;
    root.SetAttribute(h!("scenario"), h!("reminder"))?;
    root.SetAttribute(h!("launch"), &HSTRING::from(toast.launch.arguments()))?;
    root.SetAttribute(h!("activationType"), h!("foreground"))?;
    doc.AppendChild(&root)?;
    let visual = doc.CreateElement(h!("visual"))?;
    root.AppendChild(&visual)?;
    let binding = doc.CreateElement(h!("binding"))?;
    binding.SetAttribute(h!("template"), h!("ToastGeneric"))?;
    visual.AppendChild(&binding)?;
    for text in [&toast.title, &toast.body] {
        let element = doc.CreateElement(h!("text"))?;
        element.SetInnerText(&HSTRING::from(text.as_str()))?;
        binding.AppendChild(&element)?;
    }
    let actions = doc.CreateElement(h!("actions"))?;
    root.AppendChild(&actions)?;
    for (label, activation) in &toast.actions {
        let action = doc.CreateElement(h!("action"))?;
        action.SetAttribute(h!("content"), &HSTRING::from(*label))?;
        action.SetAttribute(h!("arguments"), &HSTRING::from(activation.arguments()))?;
        action.SetAttribute(h!("activationType"), h!("foreground"))?;
        actions.AppendChild(&action)?;
    }
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::reminders::Activation;

    /// Builds the XML only: nothing is shown.
    #[test]
    fn a_reminder_toast_is_a_reminder_with_its_buttons_and_text_never_markup() {
        let id: todo_core::ids::ReminderId =
            "0190c2a4-0000-7000-8000-000000000001".parse().expect("id");
        let toast = Toast {
            tag: id.to_string(),
            title: "Fix <b>bold</b> & \"quotes\"".into(),
            body: "Today, 17:00 · Errands".into(),
            launch: Activation::Open(id),
            actions: vec![
                ("Open", Activation::Open(id)),
                ("Snooze 10 min", Activation::Snooze(id)),
                ("Complete", Activation::Complete(id)),
            ],
        };
        let xml = document(&toast)
            .expect("xml")
            .GetXml()
            .expect("text")
            .to_string();
        assert!(xml.contains(r#"scenario="reminder""#), "{xml}");
        assert!(xml.contains(&format!(r#"launch="open:{id}""#)), "{xml}");
        assert!(xml.contains("Fix &lt;b&gt;bold&lt;/b&gt; &amp;"), "{xml}");
        for args in ["open", "snooze", "complete"] {
            assert!(
                xml.contains(&format!(r#"arguments="{args}:{id}""#)),
                "{xml}"
            );
        }
        assert!(xml.contains(r#"content="Snooze 10 min""#), "{xml}");
    }
}
