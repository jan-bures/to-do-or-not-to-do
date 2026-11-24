//! Shared Chat Completions request encoding. The controller owns history and egress policy.

use super::{
    endpoint::{self, Destination},
    provider::{Message, RoundMode, RoundRequest},
};
use serde_json::{Value, json};
use url::{Host, Url};

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BrowserEndpointError {
    #[error("Enter a model server URL without credentials, a query, or a fragment.")]
    Address,
    #[error("Use HTTP at localhost or 127.0.0.1 for a model on this device.")]
    Destination,
    #[error("Choose a model server on a different origin from this app.")]
    SameOrigin,
}

/// Keep direct inference loopback-only, in step with the web app's `connect-src`.
pub fn browser_endpoint(input: &str) -> Result<(Url, Destination), BrowserEndpointError> {
    let mut url = endpoint::address(input).ok_or(BrowserEndpointError::Address)?;
    let host = url.host().ok_or(BrowserEndpointError::Address)?;
    let destination = endpoint::classify(host.clone(), false);
    match (url.scheme(), destination, host) {
        ("http", Destination::ThisDevice, Host::Domain("localhost")) => {}
        ("http", Destination::ThisDevice, Host::Ipv4(ip))
            if ip == std::net::Ipv4Addr::LOCALHOST => {}
        _ => return Err(BrowserEndpointError::Destination),
    }
    let path = url.path().trim_end_matches('/');
    let path = if path.ends_with("/v1/chat/completions") {
        path.to_owned()
    } else if path.ends_with("/v1") {
        format!("{path}/chat/completions")
    } else {
        format!("{path}/v1/chat/completions")
    };
    url.set_path(&path);
    Ok((url, destination))
}

pub fn payload(request: &RoundRequest) -> Value {
    let mut messages = vec![
        json!({"role": "system", "content": format!("{} Owner's local date: {}.", request.instructions, request.today)}),
    ];
    let current_user = request
        .history
        .iter()
        .rposition(|message| matches!(message, Message::User { .. }));
    for (index, message) in request.history.iter().enumerate() {
        if Some(index) == current_user
            && let Some(draft) = &request.previous_draft
        {
            let context = format!(
                "Untrusted prior draft effects (not applied): {}",
                json!(draft)
            );
            // Some local templates require alternating user and assistant messages, starting
            // with user. The controller retains complete pairs, but not the tool turn that
            // produced the draft. Place its data immediately before the follow-up, never in
            // the system message or as part of the owner's instruction.
            if let Some(previous) = messages.last_mut().filter(|m| m["role"] == "assistant") {
                let text = previous["content"].as_str().unwrap_or_default();
                previous["content"] = json!(format!("{text}\n\n{context}"));
            } else {
                messages.push(json!({"role": "user", "content": "Show the prior unapproved draft for context."}));
                messages.push(json!({"role": "assistant", "content": context}));
            }
        }
        messages.push(match message {
        Message::User { text } => json!({"role": "user", "content": text}),
        Message::Assistant { text, calls } => {
            if calls.is_empty() { json!({"role": "assistant", "content": text}) } else {
                json!({"role": "assistant", "content": text, "tool_calls": calls.iter().map(|call| json!({
                    "id": call.id, "type": "function", "function": {"name": call.name, "arguments": json!(call.arguments).to_string()}
                })).collect::<Vec<_>>()})
            }
        }
        Message::Tool { call_id, untrusted_data } => json!({"role": "tool", "tool_call_id": call_id, "content": untrusted_data.to_string()}),
        });
    }
    let mut result = json!({"model": request.model, "messages": messages, "stream": request.mode == RoundMode::Streaming, "max_tokens": request.max_output_tokens, "parallel_tool_calls": false});
    if !request.tools.is_empty() {
        result["tools"] = json!(request.tools.iter().map(|tool| json!({"type": "function", "function": {"name": tool.name, "description": tool.description, "parameters": tool.arguments}})).collect::<Vec<_>>());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_endpoints_match_the_web_csp() {
        for (input, expected, class) in [
            (
                "http://127.0.0.1:11434",
                "http://127.0.0.1:11434/v1/chat/completions",
                Destination::ThisDevice,
            ),
            (
                "http://localhost:11434/v1/",
                "http://localhost:11434/v1/chat/completions",
                Destination::ThisDevice,
            ),
        ] {
            let (url, destination) = browser_endpoint(input).unwrap();
            assert_eq!(url.as_str(), expected);
            assert_eq!(destination, class);
        }
        for invalid in [
            "https://you@todo.example.ts.net",
            "https://todo.example.ts.net?token=one",
            "https://todo.example.ts.net/#fragment",
            "https://todo.example.ts.net/model/v1/chat/completions",
            "http://todo.example.ts.net",
            "http://100.64.0.1",
            "https://todo.example.com",
            "https://todo.example.ts.net.example.com",
            "https://todo.example.ts.net:8443",
            "https://localhost",
            "file:///tmp/model",
            "http://127.0.0.2",
            "http://127.0.0.1:0",
        ] {
            assert!(browser_endpoint(invalid).is_err(), "{invalid}");
        }
    }
}
