use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

const LATEST: &str = "https://api.github.com/repos/abb0r/rask/releases/latest";

#[derive(Clone)]
pub struct Offer {
    pub version: String,
    pub download_url: String,
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

fn parse_version(raw: &str) -> Option<(u64, u64, u64)> {
    let raw = raw.trim().trim_start_matches('v');
    let mut parts = raw.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.split(['-', '+']).next()?.parse().ok()?;
    Some((major, minor, patch))
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent(concat!("rask/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(8))
        .build()
        .map_err(|e| e.to_string())
}

pub fn check() -> Result<Option<Offer>, String> {
    let response = client()?.get(LATEST).send().map_err(|e| e.to_string())?;
    if response.status().as_u16() == 404 {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(format!("update check returned {}", response.status()));
    }
    let value: Value = response.json().map_err(|e| e.to_string())?;
    let version = value
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim_start_matches('v')
        .to_string();
    if !is_newer(&version, current_version()) {
        return Ok(None);
    }
    let Some(url) = setup_url(value.get("assets")) else {
        return Ok(None);
    };
    Ok(Some(Offer {
        version,
        download_url: url,
    }))
}

fn setup_url(assets: Option<&Value>) -> Option<String> {
    let assets = assets?.as_array()?;
    assets.iter().find_map(|asset| {
        let name = asset.get("name")?.as_str()?;
        if name.starts_with("Rask-Setup-") && name.ends_with(".exe") {
            asset
                .get("browser_download_url")
                .and_then(|v| v.as_str())
                .map(|url| url.to_string())
        } else {
            None
        }
    })
}

pub fn download_installer(url: &str, version: &str) -> Result<PathBuf, String> {
    let path = std::env::temp_dir().join(format!("Rask-Setup-{version}.exe"));
    let mut response = client()?.get(url).send().map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("download failed: {}", response.status()));
    }
    let mut file = File::create(&path).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    response.copy_to(&mut buf).map_err(|e| e.to_string())?;
    if buf.len() < 1024 {
        return Err("downloaded file is too small to be the installer".into());
    }
    file.write_all(&buf).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.2.1", "0.2.0"));
        assert!(is_newer("v0.3.0", "0.2.9"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        assert!(!is_newer("not-a-version", "0.2.0"));
    }

    #[test]
    fn picks_setup_asset() {
        let assets = serde_json::json!([
            { "name": "rask.exe", "browser_download_url": "https://example.test/raw" },
            { "name": "Rask-Setup-0.2.1.exe", "browser_download_url": "https://example.test/setup" }
        ]);
        assert_eq!(
            setup_url(Some(&assets)).as_deref(),
            Some("https://example.test/setup")
        );
    }
}
