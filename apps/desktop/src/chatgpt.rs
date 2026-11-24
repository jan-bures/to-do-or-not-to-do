//! Native browser handoff. The paired server owns PKCE, token exchange and credentials.
pub use todo_client::assistant::browser_sign_in::{Attempt, Error};
pub use todo_client::assistant::native_relay::{
    NativeProvider as DesktopProvider, USAGE_URL as USAGE,
};

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
