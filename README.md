# ClipboardShare

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

**English** | [中文](README.zh-CN.md)

Windows LAN clipboard sync with an optional tray UI. After a one-time pairing, two PCs sync text, HTML, RTF, images, and files/directories in both directions.

> **Platforms (v1):** Windows and macOS only (Linux not supported). Clipboard sync itself currently targets Windows 10 1809+ / Windows 11. Tray UI on Windows requires [Microsoft Edge WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) — if missing, the app opens the [Evergreen Bootstrapper download](https://go.microsoft.com/fwlink/p/?LinkId=2124703) and shows a prompt. macOS uses built-in WKWebView (no extra install). Runs in the logged-in user session (not as a Session 0 service).

## Features

- Real-time bidirectional sync (text / HTML / RTF / PNG images / files & directories)
- One-time pairing code + SPAKE2; persistent keys protected by Windows DPAPI
- Encrypted transport: ChaCha20-Poly1305
- Tray UI (Tauri 2): status panel, pair/start/stop, minimize to the notification area
- Portable layout: `config.json` + `data\`, copy-and-run deployment
- Optional logon autostart (scheduled task with delayed start)

## Build

Requires [Rust](https://rustup.rs/) (latest stable recommended). Tray UI also needs Node.js for `@tauri-apps/cli` in `ui/`.

```powershell
cargo build --release -p clipboard_share
cargo build --release -p clipboard_share_ui
```

Outputs:

- `target\release\clipboard_share.exe` — CLI
- `target\release\clipboard_share_ui.exe` — tray UI (embeds sync in-process)

Build A/B portable packages (CLI + UI):

```powershell
.\build-portable.ps1
```

Output (dedicated folder):

- `packages\ClipboardShare-A.zip` / `ClipboardShare-B.zip` — pair templates (CLI + tray UI)
- `packages\ClipboardShare-UI.zip` — tray-UI focused package (`start-ui.bat`)

## Tray UI

```powershell
.\clipboard_share_ui.exe
# or
.\start-ui.bat
```

- Closing the window **hides to the tray** (does not quit)
- Tray menu: Show panel / Start sync / Stop sync / Quit
- Left-click the tray icon to reopen the panel
- Only one UI instance is allowed; sync uses the same single-instance lock as the CLI daemon
- **Language:** English / 中文 toggle in the panel (default **English**; preference saved locally). Tray menu follows the selected language.
- **Windows WebView2:** if the runtime is missing, a dialog appears and the browser opens  
  [https://go.microsoft.com/fwlink/p/?LinkId=2124703](https://go.microsoft.com/fwlink/p/?LinkId=2124703)  
  (Evergreen Bootstrapper). Install, then relaunch. Docs: [WebView2](https://developer.microsoft.com/microsoft-edge/webview2/).

## Quick start

1. Enable **Clipboard history** in Windows Settings (Win+V).
2. On machine A:

   ```powershell
   .\clipboard_share.exe pair-listen
   ```

   Note the six-digit pairing code shown.

3. On machine B (replace the IP with A's LAN address):

   ```powershell
   .\clipboard_share.exe pair 192.168.1.10 123456
   ```

4. On both machines:

   ```powershell
   .\clipboard_share.exe start
   .\clipboard_share.exe status
   ```

Alternatively, edit the portable templates in `portable\A|B\config.json`: set the same `pairing_code`, use `auto_pair.mode=listen` on A and `connect` with `peer_address` on B, then run `start.bat` after packaging.

## Config

Reads `config.json` from the current working directory first, then next to the EXE. Set `CLIPBOARD_SHARE_CONFIG` to an absolute path to override. Cache and logs are stored in `data\` beside the config.

| Field | Description |
|-------|-------------|
| `listen_port` | Default sync port `24817` (pairing uses port +1) |
| `pairing_code` | Six-digit auto-pairing code (optional) |
| `auto_pair.mode` | `listen` / `connect` |
| `auto_pair.peer_address` | Peer address for connect mode, e.g. `192.168.1.10` or `192.168.1.10:24818` |
| `history_limit` | Clipboard history count when replay is enabled (default `20`) |
| `max_item_bytes` | Max total file size per clipboard item on receive (default `2 GB`) |
| `cache_bytes` | Local file cache quota under `data\cache\` (default `10 GB`) |

## File transfer

Files use the **same encrypted TCP channel** as text and images (default port `24817`, ChaCha20-Poly1305). There is no separate HTTP/SMB/file-sharing port.

1. **Metadata** — a `Clipboard` message sends a file manifest (`relative_path`, `size`, `sha256`, directory flag).
2. **Payload** — the sender reads local disk and streams **512 KB** chunks as `FileChunk` messages until each file is complete.
3. **Receive** — chunks are written to `data\cache\<item_id>\`, SHA-256 verified, then placed on the local clipboard.

**Resume:** not supported across disconnects. `offset` in `FileChunk` is only for assembling chunks within one live session. If the connection drops mid-transfer, copy the files again to restart from scratch. Partial cache directories may remain until pruned.

**Limits:**

| Limit | Default | Notes |
|-------|---------|-------|
| `max_item_bytes` | 2 GB | Total size of all files in one clipboard item (checked on receive) |
| `cache_bytes` | 10 GB | Oldest cache directories are deleted when over quota |
| `FILE_CHUNK_SIZE` | 512 KB | Fixed per-chunk size (not configurable) |
| `MAX_FRAME_SIZE` | 64 MB | Max single message frame; large file bodies are split into chunks, but text/image metadata embedded in `Clipboard` must stay under this |

The sender does not pre-check `max_item_bytes`; oversized items are rejected by the peer. Adjust limits in `config.json` as needed.

## Commands

```powershell
.\clipboard_share.exe daemon          # Run in foreground (easier to debug)
.\clipboard_share.exe start           # Start in background
.\clipboard_share.exe stop            # Stop
.\clipboard_share.exe status          # Status
.\clipboard_share.exe pair-listen     # Wait for pairing
.\clipboard_share.exe pair <ip> <code>
.\clipboard_share.exe unpair <uuid>
.\clipboard_share.exe install         # Install to %LOCALAPPDATA% and register autostart
.\clipboard_share.exe uninstall
```

Delayed logon autostart via scheduled task (recommended):

```powershell
.\setup-autostart.ps1 -InstallDir "D:\path\to\ClipboardShare" -DelaySeconds 15
```

Allow TCP `24817` / `24818` through the firewall on **Private** networks only.

## Behavior notes

- By default, only clipboard changes after connecting are synced (avoids replaying history on reconnect).
- Content hashing and brief debouncing suppress sync loops.
- Remote files are written to a local cache, SHA-256 verified, then placed on the clipboard; cross-device "cut" is treated as "copy".
- App-specific custom clipboard formats may not round-trip across devices.

## License

MIT — see [LICENSE](LICENSE).
