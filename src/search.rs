use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct Hit {
    pub title: String,
    pub url: String,
    pub content: String,
}

pub fn web_search(provider: &str, key: &str, query: &str) -> Result<Vec<Hit>, String> {
    if key.trim().is_empty() {
        return Err("no search API key".into());
    }
    let query = query.trim();
    if query.is_empty() {
        return Err("empty search query".into());
    }
    match provider {
        "tavily" => tavily_search(key, query),
        "brave" => brave_search(key, query),
        "you" => you_search(key, query),
        _ => ollama_search(key, query),
    }
}

pub fn web_fetch(provider: &str, key: &str, url: &str) -> Result<Hit, String> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("only http(s) URLs can be fetched".into());
    }
    match provider {
        "tavily" if !key.trim().is_empty() => tavily_extract(key, url),
        "ollama" if !key.trim().is_empty() => ollama_fetch(key, url),
        "you" if !key.trim().is_empty() => you_contents(key, url).or_else(|_| fetch_html(url)),
        _ => fetch_html(url),
    }
}

pub fn format_hits(hits: &[Hit]) -> String {
    if hits.is_empty() {
        return "No results.".into();
    }
    hits.iter()
        .enumerate()
        .map(|(i, hit)| {
            format!(
                "[{}] {}\n{}\n{}",
                i + 1,
                hit.title,
                hit.url,
                truncate(&hit.content, 1200)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(25))
        .connect_timeout(std::time::Duration::from_secs(8))
        .user_agent("Rask/0.1")
        .build()
        .map_err(|e| e.to_string())
}

fn ollama_search(key: &str, query: &str) -> Result<Vec<Hit>, String> {
    let response = client()?
        .post("https://ollama.com/api/web_search")
        .bearer_auth(key.trim())
        .json(&json!({ "query": query, "max_results": 5 }))
        .send()
        .map_err(|e| e.to_string())?;
    parse_body(response)
}

fn ollama_fetch(key: &str, url: &str) -> Result<Hit, String> {
    let response = client()?
        .post("https://ollama.com/api/web_fetch")
        .bearer_auth(key.trim())
        .json(&json!({ "url": url }))
        .send()
        .map_err(|e| e.to_string())?;
    let body = read_body(response)?;
    let value: Value = serde_json::from_str(&body).unwrap_or(json!({}));
    Ok(Hit {
        title: text_at(&value, &["title"]).unwrap_or_else(|| url.to_string()),
        url: url.to_string(),
        content: text_at(&value, &["content", "text", "raw_content"]).unwrap_or(body),
    })
}

fn tavily_search(key: &str, query: &str) -> Result<Vec<Hit>, String> {
    let response = client()?
        .post("https://api.tavily.com/search")
        .bearer_auth(key.trim())
        .json(&json!({
            "query": query,
            "max_results": 5,
            "search_depth": "basic",
            "include_answer": false
        }))
        .send()
        .map_err(|e| e.to_string())?;
    parse_body(response)
}

fn tavily_extract(key: &str, url: &str) -> Result<Hit, String> {
    let response = client()?
        .post("https://api.tavily.com/extract")
        .bearer_auth(key.trim())
        .json(&json!({ "urls": [url] }))
        .send()
        .map_err(|e| e.to_string())?;
    let hits = parse_body(response)?;
    hits.into_iter()
        .next()
        .ok_or_else(|| "empty extract".into())
}

fn brave_search(key: &str, query: &str) -> Result<Vec<Hit>, String> {
    let response = client()?
        .get("https://api.search.brave.com/res/v1/web/search")
        .header("X-Subscription-Token", key.trim())
        .header("Accept", "application/json")
        .query(&[("q", query), ("count", "5")])
        .send()
        .map_err(|e| e.to_string())?;
    parse_body(response)
}

fn you_search(key: &str, query: &str) -> Result<Vec<Hit>, String> {
    let response = client()?
        .post("https://ydc-index.io/v1/search")
        .header("X-API-Key", key.trim())
        .json(&json!({
            "query": query,
            "count": 5,
            "extraction": { "extraction_mode": "highlights" }
        }))
        .send()
        .map_err(|e| e.to_string())?;
    parse_body(response)
}

fn you_contents(key: &str, url: &str) -> Result<Hit, String> {
    let response = client()?
        .post("https://ydc-index.io/v1/contents")
        .header("X-API-Key", key.trim())
        .json(&json!({ "urls": [url] }))
        .send()
        .map_err(|e| e.to_string())?;
    let hits = parse_body(response)?;
    hits.into_iter()
        .next()
        .ok_or_else(|| "empty contents".into())
}

fn fetch_html(url: &str) -> Result<Hit, String> {
    let response = client()?.get(url).send().map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("fetch {} ({})", response.status(), url));
    }
    let body = response.text().map_err(|e| e.to_string())?;
    let limited = truncate(&body, 400_000);
    Ok(Hit {
        title: url.to_string(),
        url: url.to_string(),
        content: truncate(&html_to_text(&limited), 4000),
    })
}

fn read_body(response: reqwest::blocking::Response) -> Result<String, String> {
    let status = response.status();
    let body = response.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("search HTTP {status}: {}", truncate(&body, 300)));
    }
    Ok(body)
}

fn parse_body(response: reqwest::blocking::Response) -> Result<Vec<Hit>, String> {
    let body = read_body(response)?;
    let value: Value =
        serde_json::from_str(&body).map_err(|e| format!("{e}: {}", truncate(&body, 200)))?;
    let hits = collect_hits(&value);
    if hits.is_empty() {
        Err(format!("no results: {}", truncate(&body, 300)))
    } else {
        Ok(hits.into_iter().take(5).collect())
    }
}

fn collect_hits(value: &Value) -> Vec<Hit> {
    let mut found = Vec::new();
    walk_hits(value, &mut found, 0);
    found
}

fn walk_hits(value: &Value, out: &mut Vec<Hit>, depth: usize) {
    if depth > 6 || out.len() >= 8 {
        return;
    }
    if let Some(obj) = value.as_object() {
        if let (Some(url), Some(content)) =
            (obj.get("url").and_then(|v| v.as_str()), content_of(obj))
        {
            if url.starts_with("http") {
                out.push(Hit {
                    title: obj
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or(url)
                        .to_string(),
                    url: url.to_string(),
                    content,
                });
            }
        }
        for (key, child) in obj {
            if matches!(
                key.as_str(),
                "results" | "web" | "news" | "data" | "hits" | "organic"
            ) || child.is_array()
                || child.is_object()
            {
                walk_hits(child, out, depth + 1);
            }
        }
    } else if let Some(arr) = value.as_array() {
        for child in arr {
            walk_hits(child, out, depth + 1);
        }
    }
}

fn content_of(obj: &serde_json::Map<String, Value>) -> Option<String> {
    for key in ["content", "description", "snippet", "raw_content", "text"] {
        if let Some(text) = obj.get(key).and_then(|v| v.as_str()) {
            if !text.trim().is_empty() {
                return Some(text.trim().to_string());
            }
        }
    }
    if let Some(highlights) = obj.get("highlights").and_then(|v| v.as_array()) {
        let text = highlights
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if !text.is_empty() {
            return Some(text);
        }
    }
    if let Some(snippets) = obj.get("snippets").and_then(|v| v.as_array()) {
        let text = snippets
            .iter()
            .filter_map(|v| {
                v.as_str()
                    .or_else(|| v.get("text").and_then(|t| t.as_str()))
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !text.is_empty() {
            return Some(text);
        }
    }
    None
}

fn text_at(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(text) = value.get(*key).and_then(|v| v.as_str()) {
            if !text.trim().is_empty() {
                return Some(text.trim().to_string());
            }
        }
    }
    None
}

pub fn truncate(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if out.chars().count() >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

fn decode_entities(input: &str) -> String {
    let mut text = input.to_string();
    for (from, to) in [
        ("&nbsp;", " "),
        ("&amp;", "&"),
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&quot;", "\""),
        ("&#39;", "'"),
    ] {
        text = text.replace(from, to);
    }
    text
}

pub fn html_to_text(input: &str) -> String {
    let mut cleaned = String::new();
    let mut i = 0;
    let bytes = input.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'<' {
            let rest = &input[i..];
            let lower = rest.to_ascii_lowercase();
            if lower.starts_with("<script") || lower.starts_with("<style") {
                let end = if lower.starts_with("<script") {
                    "</script>"
                } else {
                    "</style>"
                };
                if let Some(rel) = lower.find(end) {
                    i += rel + end.len();
                    continue;
                }
            }
            if let Some(rel) = rest.find('>') {
                cleaned.push(' ');
                i += rel + 1;
                continue;
            }
        }
        let ch = input[i..].chars().next().unwrap_or('\u{FFFD}');
        cleaned.push(ch);
        i += ch.len_utf8();
    }
    let decoded = decode_entities(&cleaned);
    let mut collapsed = String::new();
    let mut space = false;
    for ch in decoded.chars() {
        if ch.is_whitespace() {
            if !space && !collapsed.is_empty() {
                collapsed.push(' ');
            }
            space = true;
        } else {
            collapsed.push(ch);
            space = false;
        }
    }
    collapsed.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_tags_and_scripts() {
        let text =
            html_to_text("<style>x{}</style><p>Hello&nbsp;<b>web</b></p><script>nope()</script>");
        assert!(text.contains("Hello"));
        assert!(text.contains("web"));
        assert!(!text.contains("nope"));
        assert!(!text.contains("style"));
    }
}
