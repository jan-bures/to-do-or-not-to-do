#![allow(clippy::unwrap_used)]

use super::*;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

const MODEL: &str = "fixture-model";

#[derive(Clone)]
struct Request {
    head: String,
    body: Value,
}

struct Reply {
    status: &'static str,
    headers: Vec<(&'static str, String)>,
    body: String,
}

impl Reply {
    fn json(body: Value) -> Self {
        Self {
            status: "200 OK",
            headers: vec![("Content-Type", "application/json".into())],
            body: body.to_string(),
        }
    }
    fn sse(body: String) -> Self {
        Self {
            status: "200 OK",
            headers: vec![("Content-Type", "text/event-stream; charset=utf-8".into())],
            body,
        }
    }
    fn status(status: &'static str) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: "untrusted payload".into(),
        }
    }
    fn header(mut self, key: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((key, value.into()));
        self
    }
}

struct Server {
    address: String,
    seen: Arc<Mutex<Vec<Request>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Server {
    async fn start(respond: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}/gateway/v1/", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_for_task = seen.clone();
        let task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let Some(request) = read(&mut socket).await else {
                    continue;
                };
                let reply = respond(&request);
                seen_for_task.lock().unwrap().push(request);
                let lines = reply
                    .headers
                    .into_iter()
                    .map(|(key, value)| format!("{key}: {value}\r\n"))
                    .collect::<String>();
                let head = format!(
                    "HTTP/1.1 {}\r\nConnection: close\r\nContent-Length: {}\r\n{lines}\r\n",
                    reply.status,
                    reply.body.len()
                );
                if socket.write_all(head.as_bytes()).await.is_err() {
                    continue;
                }
                // Split the SSE at arbitrary byte boundaries even inside multi-byte prose.
                for chunk in reply.body.as_bytes().chunks(37) {
                    if socket.write_all(chunk).await.is_err() {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
                let _ = socket.shutdown().await;
            }
        });
        Self {
            address,
            seen,
            task,
        }
    }

    fn provider(&self) -> LocalProvider {
        LocalProvider::new(&self.address, "").unwrap()
    }
    fn calls(&self) -> Vec<Request> {
        self.seen.lock().unwrap().clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read(socket: &mut TcpStream) -> Option<Request> {
    let mut data = Vec::new();
    let mut buffer = [0; 4096];
    let start = loop {
        let n = socket.read(&mut buffer).await.ok()?;
        if n == 0 || data.len() >= 256 * 1024 {
            return None;
        }
        data.extend_from_slice(&buffer[..n]);
        if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8(data[..start].to_vec()).ok()?;
    let length = head
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            if key.eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);
    while data.len() < start + length {
        let n = socket.read(&mut buffer).await.ok()?;
        if n == 0 {
            return None;
        }
        data.extend_from_slice(&buffer[..n]);
    }
    let body = if length == 0 {
        Value::Null
    } else {
        serde_json::from_slice(&data[start..start + length]).ok()?
    };
    Some(Request { head, body })
}

fn round(mode: RoundMode) -> RoundRequest {
    RoundRequest {
        turn: uuid::Uuid::nil(),
        round: 0,
        model: MODEL.into(),
        instructions: "fixture instructions",
        history: vec![Message::User {
            text: "Test prompt".into(),
        }],
        today: todo_core::time::Date::ZERO,
        previous_draft: None,
        tools: vec![ToolDefinition {
            name: "list_lists",
            description: "List known lists.",
            arguments: serde_json::Map::from_iter([("type".into(), json!("object"))]),
        }],
        mode,
        max_output_tokens: 128,
    }
}

async fn run(provider: &LocalProvider, mode: RoundMode) -> Result<Vec<StreamEvent>, ProviderError> {
    let (cancel, _) = AbortHandle::new_pair();
    provider
        .stream_turn(round(mode), Cancel(cancel))
        .await?
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect()
}

fn frame(value: Value) -> String {
    format!("data: {value}\r\n\r\n")
}
fn delta(value: Value, reason: Value) -> String {
    frame(json!({"choices": [{"index":0,"delta":value,"finish_reason":reason}]}))
}
fn finish() -> String {
    delta(json!({}), json!("stop"))
}
fn usage(input: u64, output: u64) -> Value {
    json!({"prompt_tokens":input,"completion_tokens":output})
}
fn call(name: &str, args: &str) -> Value {
    json!({"index":0,"id":"sample-call","type":"function","function":{"name":name,"arguments":args}})
}

#[test]
fn explicit_https_or_direct_loopback_and_tailscale_only() {
    for address in [
        "https://mac.tailnet.example:443/v1",
        "http://localhost:7618",
        "http://127.0.0.2/v1",
        "http://[::1]:8080",
        "http://sample.example.ts.net:11434/v1",
        "http://sample.example.ts.net./v1",
        "http://100.64.0.1:3/v1",
        "http://100.127.255.254:3",
        "http://[fd7a:115c:a1e0::1]/v1",
    ] {
        let provider = LocalProvider::new(address, "").unwrap();
        assert!(provider.base_url().ends_with("/v1/"));
    }
    for address in [
        "http://sample.example.ts.net.evil.example",
        "http://not-ts.net",
        "http://ts.net",
        "http://100.63.255.255",
        "http://100.128.0.1",
        "http://192.168.0.4",
        "http://[fd7a:115c:a1e1::1]",
        "http://[::ffff:100.64.0.1]",
        "http://sample.example.ts.net@unrelated.example",
        "http://@sample.example.ts.net",
        "https://key@sample.example.ts.net",
        "http://localhost:0",
        "http://localhost:65536",
        "http://localhost:",
        "https://mac.tailnet.example/v1?query",
        "https://mac.tailnet.example/v1#fragment",
        "https:////mac.tailnet.example",
        "file:///v1",
        "https://mac.tailnet.example\\v1",
        "https://mac.tailnet.example/v1\n/chat",
    ] {
        assert!(LocalProvider::new(address, "").is_err(), "{address}");
    }
    assert_eq!(
        LocalProvider::new("https://mac.tailnet.example", "split\nkey").unwrap_err(),
        SetupError::Credential
    );
    assert!(
        !format!(
            "{:?}",
            LocalProvider::new("https://mac.tailnet.example", "fixture-key").unwrap()
        )
        .contains("fixture-key")
    );
}

#[tokio::test]
async fn public_https_requires_attestation_even_for_catalog_and_bearer() {
    let address = "https://unverified.example.com/v1";
    let provider = LocalProvider::new(address, "fixture-key").unwrap();
    let caps = provider.capabilities(MODEL);
    assert_eq!(
        (caps.data_class, caps.billing),
        (DataClass::Cloud, BillingClass::Unknown)
    );
    assert_eq!(
        endpoint_info(address, false).unwrap().destination,
        Destination::NotPrivate
    );
    assert_eq!(
        provider.list_models().await.unwrap_err(),
        ProviderError::NotPrivate
    );
    assert_eq!(
        run(&provider, RoundMode::Streaming).await.unwrap_err(),
        ProviderError::NotPrivate
    );
    let provider = LocalProvider::configured(address, "fixture-key", true).unwrap();
    let caps = provider.capabilities(MODEL);
    assert_eq!(
        (caps.data_class, caps.billing),
        (DataClass::Local, BillingClass::NoPerTokenCharge)
    );
    assert_eq!(
        endpoint_info(address, true).unwrap().destination,
        Destination::YourServer
    );
    assert_eq!(
        LocalProvider::configured("http://unverified.example.com/v1", "", true).unwrap_err(),
        SetupError::Insecure
    );
    for (address, class) in [
        ("https://localhost", Destination::ThisDevice),
        ("http://127.0.0.1", Destination::ThisDevice),
        ("https://sample.example.ts.net", Destination::Tailscale),
        ("http://100.64.0.1", Destination::Tailscale),
        ("https://[fd7a:115c:a1e0::1]", Destination::Tailscale),
    ] {
        let caps = LocalProvider::new(address, "").unwrap().capabilities(MODEL);
        assert_eq!(
            (caps.data_class, caps.billing),
            (DataClass::Local, BillingClass::NoPerTokenCharge)
        );
        assert_eq!(endpoint_info(address, false).unwrap().destination, class);
    }
}

struct Interface(Arc<AtomicBool>);
impl TailnetInterface for Interface {
    fn connected(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[tokio::test]
async fn cleartext_tailnet_checks_interface_for_get_and_post_before_any_network() {
    for address in [
        "http://100.64.0.1/v1",
        "http://[fd7a:115c:a1e0::1]/v1",
        "http://sample.example.ts.net/v1",
    ] {
        let provider = LocalProvider::new(address, "fixture-key").unwrap();
        assert_eq!(
            provider.list_models().await.unwrap_err(),
            ProviderError::TailnetDisconnected
        );
        assert_eq!(
            run(&provider, RoundMode::Streaming).await.unwrap_err(),
            ProviderError::TailnetDisconnected
        );
    }
    let server = Server::start(|_| Reply::json(json!({"data": [{"id": MODEL}]}))).await;
    let port = Url::parse(&server.address).unwrap().port().unwrap();
    let link = Arc::new(AtomicBool::new(false));
    let mut provider = LocalProvider::new(
        &format!("http://sample.example.ts.net:{port}/v1"),
        "fixture-key",
    )
    .unwrap()
    .with_tailnet_interface(Interface(link.clone()));
    // Inject only the connector in this fixture. A positive interface result reaches loopback,
    // never a real tailnet server; production DNS filtering is unchanged.
    provider.client = Client::builder()
        .no_proxy()
        .resolve("sample.example.ts.net", ([127, 0, 0, 1], port).into())
        .build()
        .unwrap();
    assert_eq!(
        provider.list_models().await.unwrap_err(),
        ProviderError::TailnetDisconnected
    );
    assert!(server.calls().is_empty());
    link.store(true, Ordering::SeqCst);
    assert_eq!(provider.list_models().await.unwrap()[0].id, MODEL);
    assert!(
        server.calls()[0]
            .head
            .to_ascii_lowercase()
            .contains("authorization: bearer fixture-key")
    );
    link.store(false, Ordering::SeqCst);
    assert_eq!(
        run(&provider, RoundMode::Streaming).await.unwrap_err(),
        ProviderError::TailnetDisconnected
    );
    assert_eq!(server.calls().len(), 1);
}

#[tokio::test]
async fn invalid_model_ids_are_not_listed_and_retry_after_is_bounded() {
    let server = Server::start(|request| if request.head.starts_with("GET") {
        Reply::json(json!({"data": [{"id":"safe/model"}, {"id":"bad\u{202e}id"}, {"id":"bad\u{200b}id"}, {"id":"bad\u{2028}id"}]}))
    } else { Reply::status("429 Too Many Requests").header("Retry-After", u64::MAX.to_string()) }).await;
    let provider = server.provider();
    assert_eq!(
        provider
            .list_models()
            .await
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect::<Vec<_>>(),
        ["safe/model"]
    );
    assert_eq!(
        run(&provider, RoundMode::Streaming).await.unwrap_err(),
        ProviderError::RateLimited {
            retry_after: Some(MAX_RETRY_AFTER)
        }
    );
}

#[test]
fn refinement_context_is_an_assistant_message_before_the_owners_latest_prompt() {
    let mut request = round(RoundMode::Streaming);
    request.previous_draft = Some(Vec::new());
    let body = payload(&request);
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(
        messages
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["system", "user", "assistant", "user"]
    );
    assert!(
        messages[2]["content"]
            .as_str()
            .unwrap()
            .contains("not applied")
    );
    assert_eq!(messages[3]["content"], "Test prompt");

    request.history.splice(
        ..0,
        [
            Message::User {
                text: "Earlier question".into(),
            },
            Message::Assistant {
                text: "Earlier answer".into(),
                calls: vec![],
            },
        ],
    );
    let body = payload(&request);
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 4);
    assert!(
        messages[2]["content"]
            .as_str()
            .unwrap()
            .starts_with("Earlier answer")
    );
    assert!(
        messages[2]["content"]
            .as_str()
            .unwrap()
            .contains("not applied")
    );
    assert_eq!(messages[3]["content"], "Test prompt");
}

#[tokio::test]
async fn no_network_on_construction_or_capability_lookup_and_catalog_requires_a_gesture() {
    let server = Server::start(|_| Reply::json(json!({"object":"list","data":[{"id":"one","object":"model"},{"id":"one"},{"id":"two"}]}))).await;
    let provider = server.provider();
    assert!(!provider.capabilities("one").tools);
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(server.calls().is_empty());
    let models = provider.list_models().await.unwrap();
    assert_eq!(
        models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["one", "two"]
    );
    let [request] = &server.calls()[..] else {
        panic!("only the explicit GET should run");
    };
    assert!(request.head.starts_with("GET /gateway/v1/models "));
    assert!(!request.head.to_ascii_lowercase().contains("authorization:"));
    assert_eq!(request.body, Value::Null);
    assert!(
        !provider.capabilities("one").tools,
        "listing is not proof of tools"
    );
}

#[tokio::test]
async fn bearer_is_separate_required_and_never_redirected() {
    let other = Server::start(|_| Reply::status("500 Internal Server Error")).await;
    let redirect = format!("{}models", other.address);
    let server = Server::start(move |req| {
        if req
            .head
            .lines()
            .any(|line| line.eq_ignore_ascii_case("authorization: Bearer fixture-key"))
        {
            Reply::status("307 Temporary Redirect").header("Location", redirect.clone())
        } else {
            Reply::status("401 Unauthorized")
        }
    })
    .await;
    assert_eq!(
        server.provider().check_connection().await,
        Err(ProviderError::Unauthorized)
    );
    let with_key = LocalProvider::new(&server.address, "fixture-key").unwrap();
    assert_eq!(
        with_key.check_connection().await,
        Err(ProviderError::InvalidResponse)
    );
    assert_eq!(server.calls().len(), 2);
    assert!(other.calls().is_empty());
}

#[tokio::test]
async fn ollama_style_empty_arguments_and_duplicate_usage_are_normalized() {
    let server = Server::start(|_| Reply::sse([
        delta(json!({"role":"assistant","content":"Ready to check"}), Value::Null),
        delta(json!({"tool_calls":[call("list_lists", "")]}), Value::Null),
        frame(json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":usage(6, 2)})),
        frame(json!({"choices":[],"usage":usage(6, 3)})),
        "data: [DONE]\r\n\r\n".into(),
    ].concat())).await;
    let events = run(&server.provider(), RoundMode::Streaming).await.unwrap();
    assert!(
        matches!(&events[..], [StreamEvent::Text(_), StreamEvent::Usage(u), StreamEvent::Tool(c), StreamEvent::Finished] if u.input_tokens == 6 && u.output_tokens == 3 && c.arguments.is_empty())
    );
    let request = &server.calls()[0];
    assert!(
        request
            .head
            .starts_with("POST /gateway/v1/chat/completions ")
    );
    assert_eq!(request.body["parallel_tool_calls"], false);
    assert_eq!(request.body["stream"], true);
    assert_eq!(request.body["max_tokens"], 128);
    assert_eq!(
        request.body["tools"][0]["function"]["parameters"],
        json!({"type":"object"})
    );
}

#[tokio::test]
async fn lm_studio_style_split_tool_fields_and_utf8_stream() {
    let server = Server::start(|_| {
        Reply::sse(
            [
                delta(json!({"content":"Plán"}), Value::Null),
                delta(json!({"tool_calls":[call("list_lists", "{")]}), Value::Null),
                delta(
                    json!({"tool_calls":[{"index":0,"function":{"arguments":"}"}}]}),
                    Value::Null,
                ),
                delta(json!({}), json!("tool_calls")),
                "data: [DONE]\r\n\r\n".into(),
            ]
            .concat(),
        )
    })
    .await;
    let events = run(&server.provider(), RoundMode::Streaming).await.unwrap();
    assert!(
        matches!(&events[..], [StreamEvent::Text(s), StreamEvent::Tool(c), StreamEvent::Finished] if s == "Plán" && c.name == "list_lists")
    );
}

#[tokio::test]
async fn llama_cpp_style_buffered_tool_without_index_and_mlx_style_stream_without_done() {
    let server = Server::start(|req| if req.body["stream"] == false {
        Reply::json(json!({"choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"sample-call","type":"function","function":{"name":"list_lists","arguments":"{}"}}]},"finish_reason":"tool_calls"}],"usage":usage(11, 2)}))
    } else {
        Reply::sse([delta(json!({"role":"assistant","content":"Sample result"}), Value::Null), finish()].concat())
    }).await;
    let provider = server.provider();
    let events = run(&provider, RoundMode::Buffered).await.unwrap();
    assert!(matches!(
        &events[..],
        [
            StreamEvent::Usage(_),
            StreamEvent::Tool(_),
            StreamEvent::Finished
        ]
    ));
    let events = run(&provider, RoundMode::Streaming).await.unwrap();
    assert!(matches!(
        &events[..],
        [StreamEvent::Text(_), StreamEvent::Finished]
    ));
}

#[tokio::test]
async fn explicit_probe_checks_tool_result_round_trip_for_that_model_only() {
    let server = Server::start(|request| {
        if request.body["stream"] == true {
            return Reply::status("400 Bad Request");
        }
        let messages = request.body["messages"].as_array().unwrap();
        if let Some(result) = messages.iter().find(|message| message["role"] == "tool") {
            assert_eq!(result["tool_call_id"], "sample-call");
            assert_eq!(messages[messages.len() - 2]["tool_calls"][0]["id"], "sample-call");
            let marker: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
            Reply::json(json!({"choices":[{"index":0,"message":{"content":marker["marker"]},"finish_reason":"stop"}]}))
        } else {
            Reply::json(json!({"choices":[{"index":0,"message":{"tool_calls":[call("connection_check", "{}") ]},"finish_reason":"tool_calls"}]}))
        }
    }).await;
    let provider = server.provider();
    let support = provider.probe(MODEL).await.unwrap();
    assert_eq!(support, Support::BufferedTools);
    assert_eq!(server.calls().len(), 3);
    let provider = provider.with_support(MODEL.into(), support);
    assert!(provider.capabilities(MODEL).tools);
    assert!(!provider.capabilities(MODEL).streaming_tools);
    assert!(!provider.capabilities("other-model").tools);
}

#[tokio::test]
async fn broken_streams_and_rate_limits_never_replay_a_turn_recovery_is_manual() {
    let requests = AtomicUsize::new(0);
    let server = Server::start(move |_| match requests.fetch_add(1, Ordering::SeqCst) {
        0 => Reply::sse(delta(
            json!({"tool_calls":[call("list_lists", "{}")] }),
            Value::Null,
        )),
        1 => Reply::status("429 Too Many Requests").header("Retry-After", "45"),
        2 => Reply::status("404 Not Found"),
        3 => Reply::sse(delta(json!({"content":"Partial"}), json!("length"))),
        _ => Reply::sse([delta(json!({"content":"Ready"}), Value::Null), finish()].concat()),
    })
    .await;
    let provider = server.provider();
    let (cancel, _) = AbortHandle::new_pair();
    let mut broken = provider
        .stream_turn(round(RoundMode::Streaming), Cancel(cancel))
        .await
        .unwrap();
    assert!(matches!(
        broken.next().await,
        Some(Err(ProviderError::Incomplete))
    ));
    assert!(broken.next().await.is_none());
    let denied = provider.probe(MODEL).await.unwrap_err();
    assert_eq!(
        denied,
        ProviderError::RateLimited {
            retry_after: Some(Duration::from_secs(45))
        }
    );
    assert_eq!(
        server.calls().len(),
        2,
        "rate limit did not trigger buffered or text fallbacks"
    );
    assert_eq!(
        run(&provider, RoundMode::Streaming).await.unwrap_err(),
        ProviderError::ModelMissing
    );
    assert_eq!(
        run(&provider, RoundMode::Streaming).await.unwrap_err(),
        ProviderError::Limit
    );
    assert!(run(&provider, RoundMode::Streaming).await.is_ok());
    assert_eq!(server.calls().len(), 5);
}

#[tokio::test]
async fn unreachable_provider_is_reported_without_replaying() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let unavailable = LocalProvider::new(&format!("http://{addr}"), "").unwrap();
    assert_eq!(
        unavailable.check_connection().await,
        Err(ProviderError::Unavailable)
    );
    assert_eq!(
        unavailable.probe(MODEL).await,
        Err(ProviderError::Unavailable)
    );
}

#[tokio::test]
async fn a_connected_but_sleeping_server_times_out_without_retry() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut provider =
        LocalProvider::new(&format!("http://{}", listener.local_addr().unwrap()), "").unwrap();
    provider.client = Client::builder()
        .no_proxy()
        .read_timeout(Duration::from_millis(60))
        .retry(reqwest::retry::never())
        .build()
        .unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let counted = requests.clone();
    let server = tokio::spawn(async move {
        let mut connections = Vec::new();
        while let Ok((connection, _)) = listener.accept().await {
            counted.fetch_add(1, Ordering::SeqCst);
            connections.push(connection);
        }
    });
    assert_eq!(provider.probe(MODEL).await, Err(ProviderError::Timeout));
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    server.abort();
}
