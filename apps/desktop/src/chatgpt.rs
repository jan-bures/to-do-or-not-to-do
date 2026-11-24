//! Native browser handoff. The paired server owns PKCE, token exchange and credentials.
mod authorization;

pub use authorization::Attempt;
pub use todo_client::assistant::native_relay::{
    NativeProvider as DesktopProvider, USAGE_URL as USAGE,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("Couldn't reach your To-do server or ChatGPT.")]
    Network,
    #[error("Couldn't open or verify the local sign-in callback.")]
    Callback,
    #[error("Sign-in timed out. Start a new sign-in to try again.")]
    TimedOut,
    #[error("ChatGPT sign-in was canceled.")]
    Declined,
}

#[derive(Clone)]
pub struct AccountView {
    pub id: uuid::Uuid,
    pub label: String,
    pub consented_lists: Option<std::collections::BTreeSet<todo_core::ids::ListId>>,
}

impl AccountView {
    pub fn covers(&self, lists: &std::collections::BTreeSet<todo_core::ids::ListId>) -> bool {
        self.consented_lists
            .as_ref()
            .is_some_and(|approved| lists.is_subset(approved))
    }
}
