//! Who is calling, and what they may do.
//!
//! This is the seam between authentication and the rest of the server. Handlers only ever see an
//! [`Authenticated`](super::Authenticated) carrying a [`Principal`]; how it was established (a paired
//! device token, an agent token, or the development owner tokens) stays inside the `auth` module.

use todo_core::{Actor, AgentId, DeviceId};

/// An authenticated API caller.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Principal {
    /// The owner, on one of their devices.
    Owner {
        /// The device (paired, or from the development owner-token configuration).
        device: DeviceId,
    },
    /// The owner in the web app, through their browser session (see
    /// [`require_auth_or_session`](super::require_auth_or_session)). Acts as the owner on `device`
    /// ([`WEB_DEVICE`](super::WEB_DEVICE)); loses access when the session ends.
    Session {
        /// Hex SHA-256 of the session cookie (the session's row ID; never the cookie itself).
        session: String,
        /// The device its events are recorded under.
        device: DeviceId,
    },
    /// An AI agent, through one agent token.
    Agent {
        /// The agent; its events are recorded as `Actor::agent(agent)`.
        agent: AgentId,
        /// The token's row ID (quotas, rate limits and revocation are per token).
        token: i64,
    },
}

/// What a principal may do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    /// Everything the API offers: reads, writes, undo of any event, the OpenAPI document.
    Full,
    /// Only what the agent's grants allow. Reads are filtered by `todo-core`'s access levels (see
    /// [`crate::api::visibility`]); commands and undo are checked by `todo-core` against the actor;
    /// writes count against the token's quota.
    Agent {
        /// Writes (commands and undos) allowed per rolling hour.
        writes_per_hour: u32,
    },
}

impl Principal {
    /// The actor recorded on events this principal causes.
    #[must_use]
    pub fn actor(&self) -> Actor {
        match self {
            Self::Owner { device } | Self::Session { device, .. } => Actor::Owner {
                device: device.clone(),
            },
            Self::Agent { agent, .. } => Actor::Agent {
                agent_id: agent.clone(),
            },
        }
    }

    /// True for an agent.
    #[must_use]
    pub fn is_agent(&self) -> bool {
        matches!(self, Self::Agent { .. })
    }
}
