//! Native browser handoff. The paired server owns PKCE, token exchange and credentials.
use std::collections::BTreeSet;
use todo_client::assistant::relay;
use todo_core::ids::ListId;
use uuid::Uuid;

pub use todo_client::assistant::browser_sign_in::{Attempt, Error};
pub use todo_client::assistant::native_relay::{
    NativeProvider as DesktopProvider, USAGE_URL as USAGE,
};

#[derive(Clone)]
pub struct AccountView {
    pub id: Uuid,
    pub label: String,
    pub consented_lists: Option<BTreeSet<ListId>>,
}

impl AccountView {
    pub fn access_update(
        &self,
        lists: &BTreeSet<ListId>,
        access_version: Uuid,
    ) -> Option<relay::Consent> {
        let covered = self
            .consented_lists
            .as_ref()
            .is_some_and(|approved| lists.is_subset(approved));
        (!covered).then(|| relay::Consent {
            lists: lists.clone(),
            access_version,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_registers_the_current_scope_only_when_needed() {
        let mut account = AccountView {
            id: Uuid::nil(),
            label: String::new(),
            consented_lists: None,
        };
        let version = Uuid::from_u128(1);
        let empty = BTreeSet::new();
        assert!(account.access_update(&empty, version).is_some());
        let first = ListId::from_uuid(Uuid::from_u128(2));
        let new = ListId::from_uuid(Uuid::from_u128(3));
        account.consented_lists = Some(BTreeSet::from([first]));
        assert!(account.access_update(&empty, version).is_none());
        assert!(
            account
                .access_update(&BTreeSet::from([first]), version)
                .is_none()
        );
        let current = BTreeSet::from([new]);
        let request = account
            .access_update(&current, version)
            .expect("new scope needs recording");
        assert_eq!(request.lists, current);
        assert_eq!(request.access_version, version);
    }
}
