use super::*;
use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    response::IntoResponse,
    routing::{get, post},
};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::net::TcpListener;

const CLIENT: &str = "fixture-client";
const CLAIMS: &str = r#"{"iss":"https://auth.openai.com","sub":"fixture-subject","aud":"fixture-client","exp":4102444800,"email":"you@example.com","https://api.openai.com/auth":{"chatgpt_account_id":"fixture-workspace"}}"#;
// One signature over the invented claims above, and its PUBLIC verification key. No private key.
const SIG: &str = "Z14nM2reVEV_xuIIHIN_GQMru6wSGDxlBLZTR2mRhPoEyVWMjaWehrEzWJneJ63s7ZR2iRGQbZPPMoTu0TYVCXHQLHqXZx79szEvQjwS7IWnURHi_psyYHsB9R662BhTguf1TwPqAqCFQFpQxtzIRxzTl26UoJJVaHhMgqSgaYURbp7JnLzrJSqwp3B5ixv8sV9yDDH6xD3L_NNH42SGCTuqC259QEeym8mZ54y6JhGg53WNr_HYqDq9CPD9cI-kEquyF51VljWaS5pXYiFbhWWLHlrk32aEOX2jkqckAntDPEpKrUxa6u3QDagT-2lP2HvKHqEsgddqCVZeP8Ffmg";
const MODULUS: &str = "nrEZrs_tD3onn9glZLNiS_DLVS3iz4I-NzMwNm9fbm0Qznw1wcBDHb76zyYHwRNM_FOIyKE30Mt6S4sywCODkCFgI9U_NVUsWS8bEF2URSH7o2-9Zf2JRH6dwSd5Mb-e33N_3UeE7_H4hc_no9Bda3mzfGCOkj3e1oQ9tj8EinqRMWIdbpbMPsetpmURX8aO5jyYO7ouUzlxHbqkxwgumcKg7Fc7qe38PQyfrcBeA5NPOMjYl8Xe6gJvSArWXxf9T5qKA7eoJy3JBKSi5X7-F-WnVFsmcCft6h-gPI0PoLDZhzkkrmD56N9lBPqiPnUTuQVme6xEtFsqaJBjKAogvw";

fn jwt(claims: &str) -> String {
    format!(
        "{}.{}.{SIG}",
        B64.encode(br#"{"alg":"RS256","kid":"fixture-key"}"#),
        B64.encode(claims)
    )
}

#[tokio::test]
async fn device_login_refresh_model_list_stream_and_revoke_use_the_expected_contracts() {
    let pending = Arc::new(AtomicBool::new(true));
    let app = Router::new()
        .route("/api/accounts/deviceauth/usercode", post(|Json(body): Json<Value>| async move {
            assert_eq!(body, json!({"client_id": CLIENT}));
            Json(json!({"device_auth_id":"fixture-request", "usercode":"SAMPLE-ONE", "interval":"5"}))
        }))
        .route("/api/accounts/deviceauth/token", post(|State(pending): State<Arc<AtomicBool>>, Json(body): Json<Value>| async move {
            assert_eq!(body, json!({"device_auth_id":"fixture-request", "user_code":"SAMPLE-ONE"}));
            if pending.swap(false, Ordering::SeqCst) { return StatusCode::FORBIDDEN.into_response(); }
            Json(json!({"authorization_code":"fixture-code", "code_verifier":"fixture-verifier"})).into_response()
        }))
        .route("/oauth/token", post(|headers: HeaderMap, body: String| async move {
            if headers["content-type"] == "application/json" {
                assert_eq!(serde_json::from_str::<Value>(&body).unwrap(), json!({"grant_type":"refresh_token", "client_id":CLIENT, "refresh_token":"fixture-refresh"}));
                return Json(json!({"access_token":"fixture-new-access", "refresh_token":"fixture-rotated", "expires_in":3600}));
            }
            let fields = url::form_urlencoded::parse(body.as_bytes()).into_owned().collect::<HashMap<_,_>>();
            assert!(fields["redirect_uri"].starts_with("http://127.0.0.1:"));
            assert!(fields["redirect_uri"].ends_with("/deviceauth/callback"));
            assert_eq!(fields["client_id"], CLIENT);
            assert_eq!(fields["code"], "fixture-code");
            assert_eq!(fields["code_verifier"], "fixture-verifier");
            assert_eq!(fields["grant_type"], "authorization_code");
            assert!(!fields.contains_key("resource"));
            Json(json!({"access_token":"fixture-access", "refresh_token":"fixture-refresh", "expires_in":1, "id_token": jwt(CLAIMS)}))
        }))
        .route("/.well-known/jwks.json", get(|| async { Json(json!({"keys":[
            {"kid":"not-an-rsa-key", "kty":"EC"},
            {"kid":"fixture-key", "kty":"RSA", "use":"sig", "n":MODULUS, "e":"AQAB"}
        ]})) }))
        .route("/codex/models", get(|headers: HeaderMap| async move {
            assert_eq!(headers["authorization"], "Bearer fixture-new-access");
            assert_eq!(headers["chatgpt-account-id"], "fixture-workspace");
            Json(json!({"models":[{"slug":"fixture-model", "display_name":"Example", "visibility":"list"}]}))
        }))
        .route("/codex/responses", post(|headers: HeaderMap, Json(body): Json<Value>| async move {
            assert_eq!(headers["authorization"], "Bearer fixture-new-access");
            assert_eq!(headers["chatgpt-account-id"], "fixture-workspace");
            assert_eq!(body["store"], false);
            ([("content-type", "text/event-stream")], "data: fixture\n\n")
        }))
        .route("/oauth/revoke", post(|Json(body): Json<Value>| async move {
            assert_eq!(body, json!({"client_id":CLIENT, "token_type_hint":"refresh_token", "token":"fixture-rotated"}));
            StatusCode::NO_CONTENT
        }))
        .with_state(pending);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", listener.local_addr().unwrap());
    let mock = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let openai = OpenAi {
        issuer: issuer.clone(),
        codex: format!("{issuer}/codex"),
        ..OpenAi::new(Some(CLIENT.into())).unwrap()
    };
    let start = openai.start_device().await.unwrap();
    assert_eq!(start.user_code, "SAMPLE-ONE");
    assert_eq!(start.interval, 5);
    assert!(openai.poll_device(&start).await.unwrap().is_none());
    let mut credential = openai.poll_device(&start).await.unwrap().unwrap();
    assert_eq!(credential.label, "you@example.com");
    assert!(!credential.approved);
    assert!(
        openai
            .verify(
                &jwt(&CLAIMS.replace("fixture-subject", "different")),
                CLIENT,
                None
            )
            .await
            .is_err()
    );
    openai.refresh(&mut credential).await.unwrap();
    assert_eq!(credential.refresh, "fixture-rotated");
    assert_eq!(
        openai.models(&credential).await.unwrap()[0].id,
        "fixture-model"
    );
    let response = openai
        .chat(&credential, &json!({"store":false,"stream":true}))
        .await
        .unwrap();
    assert_eq!(response.text().await.unwrap(), "data: fixture\n\n");
    openai.revoke(&(&credential).into()).await.unwrap();
    mock.abort();
}

#[tokio::test]
async fn loopback_handoff_checks_state_and_uses_pkce_without_sending_the_verifier() {
    let openai = OpenAi::new(None).unwrap();
    let (grant, url) = openai.start_loopback(Uuid::new_v4(), 12480).unwrap();
    let url = Url::parse(&url).unwrap();
    let pairs = url.query_pairs().collect::<HashMap<_, _>>();
    assert_eq!(
        pairs["redirect_uri"],
        "http://127.0.0.1:12480/auth/callback"
    );
    assert_eq!(pairs["code_challenge_method"], "S256");
    assert_eq!(
        pairs["code_challenge"],
        B64.encode(digest::digest(&digest::SHA256, grant.verifier.as_bytes()).as_ref())
    );
    assert_ne!(pairs["code_challenge"], grant.verifier.as_str());
    assert_eq!(url.origin(), Url::parse(ISSUER).unwrap().origin());
    assert!(matches!(
        openai
            .handoff(
                &grant,
                &todo_tools::assistant::relay::Handoff {
                    state: "wrong".into(),
                    code: "fixture-code".into(),
                    client_id: "fixture-client".into()
                }
            )
            .await,
        Err(Error::BadRequest)
    ));
    let claims: Claims = serde_json::from_str(CLAIMS).unwrap();
    assert!(validate_claims(&claims, ISSUER, CLIENT, None, now()).is_ok());
    assert!(validate_claims(&claims, ISSUER, CLIENT, Some("required nonce"), now()).is_err());
    assert!(validate_claims(&claims, ISSUER, "wrong-client", None, now()).is_err());
    assert!(validate_claims(&claims, ISSUER, CLIENT, None, claims.exp).is_err());
}
