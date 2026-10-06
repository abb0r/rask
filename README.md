# Rask

A quiet native chat for [Ollama](https://ollama.com). You ask, a local model answers. If that model can use tools, it decides when to search the web and which page to open.

Rask is one window and a tray icon. It does not install Ollama, host accounts, or add a second toolbox on top of the chat.

Version 0.1.0.

## What it does

- Connects to a local Ollama server and lists the models that are already installed.
- Streams the reply. Stop cuts the generation short.
- Remembers chats, the default model, and a profile for each model: system prompt, temperature, context length, and whether thinking is on.
- Asks Ollama to unload the model after an idle time you set (`10m`, `30m`, or `0` to drop it when the reply finishes). The tray can unload it immediately.
- Gives tool-capable models two tools, `web_search` and `web_fetch`. The search provider is one of Ollama, Tavily, Brave, or You.com. Only the query (or the URL being read) is sent to that provider. The model stays on your machine.
- Can start at login, minimized to the tray. Closing the window hides it. Quit is in the tray menu.
- Refuses a second instance.

Plain text for now. Markdown rendering is not in 0.1.

## Requirements

- [Ollama](https://ollama.com/download), running locally. The default address is `http://127.0.0.1:11434`.
- A search API key if you want current answers. Without one, chat still works.
  - [Ollama keys](https://ollama.com/settings/keys)
  - [Tavily](https://app.tavily.com)
  - [Brave Search API](https://api.search.brave.com/)
  - [You.com search](https://you.com/docs/guides/search)
- To build: Rust 1.92 or newer.
- Linux desktops need the usual libraries: fontconfig, libxkbcommon, and Wayland or X11. A normal desktop install already has them.

Windows, Linux, and Apple Silicon macOS are the targets. There is no Intel-mac build in the release workflow.

## Build

```bash
cargo run --release
```

The binary is `target/release/rask`. Settings and chats are stored in the app data directory:

| System | Path |
| --- | --- |
| Linux | `~/.local/share/rask/store.json` |
| macOS | `~/Library/Application Support/rask/store.json` |
| Windows | `%APPDATA%\rask\store.json` |

The search key is in that file, in the clear, on your machine.

Startup uses the current path of the binary. If you move `rask` later, open Settings and press Apply again.

Tagged releases publish binaries from GitHub Actions:

- `rask-<tag>-linux-x86_64`
- `rask-<tag>-windows-x86_64.exe`
- `rask-<tag>-macos-aarch64`

## License

The application source is [MIT](LICENSE).

The UI uses [Slint](https://slint.dev), which is available under the [Slint Royalty-free License 2.0](https://github.com/slint-ui/slint/blob/master/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md) or GPL-3.0-only. Rask is not a UI toolkit and does not modify Slint.
