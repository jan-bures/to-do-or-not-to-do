//! Who is calling, and what they may do.
//!
//! This is the seam between authentication and the rest of the server. Handlers only ever see an
//! [`Authenticated`](super::Authenticated) carrying a [`Principal`]; how it was established (owner
//! token today; sessions, agent tokens and passkeys in step 4a) stays inside the `auth` module.

use todo_core::{Actor, DeviceId};

/// An authenticated caller.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Principal {
    /// The owner, on one of their devices.
    Owner {
        /// The device (from the token's configuration).
        device: DeviceId,
    },
}

/// What a principal may do. Command-level checks still happen in `todo-core` against the [`Actor`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    /// Everything: reads, writes, undo of any event, the OpenAPI document.
    Full,
}

impl Principal {
    /// The actor recorded on events this principal causes.
    #[must_use]
    pub fn actor(&self) -> Actor {
        match self {
            Self::Owner { device } => Actor::Owner {
                device: device.clone(),
            },
        }
    }

    /// The principal's access.
    #[must_use]
    pub fn access(&self) -> Access {
        match self {
            Self::Owner { .. } => Access::Full,
        }
    }
}
