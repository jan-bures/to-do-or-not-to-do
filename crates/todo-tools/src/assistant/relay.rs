//! Wire types for the paired server's owner-only assistant relay.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use todo_core::ids::ListId;
use uuid::Uuid;

use super::access::{AccessRules, DataClass};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Status {
    pub chatgpt: bool,
    pub chatgpt_device_code: bool,
    pub private_models: bool,
    pub accounts: Vec<Account>,
    pub access: AccessRules,
    /// Compare-and-swap token for writes from a settings view opened on another device.
    pub access_version: Uuid,
    /// A sanitized initialization failure, visible only to the owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Account {
    /// A relay-generated handle, not the provider's account or client ID.
    pub id: Uuid,
    pub label: String,
    pub approved: bool,
    pub approved_lists: BTreeSet<ListId>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccessUpdate {
    pub access: AccessRules,
    pub access_version: Uuid,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Consent {
    pub lists: BTreeSet<ListId>,
    pub access_version: Uuid,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Chat {
    pub destination: DataClass,
    /// The client's scope hint, retained for compatibility. The server does not trust or forward it.
    pub lists: BTreeSet<ListId>,
    pub request: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Model {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, tag = "flow", rename_all = "snake_case")]
pub enum SignIn {
    Device,
    Loopback { port: u16 },
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "flow", rename_all = "snake_case")]
pub enum SignInStarted {
    Device {
        attempt: Uuid,
        user_code: String,
        verification_url: String,
    },
    Loopback {
        attempt: Uuid,
        authorize_url: String,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Handoff {
    pub state: String,
    pub code: String,
    pub client_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Poll {
    Pending,
    SignedIn { account: Account },
    Expired,
    Failed,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SignOut {
    pub revoked: bool,
}

macro_rules! redacted_debug {
    ($($ty:ident),* $(,)?) => {$(
        impl std::fmt::Debug for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($ty)).finish_non_exhaustive()
            }
        }
    )*};
}

redacted_debug!(Chat, Handoff, SignInStarted);
