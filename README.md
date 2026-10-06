# Rask

A quiet native chat for [Ollama](https://ollama.com). You ask, a local model answers. If that model can use tools, it decides when to search the web and which page to open.

Rask is one window and a tray icon. It does not install Ollama, host accounts, or add a second toolbox on top of the chat.

Version 0.2.0. The download is a Windows installer. Linux and macOS builds are not published.

## Install

Download [Rask-Setup](https://github.com/abb0r/rask/releases/latest) and run it. It installs for the current user, adds a Start menu shortcut, and registers an uninstaller. No administrator account is required.

On startup Rask checks GitHub for a newer installer. When one exists, an Install button downloads it and opens the setup.

## What it does

- Connects to a local Ollama server and lists the models that are already installed.
- Streams the reply into the chat. Stop cuts the generation short.
- Remembers chats and every setting: the Ollama address, idle time, startup, search provider, API key, default model, and a profile for each model (system prompt, temperature, context length, thinking). Settings are saved as you edit them.
- Asks Ollama to unload the model after an idle time you set (`10m`, `30m`, or `0` to drop it when the reply finishes). The tray can unload it immediately.
- Gives tool-capable models two tools, `web_search` and `web_fetch`. The search provider is one of Ollama, Tavily, Brave, or You.com. Only the query (or the URL being read) is sent to that provider. The model stays on your machine.
- Can start at login, minimized to the tray. Closing the window hides it. Quit is in the tray menu.
- Refuses a second instance.

Plain text for now. Markdown rendering is not in 0.2.

## Requirements

- Windows 10 or newer for the installer.
- [Ollama](https://ollama.com/download), running locally. The default address is `http://127.0.0.1:11434`.
- A search API key if you want current answers. Without one, chat still works. The key is stored in the clear in the settings file below.
  - [Ollama keys](https://ollama.com/settings/keys)
  - [Tavily](https://app.tavily.com)
  - [Brave Search API](https://api.search.brave.com/)
  - [You.com search](https://you.com/docs/guides/search)
- To build from source: Rust 1.92 or newer.

## Build

```bash
cargo run --release
```

The binary is `target/release/rask` (or `rask.exe` on Windows). Settings and chats are stored in the app data directory:

| System | Path |
| --- | --- |
| Windows | `%APPDATA%\rask\store.json` |
| Linux | `~/.local/share/rask/store.json` |
| macOS | `~/Library/Application Support/rask/store.json` |

Startup uses the current path of the binary. If you move `rask` later, toggle "Start when I log in" off and on again.

Enter sends a message. Shift+Enter inserts a new line.

## License

The application source is [MIT](LICENSE).

The UI uses [Slint](https://slint.dev), which is available under the [Slint Royalty-free License 2.0](https://github.com/slint-ui/slint/blob/master/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md) or GPL-3.0-only. Rask is not a UI toolkit and does not modify Slint.
