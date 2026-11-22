//! Shared status and wording for the compact sync footer and its accessible description.

use crate::Connection;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncLabel<'a> {
    Unpaired,
    SignedOut,
    Failed(usize),
    Offline(usize),
    Syncing,
    /// The local time of the last successful sync, already formatted by the platform.
    Synced(Option<&'a str>),
    Paused(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncText {
    pub short: String,
    pub full: String,
}

impl<'a> SyncLabel<'a> {
    /// A signed-out session or refused changes take precedence over connectivity. A missing
    /// connection means sync is paused, not offline.
    pub fn for_client(
        connection: Option<Connection>,
        pending: usize,
        failed: usize,
        signed_out: bool,
        local_time: Option<&'a str>,
    ) -> Self {
        if signed_out {
            Self::SignedOut
        } else if failed > 0 {
            Self::Failed(failed)
        } else {
            match connection {
                None => Self::Paused(pending),
                Some(Connection::Offline) => Self::Offline(pending),
                Some(Connection::Syncing) => Self::Syncing,
                Some(Connection::Online) => Self::Synced(local_time),
            }
        }
    }

    pub fn text(self) -> SyncText {
        let (short, full) = match self {
            Self::Unpaired => ("Not signed in".into(), None),
            Self::SignedOut => ("Signed out".into(), Some("Signed out by the server".into())),
            Self::Failed(n) => (
                format!("{n} not synced"),
                Some(format!("{n} couldn't sync")),
            ),
            Self::Offline(0) => ("Offline".into(), None),
            Self::Offline(n) => (
                format!("Offline · {n}"),
                Some(format!("Offline, {n} waiting")),
            ),
            Self::Syncing => ("Syncing…".into(), None),
            Self::Synced(None) => ("Synced".into(), None),
            Self::Synced(Some(time)) => (time.into(), Some(format!("Synced {time}"))),
            Self::Paused(0) => ("Not syncing".into(), None),
            Self::Paused(n) => (
                format!("Not syncing · {n}"),
                Some(format!("Not syncing, {n} waiting")),
            ),
        };
        SyncText {
            full: full.unwrap_or_else(|| short.clone()),
            short,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_take_precedence_over_connection_but_not_sign_out() {
        for connection in [
            None,
            Some(Connection::Offline),
            Some(Connection::Syncing),
            Some(Connection::Online),
        ] {
            assert_eq!(
                SyncLabel::for_client(connection, 3, 2, true, Some("20:18")),
                SyncLabel::SignedOut,
            );
            assert_eq!(
                SyncLabel::for_client(connection, 3, 2, false, Some("20:18")),
                SyncLabel::Failed(2),
            );
        }
    }

    #[test]
    fn compact_labels_keep_the_full_explanation() {
        for (state, short, full) in [
            (SyncLabel::Synced(Some("20:18")), "20:18", "Synced 20:18"),
            (SyncLabel::Offline(3), "Offline · 3", "Offline, 3 waiting"),
            (SyncLabel::Syncing, "Syncing…", "Syncing…"),
            (SyncLabel::Failed(2), "2 not synced", "2 couldn't sync"),
            (
                SyncLabel::SignedOut,
                "Signed out",
                "Signed out by the server",
            ),
        ] {
            let text = state.text();
            assert_eq!(text.short, short);
            assert_eq!(text.full, full);
        }
    }
}
