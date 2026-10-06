#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod chat;
mod ollama;
mod search;
mod store;

use std::fs::OpenOptions;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use slint::ComponentHandle;
use slint::Model;

use store::{Message, ModelProfile, Source, Store};

slint::include_modules!();

fn main() {
    let _instance = match instance_lock() {
        Ok(file) => file,
        Err(_) => return,
    };

    let minimized = std::env::args().any(|arg| arg == "--minimized");
    let ui = AppWindow::new().expect("window");
    let tray = RaskTray::new().expect("tray");
    tray.set_tray_tooltip("Rask".into());

    let cancel = Arc::new(AtomicBool::new(false));
    load_settings_into(&ui);
    refresh_lists(&ui);
    wire(&ui, &tray, cancel);

    ui.window()
        .on_close_requested(|| slint::CloseRequestResponse::HideWindow);

    if !minimized {
        ui.show().ok();
    }
    let ui_weak = ui.as_weak();
    std::thread::spawn(move || refresh_models(ui_weak));
    slint::run_event_loop().ok();
}

fn instance_lock() -> std::io::Result<std::fs::File> {
    let dir = Store::data_dir();
    std::fs::create_dir_all(&dir)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(dir.join("instance.lock"))?;
    file.try_lock()?;
    Ok(file)
}

fn wire(ui: &AppWindow, tray: &RaskTray, cancel: Arc<AtomicBool>) {
    let weak = ui.as_weak();
    ui.on_new_chat(move || {
        let Some(ui) = weak.upgrade() else { return };
        {
            let mut store = store::lock();
            store.new_chat();
            let _ = store.save();
        }
        refresh_lists(&ui);
    });

    let weak = ui.as_weak();
    ui.on_pick_chat(move |index| {
        let Some(ui) = weak.upgrade() else { return };
        {
            let mut store = store::lock();
            if let Some(chat) = store.chats.get(index as usize) {
                store.active_chat = Some(chat.id.clone());
            }
            let _ = store.save();
        }
        refresh_lists(&ui);
    });

    let weak = ui.as_weak();
    ui.on_pick_model(move |index| {
        let Some(ui) = weak.upgrade() else { return };
        let name = model_at(&ui, index);
        {
            let mut store = store::lock();
            if let Some(chat) = store.active_mut() {
                chat.model = name.clone();
                chat.updated_at = store::now_ms();
            }
            let profile = store.profile(&name);
            let _ = store.save();
            apply_profile(&ui, &profile);
        }
        ui.set_model_index(index);
    });

    let weak = ui.as_weak();
    ui.on_refresh_models(move || {
        let weak = weak.clone();
        std::thread::spawn(move || refresh_models(weak));
    });

    let weak = ui.as_weak();
    ui.on_open_key_page(move || {
        let Some(ui) = weak.upgrade() else { return };
        let url = match ui.get_provider_index() {
            1 => "https://app.tavily.com",
            2 => "https://api.search.brave.com/",
            3 => "https://you.com/docs/guides/search",
            _ => "https://ollama.com/settings/keys",
        };
        let _ = open::that(url);
    });

    let weak = ui.as_weak();
    ui.on_apply_settings(move || {
        let Some(ui) = weak.upgrade() else { return };
        let err = apply_from_ui(&ui);
        ui.set_status(err.unwrap_or_else(|e| e).into());
        refresh_lists(&ui);
    });

    let weak = ui.as_weak();
    let cancel_send = cancel.clone();
    ui.on_send(move |text| {
        let Some(ui) = weak.upgrade() else { return };
        let text = text.trim().to_string();
        if text.is_empty() || ui.get_busy() {
            return;
        }
        cancel_send.store(false, Ordering::Relaxed);
        if let Err(err) = begin_turn(&ui, text, cancel_send.clone()) {
            ui.set_status(err.into());
            ui.set_busy(false);
        }
    });

    let cancel_stop = cancel.clone();
    ui.on_stop(move || cancel_stop.store(true, Ordering::Relaxed));

    let weak = ui.as_weak();
    tray.on_show_window(move || {
        if let Some(ui) = weak.upgrade() {
            ui.show().ok();
        }
    });

    let weak = ui.as_weak();
    tray.on_unload(move || {
        let Some(ui) = weak.upgrade() else { return };
        let base = {
            let store = store::lock();
            store.settings.ollama_url.clone()
        };
        ui.set_status("Unloading…".into());
        let weak = ui.as_weak();
        std::thread::spawn(move || {
            let result = ollama::unload_all(&base);
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = weak.upgrade() else { return };
                ui.set_status(
                    match result {
                        Ok(()) => "Model unloaded.".to_string(),
                        Err(err) => err,
                    }
                    .into(),
                );
            });
        });
    });

    tray.on_quit(|| {
        let _ = slint::quit_event_loop();
    });
}

fn begin_turn(ui: &AppWindow, text: String, cancel: Arc<AtomicBool>) -> Result<(), String> {
    let request = {
        let mut store = store::lock();
        store.ensure_chat();
        let model = {
            let chat = store.active().ok_or("no chat")?;
            if chat.model.is_empty() {
                store.settings.default_model.clone()
            } else {
                chat.model.clone()
            }
        };
        if model.is_empty() || model == "No models" {
            return Err("Select a model. Is Ollama running?".into());
        }
        if let Some(chat) = store.active_mut() {
            chat.model = model.clone();
            if chat.messages.is_empty() {
                chat.title = title_from(&text);
            }
            chat.messages.push(Message {
                role: "user".into(),
                content: text,
                sources: Vec::new(),
            });
            chat.updated_at = store::now_ms();
        }
        let history = store
            .active()
            .map(|chat| {
                chat.messages
                    .iter()
                    .rev()
                    .take(30)
                    .map(|m| (m.role.clone(), m.content.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
            .into_iter()
            .rev()
            .collect();
        let profile = store.profile(&model);
        let tools = !store.settings.search_api_key.trim().is_empty();
        let base_url = store.settings.ollama_url.clone();
        let idle_unload = store.settings.idle_unload.clone();
        let provider = store.settings.search_provider.clone();
        let api_key = store.settings.search_api_key.clone();
        store.save()?;
        chat::TurnRequest {
            base_url,
            model,
            profile,
            idle_unload,
            provider,
            api_key,
            history,
            tools,
        }
    };

    ui.set_busy(true);
    ui.set_status(
        if request.tools {
            String::new()
        } else {
            "No search API key — answering without the web.".into()
        }
        .into(),
    );
    refresh_lists(ui);

    let weak = ui.as_weak();
    let model_name = request.model.clone();
    std::thread::spawn(move || {
        let had_key = request.tools;
        let tools = if had_key {
            ollama::supports_tools(&request.base_url, &request.model)
        } else {
            false
        };
        if had_key && !tools {
            let weak_note = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak_note.upgrade() {
                    ui.set_status("This model cannot search on its own.".into());
                }
            });
        }
        let mut request = request;
        request.tools = tools;
        let latest = Arc::new(Mutex::new(chat::TurnView {
            text: String::new(),
            sources: Vec::new(),
            status: String::new(),
            done: false,
        }));
        let last_paint = Arc::new(Mutex::new(std::time::Instant::now()));
        let view_slot = latest.clone();
        let paint_slot = last_paint.clone();
        let weak_stream = weak.clone();
        let model_for_stream = model_name.clone();
        let result = chat::run(request, &cancel, move |view| {
            let done = view.done;
            let status_changed = view_slot
                .lock()
                .map(|g| g.status != view.status)
                .unwrap_or(true);
            if let Ok(mut slot) = view_slot.lock() {
                *slot = view;
            }
            let due = paint_slot
                .lock()
                .map(|t| {
                    t.elapsed() >= std::time::Duration::from_millis(40) || done || status_changed
                })
                .unwrap_or(true);
            if !due {
                return;
            }
            if let Ok(mut t) = paint_slot.lock() {
                *t = std::time::Instant::now();
            }
            let weak_stream = weak_stream.clone();
            let view_slot = view_slot.clone();
            let model_for_stream = model_for_stream.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = weak_stream.upgrade() else {
                    return;
                };
                if let Ok(view) = view_slot.lock() {
                    paint_stream(&ui, &model_for_stream, &view);
                }
            });
        });

        let weak_done = weak.clone();
        let view = latest.lock().ok().map(|v| v.clone());
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak_done.upgrade() else {
                return;
            };
            if let Some(view) = view {
                finish_turn(&ui, &model_name, &view, result.err());
            } else if let Err(err) = result {
                ui.set_status(err.into());
                ui.set_busy(false);
            }
        });
    });
    Ok(())
}

fn finish_turn(ui: &AppWindow, model: &str, view: &chat::TurnView, error: Option<String>) {
    {
        let mut store = store::lock();
        if let Some(chat) = store.active_mut() {
            if chat.model.is_empty() {
                chat.model = model.to_string();
            }
            let sources = view
                .sources
                .iter()
                .map(|hit| Source {
                    title: hit.title.clone(),
                    url: hit.url.clone(),
                })
                .collect();
            let content = if view.text.is_empty() {
                error.clone().unwrap_or_else(|| "No reply.".into())
            } else {
                view.text.clone()
            };
            chat.messages.push(Message {
                role: "assistant".into(),
                content,
                sources,
            });
            chat.updated_at = store::now_ms();
        }
        let _ = store.save();
    }
    ui.set_busy(false);
    ui.set_status(error.unwrap_or_default().into());
    refresh_lists(ui);
}

fn paint_stream(ui: &AppWindow, model: &str, view: &chat::TurnView) {
    let model_rc = ui.get_messages();
    let mut bubbles = Vec::with_capacity(model_rc.row_count());
    for index in 0..model_rc.row_count() {
        if let Some(row) = model_rc.row_data(index) {
            bubbles.push(row);
        }
    }
    let sources = source_line(&view.sources);
    if let Some(last) = bubbles.last_mut() {
        if last.role.as_str() == model {
            last.body = view.text.as_str().into();
            last.sources = sources.into();
        } else if !view.text.is_empty() || !view.status.is_empty() {
            bubbles.push(Bubble {
                role: model.into(),
                body: view.text.as_str().into(),
                sources: sources.into(),
            });
        }
    } else if !view.text.is_empty() {
        bubbles.push(Bubble {
            role: model.into(),
            body: view.text.as_str().into(),
            sources: sources.into(),
        });
    }
    ui.set_messages(slint::ModelRc::new(slint::VecModel::from(bubbles)));
    ui.set_scroll_y(-100_000.0);
    if !view.status.is_empty() {
        ui.set_status(view.status.clone().into());
    }
}

fn source_line(hits: &[search::Hit]) -> String {
    let mut seen = std::collections::BTreeSet::new();
    hits.iter()
        .filter(|hit| seen.insert(hit.url.clone()) && !hit.url.is_empty())
        .map(|hit| hit.url.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

fn source_line_saved(sources: &[Source]) -> String {
    sources
        .iter()
        .map(|source| source.url.clone())
        .filter(|url| !url.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn display_role(model: &str, role: &str) -> String {
    if role == "user" {
        "You".into()
    } else if model.is_empty() {
        "Rask".into()
    } else {
        model.to_string()
    }
}

fn refresh_lists(ui: &AppWindow) {
    let store = store::lock();
    let rows: Vec<ChatRow> = store
        .chats
        .iter()
        .map(|chat| ChatRow {
            title: chat.title.clone().into(),
        })
        .collect();
    ui.set_chats(slint::ModelRc::new(slint::VecModel::from(rows)));
    let messages = store
        .active()
        .map(|chat| {
            chat.messages
                .iter()
                .map(|message| Bubble {
                    role: display_role(&chat.model, &message.role).into(),
                    body: message.content.clone().into(),
                    sources: source_line_saved(&message.sources).into(),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    ui.set_messages(slint::ModelRc::new(slint::VecModel::from(messages)));
    ui.set_scroll_y(-100_000.0);
    if let Some(chat) = store.active() {
        let names = model_names(ui);
        if let Some(index) = names.iter().position(|name| name == &chat.model) {
            ui.set_model_index(index as i32);
        }
    }
}

fn refresh_models(weak: slint::Weak<AppWindow>) {
    let base = store::lock().settings.ollama_url.clone();
    let result = ollama::list_models(&base);
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        match result {
            Ok(mut names) => {
                if names.is_empty() {
                    names.push("No models".into());
                    ui.set_status("Ollama is up, but no models are installed.".into());
                } else {
                    ui.set_status(String::new().into());
                }
                let default_name = store::lock().settings.default_model.clone();
                let active_model = store::lock()
                    .active()
                    .map(|chat| chat.model.clone())
                    .unwrap_or_default();
                ui.set_model_names(slint::ModelRc::new(slint::VecModel::from(
                    names
                        .iter()
                        .map(|n| slint::SharedString::from(n))
                        .collect::<Vec<_>>(),
                )));
                ui.set_model_index(index_of(&names, &active_model));
                ui.set_default_index(index_of(&names, &default_name));
                if active_model.is_empty() {
                    if let Some(name) = names.first() {
                        let mut store = store::lock();
                        if store.settings.default_model.is_empty() {
                            store.settings.default_model = name.clone();
                        }
                        let fallback = store.settings.default_model.clone();
                        if let Some(chat) = store.active_mut() {
                            if chat.model.is_empty() {
                                chat.model = fallback;
                            }
                        }
                        let _ = store.save();
                    }
                }
                let model = model_at(&ui, ui.get_model_index());
                let profile = store::lock().profile(&model);
                apply_profile(&ui, &profile);
            }
            Err(err) => ui.set_status(err.into()),
        }
    });
}

fn apply_from_ui(ui: &AppWindow) -> Result<String, String> {
    let mut store = store::lock();
    let previous_model = store
        .active()
        .map(|chat| chat.model.clone())
        .unwrap_or_default();
    store.settings.ollama_url = ollama::normalize_base(&ui.get_ollama_url());
    store.settings.idle_unload = ui.get_idle_unload().trim().to_string();
    if store.settings.idle_unload.is_empty() {
        store.settings.idle_unload = "10m".into();
    }
    store.settings.autostart = ui.get_autostart();
    store.settings.start_minimized = ui.get_start_minimized();
    store.settings.search_provider = store::provider_name(ui.get_provider_index());
    store.settings.search_api_key = ui.get_api_key().trim().to_string();
    let names = model_names(ui);
    if let Some(name) = names.get(ui.get_default_index() as usize) {
        if name != "No models" {
            store.settings.default_model = name.clone();
        }
    }
    let selected = names
        .get(ui.get_model_index() as usize)
        .cloned()
        .unwrap_or(previous_model);
    let temperature = ui
        .get_temperature()
        .parse::<f64>()
        .unwrap_or(0.7)
        .clamp(0.0, 2.0);
    let num_ctx = ui.get_num_ctx().parse::<u32>().unwrap_or(8192).max(512);
    store.set_profile(
        &selected,
        ModelProfile {
            system_prompt: ui.get_system_prompt().to_string(),
            temperature,
            num_ctx,
            think: ui.get_think(),
        },
    );
    autostart::set_enabled(store.settings.autostart, store.settings.start_minimized)?;
    store.save()?;
    Ok("Saved.".into())
}

fn load_settings_into(ui: &AppWindow) {
    let mut store = store::lock();
    store.ensure_chat();
    let _ = store.save();
    ui.set_ollama_url(store.settings.ollama_url.clone().into());
    ui.set_idle_unload(store.settings.idle_unload.clone().into());
    ui.set_autostart(store.settings.autostart);
    ui.set_start_minimized(store.settings.start_minimized);
    ui.set_provider_index(store::provider_index(&store.settings.search_provider));
    ui.set_api_key(store.settings.search_api_key.clone().into());
    let model = store
        .active()
        .map(|chat| chat.model.clone())
        .unwrap_or_default();
    let profile = store.profile(&model);
    apply_profile(ui, &profile);
}

fn apply_profile(ui: &AppWindow, profile: &ModelProfile) {
    ui.set_system_prompt(profile.system_prompt.clone().into());
    ui.set_temperature(format!("{}", profile.temperature).into());
    ui.set_num_ctx(profile.num_ctx.to_string().into());
    ui.set_think(profile.think);
}

fn model_names(ui: &AppWindow) -> Vec<String> {
    ui.get_model_names()
        .iter()
        .map(|name| name.to_string())
        .collect()
}

fn model_at(ui: &AppWindow, index: i32) -> String {
    model_names(ui)
        .get(index.max(0) as usize)
        .cloned()
        .unwrap_or_default()
}

fn index_of(names: &[String], want: &str) -> i32 {
    names.iter().position(|name| name == want).unwrap_or(0) as i32
}

fn title_from(text: &str) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut title = String::new();
    for ch in one_line.chars().take(48) {
        title.push(ch);
    }
    if one_line.chars().count() > 48 {
        title.push('…');
    }
    if title.is_empty() {
        "New chat".into()
    } else {
        title
    }
}
