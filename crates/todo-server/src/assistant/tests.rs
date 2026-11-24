use super::*;
use serde_json::json;
use todo_core::{Actor, State, apply, decide, time::Timestamp};
use todo_tools::assistant::access::ListAccess;

fn add_list(state: &mut State, name: &str) -> ListId {
    let id = ListId::from_uuid(Uuid::new_v4());
    let command =
        serde_json::from_value(json!({"cmd":"create_list", "id":id, "name":name})).unwrap();
    for event in decide(state, &Actor::System, &command, Timestamp::now()).unwrap() {
        apply(state, &event).unwrap();
    }
    id
}

#[tokio::test]
async fn shared_consent_is_account_specific_and_drops_with_signout() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        chatgpt: true,
        ..Config::default()
    };
    let service = Assistant::open(config.clone(), dir.path())
        .unwrap()
        .unwrap();
    let mut state = State::new();
    let existing = add_list(&mut state, "Allowed");
    let new = add_list(&mut state, "Newly allowed");
    let limited = add_list(&mut state, "Private only");
    let hidden = add_list(&mut state, "Hidden");
    let auth = Credential {
        flow: Flow::Loopback,
        client_id: "first-client".into(),
        subject: "fixture".into(),
        workspace: None,
        label: "Example".into(),
        access: "fixture-access".into(),
        refresh: "fixture-refresh".into(),
        expires_at: openai::now() + 3600,
        scope: None,
        approved: false,
        approved_lists: BTreeSet::new(),
    };
    let id = service.insert(auth.clone()).await.unwrap().id;
    assert!(matches!(
        service.credential(id, Some(&BTreeSet::new()), None).await,
        Err(Error::Consent)
    ));
    let mut second_credential = auth.clone();
    second_credential.client_id = "second-client".into();
    let second = service.insert(second_credential).await.unwrap().id;
    assert_ne!(second, id);
    let guard = service.records.clone().lock_owned().await;
    let mut changed = guard.clone();
    changed.access.lists.extend([
        (existing, ListAccess::AllModels),
        (new, ListAccess::AllModels),
        (limited, ListAccess::PrivateOnly),
    ]);
    let account = changed.accounts.get_mut(&id).unwrap();
    account.approved = true;
    account.approved_lists.insert(existing);
    service.commit(guard, changed).await.unwrap();
    let approved = BTreeSet::from([existing]);
    let grown = BTreeSet::from([existing, new]);
    let access = service.status().await.access;
    assert!(check_scope(&state, &access, &grown, DataClass::Cloud).is_ok());
    assert!(service.credential(id, Some(&approved), None).await.is_ok());
    assert!(matches!(
        service.credential(id, Some(&grown), None).await,
        Err(Error::Consent)
    ));
    assert!(matches!(
        service.credential(second, Some(&approved), None).await,
        Err(Error::Consent)
    ));
    assert!(
        check_scope(
            &state,
            &access,
            &BTreeSet::from([limited]),
            DataClass::Local
        )
        .is_ok()
    );
    assert!(matches!(
        check_scope(
            &state,
            &access,
            &BTreeSet::from([limited]),
            DataClass::Cloud
        ),
        Err(Error::Scope)
    ));
    assert!(matches!(
        check_scope(&state, &access, &BTreeSet::from([hidden]), DataClass::Local),
        Err(Error::Scope)
    ));
    service.remove(id).await.unwrap();
    let restarted = Assistant::open(config, dir.path()).unwrap().unwrap();
    assert!(matches!(
        restarted.credential(id, None, None).await,
        Err(Error::AccountMissing)
    ));
    let signed_in_again = restarted.insert(auth).await.unwrap();
    assert_ne!(signed_in_again.id, id);
    assert!(!signed_in_again.approved && signed_in_again.approved_lists.is_empty());
    assert!(restarted.credential(second, None, None).await.is_ok());
}

#[test]
fn cloud_request_forces_stream_and_disables_provider_storage_or_builtin_tools() {
    let mut request = relay::Chat {
        destination: DataClass::Cloud,
        lists: BTreeSet::new(),
        request: json!({"model":"example", "input":[{"role":"user","content":"hello"}],"store":true,"stream":false}),
    };
    validate_request(&mut request, DataClass::Cloud).unwrap();
    assert_eq!(request.request["stream"], true);
    assert_eq!(request.request["store"], false);
    request.request["tools"] = json!([{"type":"web_search"}]);
    assert!(validate_request(&mut request, DataClass::Cloud).is_err());
}
