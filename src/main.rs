#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod chat;
mod ollama;
mod search;
mod store;
mod update;

use std::fs::OpenOptions;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use slint::ComponentHandle;
use slint::Model;

use store::{Message, ModelProfile, Source, Store};

slint::include_modules!();

struct StreamSlot {
    model: String,
    view: chat::TurnView,
    note: String,
    error: Option<String>,
    done: bool,
    applied: bool,
    painted: String,
    painted_status: String,
}

struct AppState {
    stream: Mutex<Option<StreamSlot>>,
    update: Mutex<Option<update::Offer>>,
    update_seen: AtomicBool,
    download: Mutex<Option<Result<PathBuf, String>>>,
}

fn main() {
    let _instance = match instance_lock() {
        Ok(file) => file,
        Err(_) => return,
    };

    let minimized = std::env::args().any(|arg| arg == "--minimized");
    let ui = AppWindow::new().expect("window");
    ui.set_app_version(update::current_version().into());
    let tray = RaskTray::new().expect("tray");
    tray.set_tray_tooltip("Rask".into());

    let cancel = Arc::new(AtomicBool::new(false));
    let app = Arc::new(AppState {
        stream: Mutex::new(None),
        update: Mutex::new(None),
        update_seen: AtomicBool::new(false),
        download: Mutex::new(None),
    });
    load_settings_into(&ui);
    refresh_lists(&ui);
    wire(&ui, &tray, cancel, app.clone());
    start_poll(ui.as_weak(), app.clone());
    start_update_check(app);

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

fn start_poll(weak: slint::Weak<AppWindow>, app: Arc<AppState>) {
    let timer = Box::leak(Box::new(slint::Timer::default()));
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(50),
        move || {
            let Some(ui) = weak.upgrade() else { return };
            poll(&ui, &app);
        },
    );
}

fn start_update_check(app: Arc<AppState>) {
    std::thread::spawn(move || {
        if let Ok(Some(offer)) = update::check() {
            if let Ok(mut slot) = app.update.lock() {
                *slot = Some(offer);
            }
        }
    });
}

fn poll(ui: &AppWindow, app: &AppState) {
    if !app.update_seen.load(Ordering::Relaxed) {
        if let Ok(slot) = app.update.lock() {
            if let Some(offer) = slot.as_ref() {
                ui.set_update_version(offer.version.clone().into());
                app.update_seen.store(true, Ordering::Relaxed);
            }
        }
    }

    if let Ok(mut slot) = app.download.lock() {
        if let Some(result) = slot.take() {
            ui.set_update_busy(false);
            match result {
                Ok(path) => {
                    if std::process::Command::new(&path).spawn().is_ok() {
                        let _ = slint::quit_event_loop();
                    } else {
                        ui.set_status("Could not start the installer.".into());
                        let _ = open::that("https://github.com/abb0r/rask/releases/latest");
                    }
                }
                Err(err) => {
                    ui.set_status(err.into());
                    let _ = open::that("https://github.com/abb0r/rask/releases/latest");
                }
            }
        }
    }

    let snapshot = {
        let Ok(mut guard) = app.stream.lock() else {
            return;
        };
        let Some(state) = guard.as_mut() else {
            return;
        };
        if state.applied {
            return;
        }
        if state.done {
            state.applied = true;
            Some((
                true,
                state.model.clone(),
                state.view.clone(),
                state.note.clone(),
                state.error.clone(),
            ))
        } else if state.view.text == state.painted && state.view.status == state.painted_status {
            None
        } else {
            state.painted = state.view.text.clone();
            state.painted_status = state.view.status.clone();
            Some((
                false,
                state.model.clone(),
                state.view.clone(),
                state.note.clone(),
                None,
            ))
        }
    };
    let Some((done, model, view, note, error)) = snapshot else {
        return;
    };
    if done {
        finish_turn(ui, &model, &view, error, &note);
    } else {
        paint_stream(ui, &model, &view, &note);
    }
}

fn wire(ui: &AppWindow, tray: &RaskTray, cancel: Arc<AtomicBool>, app: Arc<AppState>) {
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
        if ui.get_suppress_persist() {
            return;
        }
        let name = model_at(&ui, index);
        {
            let mut store = store::lock();
            let previous = store
                .active()
                .map(|chat| chat.model.clone())
                .unwrap_or_default();
            if !previous.is_empty() && previous != name {
                let profile = profile_from_ui(&ui, &store.profile(&previous));
                store.set_profile(&previous, profile);
            }
            if let Some(chat) = store.active_mut() {
                chat.model = name.clone();
                chat.updated_at = store::now_ms();
            }
            let profile = store.profile(&name);
            let _ = store.save();
            ui.set_suppress_persist(true);
            apply_profile(&ui, &profile);
            ui.set_suppress_persist(false);
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
    ui.on_persist(move || {
        let Some(ui) = weak.upgrade() else { return };
        if let Err(err) = persist_from_ui(&ui, false) {
            ui.set_status(err.into());
        } else if !ui.get_api_key().trim().is_empty() && ui.get_status().contains("No search key") {
            ui.set_status(String::new().into());
        }
    });

    let weak = ui.as_weak();
    ui.on_persist_autostart(move || {
        let Some(ui) = weak.upgrade() else { return };
        if let Err(err) = persist_from_ui(&ui, true) {
            ui.set_status(err.into());
        }
    });

    let weak = ui.as_weak();
    let cancel_send = cancel.clone();
    let app_send = app.clone();
    ui.on_send(move |text| {
        let Some(ui) = weak.upgrade() else { return };
        let text = text.trim().to_string();
        if text.is_empty() || ui.get_busy() {
            return;
        }
        cancel_send.store(false, Ordering::Relaxed);
        match begin_turn(&ui, text, cancel_send.clone(), app_send.clone()) {
            Ok(()) => ui.set_draft(String::new().into()),
            Err(err) => {
                ui.set_status(err.into());
                ui.set_busy(false);
            }
        }
    });

    let cancel_stop = cancel.clone();
    ui.on_stop(move || cancel_stop.store(true, Ordering::Relaxed));

    let weak = ui.as_weak();
    let app_update = app.clone();
    ui.on_install_update(move || {
        let Some(ui) = weak.upgrade() else { return };
        let offer = app_update.update.lock().ok().and_then(|slot| slot.clone());
        let Some(offer) = offer else { return };
        ui.set_update_busy(true);
        ui.set_status("Downloading update…".into());
        let app_update = app_update.clone();
        std::thread::spawn(move || {
            let result = update::download_installer(&offer.download_url, &offer.version);
            if let Ok(mut slot) = app_update.download.lock() {
                *slot = Some(result);
            }
        });
    });

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

    let weak = ui.as_weak();
    tray.on_quit(move || {
        if let Some(ui) = weak.upgrade() {
            let _ = persist_from_ui(&ui, false);
        }
        let _ = slint::quit_event_loop();
    });
}

fn begin_turn(
    ui: &AppWindow,
    text: String,
    cancel: Arc<AtomicBool>,
    app: Arc<AppState>,
) -> Result<(), String> {
    let _ = persist_from_ui(ui, false);
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
        let has_key = !store.settings.search_api_key.trim().is_empty();
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
            tools: has_key,
        }
    };

    let note = if request.tools {
        String::new()
    } else {
        "No search key saved — answering without the web.".into()
    };
    if let Ok(mut slot) = app.stream.lock() {
        *slot = Some(StreamSlot {
            model: request.model.clone(),
            view: chat::TurnView {
                text: String::new(),
                sources: Vec::new(),
                status: format!("Waiting for {}…", request.model),
            },
            note: note.clone(),
            error: None,
            done: false,
            applied: false,
            painted: String::new(),
            painted_status: String::new(),
        });
    }

    ui.set_busy(true);
    ui.set_status(format!("Waiting for {}…", request.model).into());
    refresh_lists(ui);
    ui.invoke_scroll_to_bottom();

    std::thread::spawn(move || {
        let had_key = request.tools;
        let tools = if had_key {
            ollama::supports_tools(&request.base_url, &request.model)
        } else {
            false
        };
        let note = if had_key && !tools {
            "This model cannot search on its own.".into()
        } else if !had_key {
            "No search key saved — answering without the web.".into()
        } else {
            String::new()
        };
        if let Ok(mut slot) = app.stream.lock() {
            if let Some(state) = slot.as_mut() {
                state.note = note;
            }
        }
        let mut request = request;
        request.tools = tools;
        let app_stream = app.clone();
        let result = chat::run(request, &cancel, move |view| {
            if let Ok(mut slot) = app_stream.stream.lock() {
                if let Some(state) = slot.as_mut() {
                    state.view = view;
                }
            }
        });
        if let Ok(mut slot) = app.stream.lock() {
            if let Some(state) = slot.as_mut() {
                let stopped = cancel.load(Ordering::Relaxed) && state.view.text.is_empty();
                state.error = result.err().or_else(|| stopped.then(|| "Stopped.".into()));
                state.done = true;
            }
        }
    });
    Ok(())
}

fn finish_turn(
    ui: &AppWindow,
    model: &str,
    view: &chat::TurnView,
    error: Option<String>,
    note: &str,
) {
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
    let status = error.unwrap_or_else(|| note.to_string());
    ui.set_status(status.into());
    refresh_lists(ui);
    ui.invoke_scroll_to_bottom();
    ui.invoke_scroll_to_bottom();
}

fn paint_stream(ui: &AppWindow, model: &str, view: &chat::TurnView, note: &str) {
    let model_rc = ui.get_messages();
    let mut bubbles = Vec::with_capacity(model_rc.row_count());
    for index in 0..model_rc.row_count() {
        if let Some(row) = model_rc.row_data(index) {
            bubbles.push(row);
        }
    }
    let body = if view.text.is_empty() {
        "…"
    } else {
        view.text.as_str()
    };
    let sources = source_line(&view.sources);
    if let Some(last) = bubbles.last_mut() {
        if last.role.as_str() == model {
            last.body = body.into();
            last.sources = sources.into();
        } else {
            bubbles.push(Bubble {
                role: model.into(),
                body: body.into(),
                sources: sources.into(),
            });
        }
    } else {
        bubbles.push(Bubble {
            role: model.into(),
            body: body.into(),
            sources: sources.into(),
        });
    }
    ui.set_messages(slint::ModelRc::new(slint::VecModel::from(bubbles)));
    let status = if !view.status.is_empty() {
        view.status.clone()
    } else if view.text.is_empty() {
        format!("Waiting for {model}…")
    } else {
        note.to_string()
    };
    ui.set_status(status.into());
    ui.invoke_scroll_to_bottom();
    ui.invoke_scroll_to_bottom();
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
    ui.invoke_scroll_to_bottom();
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
                } else if ui.get_status().contains("Cannot reach Ollama")
                    || ui.get_status().contains("no models")
                {
                    ui.set_status(String::new().into());
                }
                let default_name = store::lock().settings.default_model.clone();
                let active_model = store::lock()
                    .active()
                    .map(|chat| chat.model.clone())
                    .unwrap_or_default();
                ui.set_suppress_persist(true);
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
                ui.set_suppress_persist(false);
            }
            Err(err) => ui.set_status(err.into()),
        }
    });
}

fn profile_from_ui(ui: &AppWindow, previous: &ModelProfile) -> ModelProfile {
    let mut profile = previous.clone();
    profile.system_prompt = ui.get_system_prompt().to_string();
    profile.think = ui.get_think();
    if let Ok(temperature) = ui.get_temperature().trim().parse::<f64>() {
        profile.temperature = temperature.clamp(0.0, 2.0);
    }
    if let Ok(num_ctx) = ui.get_num_ctx().trim().parse::<u32>() {
        if num_ctx >= 512 {
            profile.num_ctx = num_ctx;
        }
    }
    profile
}

fn persist_from_ui(ui: &AppWindow, autostart_changed: bool) -> Result<(), String> {
    if ui.get_suppress_persist() {
        return Ok(());
    }
    let mut store = store::lock();
    let url = ui.get_ollama_url().trim().to_string();
    if !url.is_empty() {
        store.settings.ollama_url = ollama::normalize_base(&url);
    }
    let idle = ui.get_idle_unload().trim().to_string();
    if !idle.is_empty() {
        store.settings.idle_unload = idle;
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
        .filter(|name| !name.is_empty() && name != "No models")
        .or_else(|| {
            store
                .active()
                .map(|chat| chat.model.clone())
                .filter(|name| !name.is_empty())
        });
    if let Some(selected) = selected {
        let profile = profile_from_ui(ui, &store.profile(&selected));
        store.set_profile(&selected, profile);
    }
    if autostart_changed {
        autostart::set_enabled(store.settings.autostart, store.settings.start_minimized)?;
    }
    store.save()?;
    Ok(())
}

fn load_settings_into(ui: &AppWindow) {
    let mut store = store::lock();
    store.ensure_chat();
    let _ = store.save();
    ui.set_suppress_persist(true);
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
    ui.set_suppress_persist(false);
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
