//! Policy for exposing browser notification permission and arming browser timers.

use todo_core::time::Timestamp;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    NotAsked,
    Granted,
    Blocked,
    Unsupported,
}

impl Permission {
    pub fn from_browser(value: &str) -> Self {
        match value {
            "default" => Self::NotAsked,
            "granted" => Self::Granted,
            "denied" => Self::Blocked,
            _ => Self::Unsupported,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::NotAsked => "Not enabled. Until then, reminders appear in this tab.",
            Self::Granted => "Allowed while the app is open.",
            Self::Blocked => "Blocked. Allow notifications in your browser’s site settings.",
            Self::Unsupported => "Not supported here. Reminders appear in this tab.",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Background {
    Unavailable,
    Off,
    On,
    Blocked,
}

impl Background {
    pub fn from_browser(value: &str) -> Self {
        match value {
            "on" => Self::On,
            "off" => Self::Off,
            "blocked" => Self::Blocked,
            _ => Self::Unavailable,
        }
    }

    pub fn description(self, permission: Permission) -> &'static str {
        match self {
            Self::On => {
                "On. Reminders can arrive after you close the app or your session expires. Open the app every 30 days to keep them on."
            }
            Self::Off => "Off. Reminders still appear while the app is open.",
            Self::Blocked => {
                "Blocked by the browser. Allow notifications in your browser’s site settings."
            }
            Self::Unavailable => permission.description(),
        }
    }

    /// Give the service worker a chance to deliver before showing the visible-tab fallback.
    pub fn fallback_at(self, at: Timestamp) -> Timestamp {
        if self == Self::On {
            Timestamp::from_millisecond(at.as_millisecond().saturating_add(8_000))
                .unwrap_or(Timestamp::MAX)
        } else {
            at
        }
    }
}

/// Browser timeouts take a signed 32-bit millisecond count. Wake again for dates farther out;
/// always wait at least a millisecond for a future instant with sub-millisecond precision.
pub fn timeout_ms(next: Timestamp, now: Timestamp) -> u64 {
    next.as_millisecond()
        .saturating_sub(now.as_millisecond())
        .clamp(1, i64::from(i32::MAX))
        .unsigned_abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_only_known_browser_permission_states() {
        assert_eq!(Permission::from_browser("default"), Permission::NotAsked);
        assert_eq!(Permission::from_browser("granted"), Permission::Granted);
        assert_eq!(Permission::from_browser("denied"), Permission::Blocked);
        for value in ["unsupported", "", "prompt"] {
            assert_eq!(Permission::from_browser(value), Permission::Unsupported);
        }
    }

    #[test]
    fn only_an_active_subscription_delays_the_in_tab_fallback() {
        let at = Timestamp::UNIX_EPOCH;
        assert_eq!(
            Background::from_browser("on")
                .fallback_at(at)
                .as_millisecond(),
            8_000
        );
        for status in ["off", "blocked", "unknown"] {
            assert_eq!(Background::from_browser(status).fallback_at(at), at);
        }
    }

    #[test]
    fn browser_timer_does_not_wrap_or_busy_loop() {
        let now = Timestamp::UNIX_EPOCH;
        assert_eq!(timeout_ms(Timestamp::MIN, now), 1);
        assert_eq!(
            timeout_ms(Timestamp::from_nanosecond(1).expect("timestamp"), now),
            1
        );
        assert_eq!(
            timeout_ms(Timestamp::from_millisecond(2).expect("timestamp"), now),
            2
        );
        assert_eq!(timeout_ms(Timestamp::MAX, now), i32::MAX as u64);
    }
}
