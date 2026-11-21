//! Start with Windows: a value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
//!
//! Per user, so it never needs admin rights. The registry value is the setting itself: the tray's
//! "Start with Windows" item shows whether it is present and points at this executable.

use std::path::Path;

use windows_registry::{CURRENT_USER, Key};

/// The per-user Run key.
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Our value's name under [`RUN_KEY`].
pub const VALUE_NAME: &str = "To-do or not to do";
/// Started from the Run key, the app goes straight to the tray.
pub const BACKGROUND_FLAG: &str = "--background";

/// The command line stored in the Run value: the quoted executable path plus `--background`.
pub fn run_command(exe: &Path) -> String {
    format!("\"{}\" {BACKGROUND_FLAG}", exe.display())
}

/// The executable a Run value launches: the quoted (or, for hand-edited values, unquoted) first token.
pub fn command_exe(command: &str) -> Option<&str> {
    let command = command.trim();
    if let Some(rest) = command.strip_prefix('"') {
        return rest
            .split_once('"')
            .map(|(exe, _)| exe)
            .filter(|exe| !exe.is_empty());
    }
    command.split_whitespace().next()
}

/// Whether a Run value launches `exe`. Windows paths compare case-insensitively.
pub fn command_targets(command: &str, exe: &Path) -> bool {
    command_exe(command).is_some_and(|c| c.eq_ignore_ascii_case(&exe.display().to_string()))
}

/// Start with Windows, against an arbitrary key (tests point it at a scratch key).
#[derive(Debug)]
pub struct Autostart<'k> {
    root: &'k Key,
    path: String,
}

impl Autostart<'static> {
    /// The real per-user Run key.
    pub fn current_user() -> Self {
        Self {
            root: CURRENT_USER,
            path: RUN_KEY.to_string(),
        }
    }
}

impl<'k> Autostart<'k> {
    #[cfg(test)]
    pub fn at(root: &'k Key, path: impl Into<String>) -> Self {
        Self {
            root,
            path: path.into(),
        }
    }

    /// Whether the app starts with Windows (the value exists and launches `exe`).
    pub fn is_enabled(&self, exe: &Path) -> bool {
        let Ok(key) = self.root.open(&self.path) else {
            return false;
        };
        key.get_string(VALUE_NAME)
            .is_ok_and(|v| command_targets(&v, exe))
    }

    /// Turns Start with Windows on or off for `exe`.
    pub fn set_enabled(&self, exe: &Path, enabled: bool) -> windows_registry::Result<()> {
        let key = self.root.create(&self.path)?;
        if enabled {
            key.set_string(VALUE_NAME, run_command(exe))
        } else {
            match key.remove_value(VALUE_NAME) {
                // Already absent.
                Err(e) if e.code().0 as u32 == 0x8007_0002 => Ok(()),
                other => other,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn run_command_quotes_paths_with_spaces() {
        let exe = PathBuf::from(r"C:\Program Files\To-do\todo-desktop.exe");
        assert_eq!(
            run_command(&exe),
            r#""C:\Program Files\To-do\todo-desktop.exe" --background"#
        );
    }

    #[test]
    fn command_exe_reads_quoted_and_bare_commands() {
        assert_eq!(
            command_exe(r#""C:\a b\x.exe" --background"#),
            Some(r"C:\a b\x.exe")
        );
        assert_eq!(
            command_exe(r"C:\apps\x.exe --background"),
            Some(r"C:\apps\x.exe")
        );
        assert_eq!(command_exe(r#"  "C:\x.exe""#), Some(r"C:\x.exe"));
        assert_eq!(command_exe(r#""unterminated"#), None);
        assert_eq!(command_exe(r#""""#), None);
        assert_eq!(command_exe(""), None);
    }

    #[test]
    fn command_targets_ignores_case_and_rejects_other_executables() {
        let exe = PathBuf::from(r"C:\Apps\To-do\todo-desktop.exe");
        assert!(command_targets(
            r#""c:\apps\to-do\TODO-DESKTOP.exe" --background"#,
            &exe
        ));
        assert!(!command_targets(
            r#""C:\Old\todo-desktop.exe" --background"#,
            &exe
        ));
    }

    /// A throwaway key under HKCU, removed on drop. HKCU needs no admin rights.
    struct ScratchKey(String);

    impl ScratchKey {
        fn new() -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default();
            Self(format!(
                r"Software\to-do-or-not-to-do-tests\autostart-{}-{nanos}",
                std::process::id()
            ))
        }
    }

    impl Drop for ScratchKey {
        fn drop(&mut self) {
            let _ = CURRENT_USER.remove_tree(&self.0);
        }
    }

    #[test]
    fn enabling_and_disabling_round_trips_through_the_registry() {
        let scratch = ScratchKey::new();
        let autostart = Autostart::at(CURRENT_USER, scratch.0.clone());
        let exe = PathBuf::from(r"C:\Program Files\To-do\todo-desktop.exe");
        assert!(!autostart.is_enabled(&exe));

        autostart.set_enabled(&exe, true).expect("enable");
        assert!(autostart.is_enabled(&exe));
        let stored = CURRENT_USER
            .open(&scratch.0)
            .and_then(|k| k.get_string(VALUE_NAME))
            .expect("value");
        assert_eq!(stored, run_command(&exe));
        // A different install location doesn't count as enabled.
        assert!(!autostart.is_enabled(&PathBuf::from(r"D:\other\todo-desktop.exe")));

        autostart.set_enabled(&exe, false).expect("disable");
        assert!(!autostart.is_enabled(&exe));
        // Disabling twice is fine.
        autostart.set_enabled(&exe, false).expect("disable again");
    }
}
