//! Shared Chat Completions request encoding. The controller owns history and egress policy.

use super::provider::{DataClass, Message, RoundMode, RoundRequest};
use serde_json::{Value, json};
use url::{Host, Url};

/// Normalize the browser's Chat Completions URL and classify it without trusting a hostname
/// or asking the browser to attest to its network route. Only exact loopback literals in the
/// application CSP are local; every other allowed address uses the cloud projection.
pub fn browser_endpoint(input: &str) -> Option<(Url, DataClass)> {
    let input = input.trim();
    let (_, rest) = input.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if input.len() > 2048
        || input
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
        || authority.is_empty()
        || authority.contains('@')
        || authority.ends_with(':')
    {
        return None;
    }
    let mut url = Url::parse(input).ok()?;
    if url.query().is_some() || url.fragment().is_some() || url.port() == Some(0) {
        return None;
    }
    let local = matches!(url.host()?, Host::Ipv4(ip) if ip == std::net::Ipv4Addr::LOCALHOST);
    match url.scheme() {
        "https" => {}
        "http" if local => {}
        _ => return None,
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
    Some((
        url,
        if local {
            DataClass::Local
        } else {
            DataClass::Cloud
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_endpoints_are_normalized_and_classified_conservatively() {
        for (input, expected, class) in [
            (
                "http://127.0.0.1:11434",
                "http://127.0.0.1:11434/v1/chat/completions",
                DataClass::Local,
            ),
            (
                "http://127.0.0.1:11434/v1/",
                "http://127.0.0.1:11434/v1/chat/completions",
                DataClass::Local,
            ),
            (
                "https://todo.example.com/model/v1/chat/completions",
                "https://todo.example.com/model/v1/chat/completions",
                DataClass::Cloud,
            ),
            (
                "https://localhost/v1",
                "https://localhost/v1/chat/completions",
                DataClass::Cloud,
            ),
        ] {
            let (url, data_class) = browser_endpoint(input).unwrap();
            assert_eq!(url.as_str(), expected);
            assert_eq!(data_class, class);
        }
        for invalid in [
            "https://you@todo.example.com",
            "https://todo.example.com?token=one",
            "https://todo.example.com/#fragment",
            "http://todo.example.com",
            "file:///tmp/model",
            "http://127.0.0.2",
            "http://127.0.0.1:0",
        ] {
            assert!(browser_endpoint(invalid).is_none(), "{invalid}");
        }
    }
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
