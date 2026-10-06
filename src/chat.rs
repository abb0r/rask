use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};

use crate::ollama::{self, Partial, ToolCall};
use crate::search::{self, Hit};
use crate::store::ModelProfile;

const MAX_ROUNDS: u32 = 4;

#[derive(Clone)]
pub struct TurnRequest {
    pub base_url: String,
    pub model: String,
    pub profile: ModelProfile,
    pub idle_unload: String,
    pub provider: String,
    pub api_key: String,
    pub history: Vec<(String, String)>,
    pub tools: bool,
}

#[derive(Clone)]
pub struct TurnView {
    pub text: String,
    pub sources: Vec<Hit>,
    pub status: String,
    pub done: bool,
}

pub fn run(
    request: TurnRequest,
    cancel: &AtomicBool,
    mut on_view: impl FnMut(TurnView),
) -> Result<(), String> {
    let mut messages = Vec::new();
    let mut system = request.profile.system_prompt.trim().to_string();
    if request.tools {
        if !system.is_empty() {
            system.push_str("\n\n");
        }
        system.push_str(
            "You can call web_search and web_fetch. Use them when the answer depends on current facts. \
Cite the URLs you actually used. If the results do not contain the answer, say so.",
        );
    }
    if !system.is_empty() {
        messages.push(json!({ "role": "system", "content": system }));
    }
    for (role, content) in &request.history {
        messages.push(json!({ "role": role, "content": content }));
    }

    let mut sources = Vec::new();
    let mut final_text = String::new();

    for round in 0..MAX_ROUNDS {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let allow_tools = request.tools && round + 1 < MAX_ROUNDS;
        let body = chat_body(&request, &messages, allow_tools);
        let partial = ollama::stream_chat(
            &request.base_url,
            &body,
            |partial| {
                let text = visible_text(partial);
                on_view(TurnView {
                    text: text.clone(),
                    sources: sources.clone(),
                    status: if text.is_empty() && !partial.thinking.is_empty() {
                        "Thinking…".into()
                    } else {
                        String::new()
                    },
                    done: false,
                });
            },
            cancel,
        )?;

        if !partial.tool_calls.is_empty() && allow_tools && !cancel.load(Ordering::Relaxed) {
            let (tool_messages, hits, status) = run_tools(&request, &partial.tool_calls);
            sources.extend(hits);
            on_view(TurnView {
                text: visible_text(&partial),
                sources: sources.clone(),
                status,
                done: false,
            });
            messages.push(assistant_tool_message(&partial));
            messages.extend(tool_messages);
            continue;
        }

        final_text = visible_text(&partial);
        break;
    }

    on_view(TurnView {
        text: final_text,
        sources,
        status: String::new(),
        done: true,
    });
    Ok(())
}

fn visible_text(partial: &Partial) -> String {
    if !partial.content.is_empty() {
        partial.content.clone()
    } else {
        partial.thinking.clone()
    }
}

fn chat_body(request: &TurnRequest, messages: &[Value], tools: bool) -> Value {
    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "stream": true,
        "think": request.profile.think,
        "keep_alive": ollama::keep_alive_value(&request.idle_unload),
        "options": {
            "temperature": request.profile.temperature,
            "num_ctx": request.profile.num_ctx,
        }
    });
    if tools {
        body["tools"] = ollama::tool_defs();
    }
    body
}

fn assistant_tool_message(partial: &Partial) -> Value {
    json!({
        "role": "assistant",
        "content": partial.content,
        "tool_calls": partial.tool_calls.iter().map(|call| {
            json!({
                "function": {
                    "name": call.name,
                    "arguments": call.arguments
                }
            })
        }).collect::<Vec<_>>()
    })
}

fn run_tools(request: &TurnRequest, calls: &[ToolCall]) -> (Vec<Value>, Vec<Hit>, String) {
    let mut status = String::new();
    let results: Vec<(String, Vec<Hit>)> = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for call in calls {
            let provider = request.provider.clone();
            let key = request.api_key.clone();
            handles.push(scope.spawn(move || dispatch(&provider, &key, call)));
        }
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|_| ("tool failed".into(), Vec::new()))
            })
            .collect()
    });

    let mut messages = Vec::new();
    let mut hits = Vec::new();
    for (call, (text, call_hits)) in calls.iter().zip(results) {
        if status.is_empty() {
            status = tool_status(call);
        }
        hits.extend(call_hits);
        messages.push(json!({
            "role": "tool",
            "tool_name": call.name,
            "content": text,
        }));
    }
    (messages, hits, status)
}

fn tool_status(call: &ToolCall) -> String {
    let query = call
        .arguments
        .get("query")
        .or_else(|| call.arguments.get("url"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    match call.name.as_str() {
        "web_fetch" => format!("Opening {query}"),
        _ => format!("Searching: {query}"),
    }
}

fn dispatch(provider: &str, key: &str, call: &ToolCall) -> (String, Vec<Hit>) {
    match call.name.as_str() {
        "web_fetch" => {
            let url = call
                .arguments
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match search::web_fetch(provider, key, url) {
                Ok(hit) => {
                    let text = search::format_hits(std::slice::from_ref(&hit));
                    (text, vec![hit])
                }
                Err(err) => (format!("fetch failed: {err}"), Vec::new()),
            }
        }
        _ => {
            let query = call
                .arguments
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match search::web_search(provider, key, query) {
                Ok(hits) => (search::format_hits(&hits), hits),
                Err(err) => (format!("search failed: {err}"), Vec::new()),
            }
        }
    }
}
