# ClipboardShare

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

**English** | [中文](README.zh-CN.md)

Windows LAN clipboard sync. The CLI is the recommended entry point: after a one-time pairing, two PCs sync text, HTML, RTF, images, and files/directories in both directions.

> **Platforms (v1):** Windows and macOS only (Linux not supported). Clipboard sync currently targets Windows 10 1809+ / Windows 11. The Windows tray UI requires [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/); the CLI does not. The app runs in the logged-in user session.

## 30-second CLI quick start

### 1. Prepare

Copy `clipboard_share.exe` to a directory on both PCs. Enable Windows Clipboard history with `Win+V`.

Allow TCP `24817` (sync) and `24818` (pairing) on **Private** networks only. Run this in an elevated PowerShell on each PC:

```powershell
New-NetFirewallRule -DisplayName "ClipboardShare" `
  -Direction Inbound -Protocol TCP -LocalPort 24817,24818 `
  -Profile Private -Action Allow
```

### 2. Pair once

On PC A, keep this window open:

```powershell
.\clipboard_share.exe pair-listen
# The command prints a six-digit pairing code, for example 123456
```

On PC B, replace the IP and code with A's values:

```powershell
.\clipboard_share.exe pair 192.168.1.10 123456
```

After pairing succeeds, start sync on both PCs:

```powershell
.\clipboard_share.exe start
.\clipboard_share.exe status
```

From now on, only `start` is needed. Pairing information is stored in `config.json`, and keys are protected by Windows DPAPI. `pair-listen` waits for five minutes; its code is valid for that pairing attempt only.

### 3. Verify

Copy text or a file on either PC, then check it with `Win+V` on the other PC. For troubleshooting, run in the foreground:

```powershell
.\clipboard_share.exe stop
.\clipboard_share.exe daemon
```

Press `Ctrl+C` to exit foreground mode.

## Config templates (automatic pairing)

To avoid typing `pair` manually, save the following as `config.json` on each PC and run `start`. The A/B `device_id` values must be different, `pairing_code` must match, and B's `peer_address` must be A's LAN IP.

PC A:

```json
{
  "device_id": "11111111-1111-4111-8111-111111111111",
  "device_name": "ClipboardShare-A",
  "listen_port": 24817,
  "history_limit": 20,
  "max_item_bytes": 2147483648,
  "cache_bytes": 10737418240,
  "pairing_code": "123456",
  "auto_pair": {
    "enabled": true,
    "mode": "listen",
    "peer_address": null
  },
  "peers": []
}
```

PC B:

```json
{
  "device_id": "22222222-2222-4222-8222-222222222222",
  "device_name": "ClipboardShare-B",
  "listen_port": 24817,
  "history_limit": 20,
  "max_item_bytes": 2147483648,
  "cache_bytes": 10737418240,
  "pairing_code": "123456",
  "auto_pair": {
    "enabled": true,
    "mode": "connect",
    "peer_address": "192.168.1.10"
  },
  "peers": []
}
```

Start on both PCs from the directory containing `config.json`:

```powershell
.\clipboard_share.exe start
.\clipboard_share.exe status
```

The repository includes the same templates at `portable\A\config.json` and `portable\B\config.json`. Build portable packages with:

```powershell
.\build-portable.ps1
```

This creates `packages\ClipboardShare-A.zip` and `packages\ClipboardShare-B.zip`. After extracting, confirm the IP and pairing code in the A/B configuration.

## Common CLI commands

```powershell
.\clipboard_share.exe --help
.\clipboard_share.exe start             # Start in the background
.\clipboard_share.exe stop              # Stop background sync
.\clipboard_share.exe status            # Show state, ports, and paired devices
.\clipboard_share.exe daemon            # Run in the foreground for troubleshooting
.\clipboard_share.exe pair-listen       # Wait for a one-time pairing code
.\clipboard_share.exe pair <ip> <code>  # Connect with a pairing code
.\clipboard_share.exe unpair <uuid>     # UUID comes from status
.\clipboard_share.exe install            # Copy to the user directory and enable logon startup
.\clipboard_share.exe uninstall
```

Config lookup order:

1. Absolute path from `CLIPBOARD_SHARE_CONFIG`;
2. `config.json` in the current working directory;
3. `config.json` beside the executable.

Cache and logs are stored under `data\` beside the config: file cache in `data\cache\`, logs in `data\logs\`. Example:

```powershell
$env:CLIPBOARD_SHARE_CONFIG = "D:\ClipboardShare\A\config.json"
.\clipboard_share.exe status
```

Common settings:

- `listen_port`: sync port, default `24817`; pairing uses this port plus `1`.
- `pairing_code`: six-digit code for automatic pairing.
- `auto_pair.enabled`: automatically pair when `start` runs.
- `auto_pair.mode`: use `listen` on A and `connect` on B.
- `auto_pair.peer_address`: B's address for A, for example `192.168.1.10` or `192.168.1.10:24818`.
- `max_item_bytes`: maximum total file size per transfer, default `2 GB`.
- `cache_bytes`: local file-cache quota, default `10 GB`.

## Features

- Real-time bidirectional sync (text / HTML / RTF / PNG images / files & directories)
- One-time pairing code + SPAKE2; persistent keys protected by Windows DPAPI
- Encrypted transport: ChaCha20-Poly1305
- Tray UI (Tauri 2): status panel, pair/start/stop, minimize to the notification area
- Portable layout: `config.json` + `data\`, copy-and-run deployment
- Optional logon autostart (scheduled task with delayed start)

## Optional tray UI

```powershell
.\clipboard_share_ui.exe
# or
.\start-ui.bat
```

- Closing the window hides it in the tray; it does not stop sync.
- The tray menu can show the panel, start/stop sync, and quit.
- The panel defaults to English; switch to 中文 in the panel if needed. The preference is saved locally.
- If WebView2 is missing, the app opens the [installation page](https://developer.microsoft.com/microsoft-edge/webview2/); install it and relaunch the UI.

## Build

Requires [Rust](https://rustup.rs/) (latest stable recommended). Build the CLI:

```powershell
cargo build --release -p clipboard_share
```

The optional UI also requires Node.js:

```powershell
cargo build --release -p clipboard_share_ui
```

Outputs:

- `target\release\clipboard_share.exe` — CLI
- `target\release\clipboard_share_ui.exe` — tray UI (embeds sync in-process)

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

## Logon startup (optional)

Simple startup:

```powershell
.\clipboard_share.exe install
```

For delayed startup, use the scheduled-task script:

```powershell
.\setup-autostart.ps1 -InstallDir "D:\path\to\ClipboardShare" -DelaySeconds 15
```

## Behavior notes

- By default, only clipboard changes after connecting are synced (avoids replaying history on reconnect).
- Content hashing and brief debouncing suppress sync loops.
- Remote files are written to a local cache, SHA-256 verified, then placed on the clipboard; cross-device "cut" is treated as "copy".
- App-specific custom clipboard formats may not round-trip across devices.

## License

MIT — see [LICENSE](LICENSE).
