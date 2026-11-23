#![cfg(feature = "assistant")]
#![allow(clippy::unwrap_used)]

use proptest::prelude::*;
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

fn tool_delta(index: usize, id: String) -> Value {
    json!({"index": index, "id": id, "type":"function", "function":{"name":"list_lists", "arguments":"{}"}})
}

#[test]
fn rejects_body_and_tool_count_overflow_and_duplicate_usage() {
    for mut decoder in [RoundDecoder::json(), RoundDecoder::sse()] {
        // A body of exactly 256 KiB, fragmented well below the per-frame cap.
        let comments = b":..\n\n".repeat(52_428);
        assert!(decoder.push(&comments).unwrap().is_empty());
        assert!(decoder.push(b":.\n\n").unwrap().is_empty());
        assert_eq!(decoder.push(b"\n").unwrap_err(), ProviderError::Limit);
    }
    let calls = (0..9)
        .map(|i| tool_delta(i, i.to_string()))
        .collect::<Vec<_>>();
    let mut decoder = RoundDecoder::sse();
    let body = choice(json!({"tool_calls":calls}), Some("tool_calls"));
    assert_eq!(
        decoder.push(body.as_bytes()).unwrap_err(),
        ProviderError::Limit
    );

    let usage = sse(json!({"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":7}}));
    let mut decoder = RoundDecoder::sse();
    assert!(matches!(
        decoder.push(usage.as_bytes()).unwrap().as_slice(),
        [StreamEvent::Usage(_)]
    ));
    assert_eq!(
        decoder.push(usage.as_bytes()).unwrap_err(),
        ProviderError::InvalidResponse
    );
}

fn decode(json_mode: bool, bytes: &[u8], widths: &[usize]) -> (Vec<Value>, Option<ProviderError>) {
    let mut decoder = if json_mode {
        RoundDecoder::json()
    } else {
        RoundDecoder::sse()
    };
    let mut out = Vec::new();
    let mut add = |events: Vec<StreamEvent>| {
        out.extend(events.into_iter().map(|event| match event {
            StreamEvent::Text(text) => json!({"text":text}),
            StreamEvent::Tool(tool) => json!({"tool":tool}),
            StreamEvent::Usage(usage) => json!({"usage":[usage.input_tokens,usage.output_tokens]}),
            StreamEvent::Finished => json!({"finished":true}),
        }));
    };
    let mut rest = bytes;
    for &width in widths {
        let (chunk, next) = rest.split_at(rest.len().min(width));
        match decoder.push(chunk) {
            Ok(events) => add(events),
            Err(err) => return (out, Some(err)),
        }
        rest = next;
    }
    match decoder.push(rest).and_then(|mut events| {
        add(std::mem::take(&mut events));
        decoder.finish()
    }) {
        Ok(events) => {
            add(events);
            (out, None)
        }
        Err(err) => (out, Some(err)),
    }
}

fn check_wire(json_mode: bool, bytes: &[u8], widths: &[usize]) -> Result<(), TestCaseError> {
    let expected = decode(json_mode, bytes, &[]);
    let actual = decode(json_mode, bytes, widths);
    prop_assert_eq!(&actual, &expected);
    let (events, error) = actual;
    if events.iter().any(|e| e.get("tool").is_some()) {
        prop_assert_eq!(events.last(), Some(&json!({"finished":true})));
        prop_assert!(error.is_none());
    }
    if error.is_some() {
        prop_assert!(events.iter().all(|e| e.get("finished").is_none()));
    }
    Ok(())
}

fn tool_stream() -> String {
    choice(
        json!({"tool_calls":[tool_delta(0, "one".into())]}),
        Some("tool_calls"),
    )
}

proptest! {
    #[test]
    fn arbitrary_bytes_and_chunking_are_split_invariant_and_never_emit_partial_tools(
        random in prop::collection::vec(any::<u8>(), 0..400),
        widths in prop::collection::vec(0usize..65, 0..50),
        mode in 0u8..5,
    ) {
        let mut wire = match mode {
            1 => choice(json!({"content":"Unicode Δ 🌱"}), None).into_bytes(),
            2 => tool_stream().into_bytes(),
            3 => format!("{}data: [DONE]\n\n", tool_stream()).into_bytes(),
            _ => Vec::new(),
        };
        wire.extend(random);
        check_wire(mode == 4, &wire, &widths)?;
        check_wire(false, tool_stream().as_bytes(), &widths)?;
    }
}

#[test]
fn done_is_not_a_license_to_ignore_trailing_or_conflicting_events() {
    let good = tool_stream();
    for suffix in [
        "data: [DONE]\n\ndata: [DONE]\n\n".into(),
        "data: [DONE]\n\ndata: {".into(),
        "data: [DONE]\n\nevent: error\ndata: {}\n\n".into(),
        choice(json!({"content":"late"}), None),
        choice(json!({}), Some("stop")),
    ] {
        let input = format!("{good}{suffix}");
        check_wire(false, input.as_bytes(), &vec![1; input.len()]).unwrap();
        let (out, error) = decode(false, input.as_bytes(), &[]);
        assert!(out.is_empty() && error.is_some());
    }
    for delta in [
        json!({"tool_calls":[tool_delta(2,"gap".into())]}),
        json!({"tool_calls":[{"index":0,"type":"unknown"}]}),
        json!({"function_call":{}}),
        json!({"refusal":"No"}),
        json!({"tool_calls":[tool_delta(0,"duplicate".into()),tool_delta(1,"duplicate".into())]}),
    ] {
        let input = choice(delta, Some("tool_calls"));
        check_wire(false, input.as_bytes(), &vec![1; input.len()]).unwrap();
        assert!(decode(false, input.as_bytes(), &[]).1.is_some());
    }
}
