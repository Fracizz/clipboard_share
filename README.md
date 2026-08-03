# ClipboardShare

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

**English** | [中文](README.zh-CN.md)

Headless Windows LAN clipboard sync. After a one-time pairing, two PCs can sync text, HTML, RTF, images, and files/directories in both directions.

> Requires Windows 10 1809+ or Windows 11. Runs in the logged-in user session (not as a Session 0 service, which cannot access the user clipboard).

## Features

- Real-time bidirectional sync (text / HTML / RTF / PNG images / files & directories)
- One-time pairing code + SPAKE2; persistent keys protected by Windows DPAPI
- Encrypted transport: ChaCha20-Poly1305
- Portable layout: `config.json` + `data\`, copy-and-run deployment
- Optional logon autostart (scheduled task with delayed start)

## Build

Requires [Rust](https://rustup.rs/) (latest stable recommended):

```powershell
cargo build --release
```

Output: `target\release\clipboard_share.exe`

Build A/B portable packages:

```powershell
.\build-portable.ps1
```

Output: `dist\ClipboardShare-A.zip` / `dist\ClipboardShare-B.zip`

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
| `history_limit` / `max_item_bytes` / `cache_bytes` | History count, per-item size limit, cache cap |

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
