#![cfg(feature = "assistant")]
#![allow(clippy::unwrap_used)]

use serde_json::{Value, json};
use todo_client::assistant::provider::{ProviderError, StreamEvent, Usage};
use todo_client::assistant::wire::RoundDecoder;

fn sse(body: Value) -> String {
    format!("data: {body}\r\n\r\n")
}

fn choice(delta: Value, finish: Option<&str>) -> String {
    sse(json!({"choices":[{"index":0,"delta":delta,"finish_reason":finish}]}))
}

#[test]
fn accepts_every_fragment_boundary_including_utf8_and_split_tool_json() {
    let wire = [
        ":heartbeat\r\n\r\n".into(),
        choice(json!({"content":"Plán 🌱"}), None),
        choice(
            json!({"tool_calls":[{"index":0,"id":"call-1","type":"function",
            "function":{"name":"create_task","arguments":"{\"title\":\"Ž"}}]}),
            None,
        ),
        choice(
            json!({"tool_calls":[{"index":0,"function":{"arguments":"luťoučký\"}"}}]}),
            None,
        ),
        choice(json!({}), Some("tool_calls")),
        sse(json!({"choices":[],"usage":{"prompt_tokens":11,"completion_tokens":7}})),
    ]
    .concat();
    for marker in ["", "data: [DONE]\r\n\r\n"] {
        let wire = format!("{wire}{marker}");
        for width in 1..=41 {
            let mut decoder = RoundDecoder::sse();
            let mut events = Vec::new();
            for bytes in wire.as_bytes().chunks(width) {
                events.extend(decoder.push(bytes).unwrap());
            }
            events.extend(decoder.finish().unwrap());
            let [
                StreamEvent::Text(text),
                StreamEvent::Usage(usage),
                StreamEvent::Tool(call),
                StreamEvent::Finished,
            ] = events.as_slice()
            else {
                panic!("expected prose, usage, a complete call, and success");
            };
            assert_eq!(text, "Plán 🌱");
            assert_eq!(
                *usage,
                Usage {
                    input_tokens: 11,
                    output_tokens: 7
                }
            );
            assert_eq!(call.id, "call-1");
            assert_eq!(call.name, "create_task");
            assert_eq!(call.arguments["title"], "Žluťoučký");
        }
    }
}

#[test]
fn buffered_round_accepts_chunked_json_but_never_an_incomplete_object() {
    let mut decoder = RoundDecoder::json();
    let wire = json!({"choices":[{"index":0,"message":{"tool_calls":[{
        "id":"one", "type":"function", "function":{"name":"create_task","arguments":"{\"title\":\"Ž\"}"}
    }]},"finish_reason":"tool_calls"}]}).to_string();
    for byte in wire.as_bytes() {
        assert!(decoder.push(&[*byte]).unwrap().is_empty());
    }
    let events = decoder.finish().unwrap();
    assert!(
        matches!(events.as_slice(), [StreamEvent::Tool(call), StreamEvent::Finished] if call.arguments["title"] == "Ž")
    );
    let mut decoder = RoundDecoder::json();
    decoder.push(&wire.as_bytes()[..wire.len() - 1]).unwrap();
    assert_eq!(
        decoder.finish().unwrap_err(),
        ProviderError::InvalidResponse
    );
}

#[test]
fn missing_finish_reason_never_becomes_success_even_with_well_formed_arguments() {
    let mut decoder = RoundDecoder::sse();
    let complete_args = choice(
        json!({"tool_calls":[{"index":0,"id":"one",
        "function":{"name":"create_task","arguments":"{\"title\":\"Draft\"}"}}]}),
        None,
    );
    assert!(decoder.push(complete_args.as_bytes()).unwrap().is_empty());
    assert_eq!(decoder.finish().unwrap_err(), ProviderError::Incomplete);

    let mut decoder = RoundDecoder::sse();
    decoder.push(complete_args.as_bytes()).unwrap();
    assert_eq!(
        decoder.push(b"data: [DONE]\n\n").unwrap_err(),
        ProviderError::Incomplete
    );

    let mut decoder = RoundDecoder::sse();
    decoder
        .push(choice(json!({}), Some("stop")).as_bytes())
        .unwrap();
    decoder.push(b"data: {").unwrap();
    assert_eq!(decoder.finish().unwrap_err(), ProviderError::Incomplete);
}

#[test]
fn rejects_invalid_utf8_duplicate_arguments_and_truncated_generation() {
    let mut decoder = RoundDecoder::sse();
    assert_eq!(
        decoder.push(b"data: \xff\n\n").unwrap_err(),
        ProviderError::InvalidResponse
    );

    for arguments in ["[1]", "{", r#"{"title":"one","title":"two"}"#, "{}{}"] {
        let mut decoder = RoundDecoder::sse();
        let body = choice(
            json!({"tool_calls":[{"index":0,"id":"call",
            "function":{"name":"create_task","arguments":arguments}}]}),
            Some("tool_calls"),
        );
        assert!(decoder.push(body.as_bytes()).unwrap().is_empty());
        assert_eq!(
            decoder.finish().unwrap_err(),
            ProviderError::InvalidResponse
        );
    }

    let mut decoder = RoundDecoder::sse();
    assert_eq!(
        decoder
            .push(choice(json!({}), Some("length")).as_bytes())
            .unwrap_err(),
        ProviderError::Limit
    );
    let mut decoder = RoundDecoder::sse();
    assert_eq!(
        decoder.push(&vec![b'x'; 33 * 1024]).unwrap_err(),
        ProviderError::Limit
    );
}
