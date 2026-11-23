//! Who performed an action.

use crate::ids::ParseIdError;
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

/// Reserved for owner-approved in-app assistant batches; never a provisionable agent token.
pub const ASSISTANT_AGENT_ID: &str = "assistant";

/// Reserve the native identity and look-alike slugs. Agent IDs use ASCII, so removing their two
/// separators catches `in-app-assistant`, `in_app_assistant` and suffixed variants consistently.
#[must_use]
pub fn is_reserved_assistant_slug(id: &str) -> bool {
    let compact: String = id.chars().filter(|c| !matches!(c, '-' | '_')).collect();
    compact.starts_with(ASSISTANT_AGENT_ID) || compact.starts_with("inappassistant")
}

fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

macro_rules! slug_id {
    ($(#[$meta:meta])* $name:ident, $what:literal) => {
        $(#[$meta])*
        #[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Validates and wraps the ID.
            pub fn new(id: impl Into<String>) -> Result<Self, ParseIdError> {
                let id = id.into();
                if valid_slug(&id) {
                    Ok(Self(id))
                } else {
                    Err(ParseIdError(format!(concat!("invalid ", $what, " id `{}`"), id)))
                }
            }

            /// The ID as a string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                Self::new(s).map_err(serde::de::Error::custom)
            }
        }
    };
}

slug_id!(
    /// Identifier of an agent (for example `codex`): lowercase letters, digits, `-` and `_`, up to 64 bytes.
    AgentId,
    "agent"
);
slug_id!(
    /// Identifier of an owner device session (same alphabet as [`AgentId`]).
    DeviceId,
    "device"
);

/// The party that caused an event.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Actor {
    /// The human owner, acting from a device session.
    Owner {
        /// The device session.
        device: DeviceId,
    },
    /// An external AI agent or the reserved in-app assistant.
    Agent {
        /// The agent.
        agent_id: AgentId,
    },
    /// The one-time Microsoft To Do importer.
    Importer,
    /// The email-to-task ingest pipeline.
    EmailIngest,
    /// Server-internal housekeeping.
    System,
}

impl Actor {
    /// Convenience constructor for an owner on a device.
    pub fn owner(device: &str) -> Result<Self, ParseIdError> {
        Ok(Self::Owner {
            device: DeviceId::new(device)?,
        })
    }

    /// Convenience constructor for an agent.
    pub fn agent(agent_id: &str) -> Result<Self, ParseIdError> {
        Ok(Self::Agent {
            agent_id: AgentId::new(agent_id)?,
        })
    }

    /// The agent ID if this actor is an agent.
    #[must_use]
    pub fn agent_id(&self) -> Option<&AgentId> {
        match self {
            Self::Agent { agent_id } => Some(agent_id),
            _ => None,
        }
    }

    /// True for any agent.
    #[must_use]
    pub fn is_agent(&self) -> bool {
        matches!(self, Self::Agent { .. })
    }

    /// True for an owner device session.
    #[must_use]
    pub fn is_owner(&self) -> bool {
        matches!(self, Self::Owner { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_validated() {
        assert!(AgentId::new("codex").is_ok());
        assert!(AgentId::new("Bad Name").is_err());
        assert!(AgentId::new("").is_err());
        assert!(serde_json::from_str::<AgentId>("\"A\"").is_err());
    }

    #[test]
    fn assistant_aliases_are_reserved() {
        for reserved in [
            "assistant",
            "in-app-assistant",
            "in_app_assistant",
            "assistant-worker",
            "in--app-assistant_2",
        ] {
            assert!(is_reserved_assistant_slug(reserved), "{reserved}");
        }
        for other in ["claude", "codex", "editor-assistant"] {
            assert!(!is_reserved_assistant_slug(other), "{other}");
        }
    }

    #[test]
    fn actor_serde_roundtrip() {
        let a = Actor::agent("claude").unwrap();
        let s = serde_json::to_string(&a).unwrap();
        assert_eq!(s, r#"{"type":"agent","agent_id":"claude"}"#);
        assert_eq!(serde_json::from_str::<Actor>(&s).unwrap(), a);
    }
}
