use serde_json::{json, Value};

use crate::search;

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Default)]
pub struct Partial {
    pub content: String,
    pub thinking: String,
    pub tool_calls: Vec<ToolCall>,
    pub done: bool,
}

pub fn normalize_base(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30 * 60))
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())
}

pub fn list_models(base: &str) -> Result<Vec<String>, String> {
    let response = client()?
        .get(format!("{}/api/tags", normalize_base(base)))
        .send()
        .map_err(|e| format!("Cannot reach Ollama: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("Ollama returned {}", response.status()));
    }
    let value: Value = response.json().map_err(|e| e.to_string())?;
    let mut names = Vec::new();
    if let Some(models) = value.get("models").and_then(|m| m.as_array()) {
        for model in models {
            if let Some(name) = model.get("name").and_then(|n| n.as_str()) {
                names.push(name.to_string());
            }
        }
    }
    Ok(names)
}

pub fn supports_tools(base: &str, model: &str) -> bool {
    let Ok(response) = client().and_then(|c| {
        c.post(format!("{}/api/show", normalize_base(base)))
            .json(&json!({ "model": model }))
            .send()
            .map_err(|e| e.to_string())
    }) else {
        return false;
    };
    let Ok(value) = response.json::<Value>() else {
        return false;
    };
    value
        .get("capabilities")
        .and_then(|c| c.as_array())
        .map(|caps| caps.iter().any(|c| c.as_str() == Some("tools")))
        .unwrap_or(false)
}

pub fn running_models(base: &str) -> Result<Vec<String>, String> {
    let response = client()?
        .get(format!("{}/api/ps", normalize_base(base)))
        .send()
        .map_err(|e| e.to_string())?;
    let value: Value = response.json().map_err(|e| e.to_string())?;
    let mut names = Vec::new();
    if let Some(models) = value.get("models").and_then(|m| m.as_array()) {
        for model in models {
            if let Some(name) = model.get("name").and_then(|n| n.as_str()) {
                names.push(name.to_string());
            }
        }
    }
    Ok(names)
}

pub fn unload_all(base: &str) -> Result<(), String> {
    let base = normalize_base(base);
    let models = running_models(&base)?;
    if models.is_empty() {
        return Ok(());
    }
    let client = client()?;
    for model in models {
        let _ = client
            .post(format!("{base}/api/generate"))
            .json(&json!({ "model": model, "keep_alive": 0, "prompt": "" }))
            .send();
    }
    Ok(())
}

pub fn keep_alive_value(raw: &str) -> Value {
    let raw = raw.trim();
    if raw.is_empty() {
        return json!("10m");
    }
    if let Ok(n) = raw.parse::<i64>() {
        json!(n)
    } else {
        json!(raw)
    }
}

pub fn tool_defs() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "web_search",
                "description": "Search the web for up-to-date facts, news, versions, prices, or events. Skip it for timeless reasoning, rewriting, and code that does not need current information.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "A concise web search query" }
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "web_fetch",
                "description": "Read one web page when search snippets are not enough to answer.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "Absolute http(s) URL" }
                    },
                    "required": ["url"]
                }
            }
        }
    ])
}

/// Streams one `/api/chat` round. `on_partial` sees the accumulated text so far.
pub fn stream_chat(
    base: &str,
    body: &Value,
    mut on_partial: impl FnMut(&Partial),
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<Partial, String> {
    use std::io::BufRead;
    use std::sync::atomic::Ordering;

    let response = client()?
        .post(format!("{}/api/chat", normalize_base(base)))
        .json(body)
        .send()
        .map_err(|e| format!("Cannot reach Ollama: {e}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().unwrap_or_default();
        return Err(format!(
            "Ollama returned {status}: {}",
            search::truncate(&text, 400)
        ));
    }

    let mut reader = std::io::BufReader::new(response);
    let mut line = String::new();
    let mut partial = Partial::default();
    loop {
        if cancelled.load(Ordering::Relaxed) {
            partial.done = true;
            break;
        }
        line.clear();
        let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        if let Some(err) = value.get("error").and_then(|e| e.as_str()) {
            return Err(err.to_string());
        }
        if let Some(message) = value.get("message") {
            if let Some(text) = message.get("content").and_then(|c| c.as_str()) {
                partial.content.push_str(text);
            }
            if let Some(text) = message.get("thinking").and_then(|c| c.as_str()) {
                partial.thinking.push_str(text);
            }
            if let Some(calls) = message.get("tool_calls") {
                merge_tool_calls(&mut partial.tool_calls, calls);
            }
        }
        partial.done = value.get("done").and_then(|d| d.as_bool()).unwrap_or(false);
        on_partial(&partial);
        if partial.done {
            break;
        }
    }
    Ok(partial)
}

pub fn merge_tool_calls(dst: &mut Vec<ToolCall>, incoming: &Value) {
    let Some(calls) = incoming.as_array() else {
        return;
    };
    for (index, call) in calls.iter().enumerate() {
        let name = call
            .pointer("/function/name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let arguments = normalize_arguments(
            call.pointer("/function/arguments")
                .cloned()
                .unwrap_or_else(|| json!({})),
        );
        if dst.len() <= index {
            dst.push(ToolCall { name, arguments });
            continue;
        }
        if !name.is_empty() {
            dst[index].name = name;
        }
        if arguments.as_object().map(|o| !o.is_empty()).unwrap_or(true) {
            dst[index].arguments = arguments;
        }
    }
}

fn normalize_arguments(value: Value) -> Value {
    match value {
        Value::String(text) => serde_json::from_str(&text).unwrap_or(json!({ "query": text })),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_streamed_tool_calls() {
        let mut dst = Vec::new();
        merge_tool_calls(
            &mut dst,
            &json!([{ "function": { "name": "web_search", "arguments": { "query": "ollama" } } }]),
        );
        assert_eq!(dst.len(), 1);
        assert_eq!(dst[0].name, "web_search");
        assert_eq!(dst[0].arguments["query"], "ollama");
    }

    #[test]
    fn parses_keep_alive() {
        assert_eq!(keep_alive_value("10m"), json!("10m"));
        assert_eq!(keep_alive_value("0"), json!(0));
    }
}
