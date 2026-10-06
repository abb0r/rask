use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

pub const PROVIDERS: [&str; 4] = ["ollama", "tavily", "brave", "you"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProfile {
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default = "default_temperature")]
    pub temperature: f64,
    #[serde(default = "default_num_ctx")]
    pub num_ctx: u32,
    #[serde(default)]
    pub think: bool,
}

fn default_temperature() -> f64 {
    0.7
}

fn default_num_ctx() -> u32 {
    8192
}

impl Default for ModelProfile {
    fn default() -> Self {
        Self {
            system_prompt: String::new(),
            temperature: default_temperature(),
            num_ctx: default_num_ctx(),
            think: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub title: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub sources: Vec<Source>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chat {
    pub id: String,
    pub title: String,
    pub model: String,
    #[serde(default)]
    pub messages: Vec<Message>,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_url")]
    pub ollama_url: String,
    #[serde(default)]
    pub default_model: String,
    #[serde(default = "default_idle")]
    pub idle_unload: String,
    #[serde(default)]
    pub autostart: bool,
    #[serde(default = "default_true")]
    pub start_minimized: bool,
    #[serde(default = "default_provider")]
    pub search_provider: String,
    #[serde(default)]
    pub search_api_key: String,
    #[serde(default)]
    pub profiles: BTreeMap<String, ModelProfile>,
}

fn default_url() -> String {
    "http://127.0.0.1:11434".into()
}

fn default_idle() -> String {
    "10m".into()
}

fn default_provider() -> String {
    "ollama".into()
}

fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ollama_url: default_url(),
            default_model: String::new(),
            idle_unload: default_idle(),
            autostart: false,
            start_minimized: true,
            search_provider: default_provider(),
            search_api_key: String::new(),
            profiles: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Store {
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub chats: Vec<Chat>,
    #[serde(default)]
    pub active_chat: Option<String>,
}

impl Store {
    pub fn data_dir() -> PathBuf {
        if let Ok(dir) = std::env::var("RASK_DATA_DIR") {
            if !dir.is_empty() {
                return PathBuf::from(dir);
            }
        }
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("rask")
    }

    fn path() -> PathBuf {
        Self::data_dir().join("store.json")
    }

    pub fn load() -> Self {
        let path = Self::path();
        let Ok(bytes) = fs::read(&path) else {
            return Self::default();
        };
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let dir = Self::data_dir();
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        fs::write(Self::path(), bytes).map_err(|e| e.to_string())
    }

    pub fn profile(&self, model: &str) -> ModelProfile {
        self.settings
            .profiles
            .get(model)
            .cloned()
            .unwrap_or_default()
    }

    pub fn set_profile(&mut self, model: &str, profile: ModelProfile) {
        if !model.is_empty() && model != "No models" {
            self.settings.profiles.insert(model.to_string(), profile);
        }
    }

    pub fn active(&self) -> Option<&Chat> {
        let id = self.active_chat.as_deref()?;
        self.chats.iter().find(|c| c.id == id)
    }

    pub fn active_mut(&mut self) -> Option<&mut Chat> {
        let id = self.active_chat.clone()?;
        self.chats.iter_mut().find(|c| c.id == id)
    }

    pub fn new_chat(&mut self) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let now = now_ms();
        self.chats.insert(
            0,
            Chat {
                id: id.clone(),
                title: "New chat".into(),
                model: self.settings.default_model.clone(),
                messages: Vec::new(),
                updated_at: now,
            },
        );
        self.active_chat = Some(id.clone());
        id
    }

    pub fn ensure_chat(&mut self) -> String {
        if self.active_chat.is_none() || self.chats.is_empty() {
            return self.new_chat();
        }
        self.active_chat.clone().unwrap_or_default()
    }
}

pub fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn provider_index(name: &str) -> i32 {
    PROVIDERS.iter().position(|p| *p == name).unwrap_or(0) as i32
}

pub fn provider_name(index: i32) -> String {
    PROVIDERS
        .get(index.max(0) as usize)
        .copied()
        .unwrap_or("ollama")
        .to_string()
}

pub fn lock() -> std::sync::MutexGuard<'static, Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE
        .get_or_init(|| Mutex::new(Store::load()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_profile() {
        let store = Store::default();
        assert_eq!(store.settings.idle_unload, "10m");
        assert!(!store.profile("missing").think);
        assert_eq!(store.profile("missing").num_ctx, 8192);
    }

    #[test]
    fn settings_roundtrip_keeps_key() {
        let mut store = Store::default();
        store.settings.search_api_key = "tvly-test".into();
        store.settings.search_provider = "tavily".into();
        let bytes = serde_json::to_vec(&store).unwrap();
        let loaded: Store = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(loaded.settings.search_api_key, "tvly-test");
        assert_eq!(provider_index(&loaded.settings.search_provider), 1);
    }
}
