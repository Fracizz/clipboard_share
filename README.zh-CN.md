# ClipboardShare

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

[English](README.md) | **中文**

Windows 局域网剪贴板同步工具。推荐直接使用 CLI：两台电脑只需配对一次，即可双向同步文本、HTML、RTF、图片以及文件/目录。

> **平台（v1）：** 仅支持 Windows / macOS，暂不支持 Linux。剪贴板同步目前面向 Windows 10 1809+ / Windows 11。Windows 托盘 UI 需要 [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)，CLI 不需要。程序运行在当前登录用户会话中。

## 30 秒开始（CLI）

### 1. 准备

把 `clipboard_share.exe` 复制到两台电脑的同一个目录，并在 Windows 中开启剪贴板历史记录（按 `Win+V`）。

默认需要在局域网防火墙放行 **专用网络** TCP `24817`（同步）和 `24818`（配对）。管理员 PowerShell 可执行：

```powershell
New-NetFirewallRule -DisplayName "ClipboardShare" `
  -Direction Inbound -Protocol TCP -LocalPort 24817,24818 `
  -Profile Private -Action Allow
```

### 2. 配对一次

在电脑 A 执行，保持窗口打开：

```powershell
.\clipboard_share.exe pair-listen
# 屏幕会显示六位配对码，例如 123456
```

在电脑 B 执行（把 IP 和配对码换成 A 的实际值）：

```powershell
.\clipboard_share.exe pair 192.168.1.10 123456
```

看到“配对成功”后，在两台电脑分别启动同步：

```powershell
.\clipboard_share.exe start
.\clipboard_share.exe status
```

以后只需要 `start`；配对信息会保存在 `config.json`，密钥由 Windows DPAPI 保护。`pair-listen` 等待 5 分钟，配对码只对本次配对有效。

### 3. 验证

在任一电脑复制一段文字或一个文件，在另一台电脑按 `Win+V` 查看。排查时用前台模式：

```powershell
.\clipboard_share.exe stop
.\clipboard_share.exe daemon
```

按 `Ctrl+C` 退出前台模式。

## 配置模板（自动配对）

不想手动输入 `pair` 时，可在两台电脑分别保存下面的 `config.json`，然后直接运行 `start`。A、B 的 `device_id` 必须不同，`pairing_code` 必须相同；B 的 `peer_address` 填 A 的局域网 IP。

电脑 A：

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

电脑 B：

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

启动：

```powershell
# 两台电脑都在 config.json 所在目录执行
.\clipboard_share.exe start
.\clipboard_share.exe status
```

仓库已经提供同样的模板：`portable\A\config.json` 和 `portable\B\config.json`。打包便携版：

```powershell
.\build-portable.ps1
```

生成的 `packages\ClipboardShare-A.zip` 和 `ClipboardShare-B.zip` 解压后，按上面的 A/B 规则确认 IP 和配对码即可。

## 常用 CLI

```powershell
.\clipboard_share.exe --help
.\clipboard_share.exe start             # 后台启动
.\clipboard_share.exe stop              # 停止后台同步
.\clipboard_share.exe status            # 查看运行状态、端口和已配对设备
.\clipboard_share.exe daemon            # 前台运行，便于排错
.\clipboard_share.exe pair-listen       # 等待一次性配对码
.\clipboard_share.exe pair <ip> <code>  # 使用配对码连接
.\clipboard_share.exe unpair <uuid>     # UUID 从 status 复制
.\clipboard_share.exe install            # 复制到用户目录并设置登录自启
.\clipboard_share.exe uninstall
```

配置文件查找顺序：

1. 环境变量 `CLIPBOARD_SHARE_CONFIG` 指定的绝对路径；
2. 当前工作目录的 `config.json`；
3. EXE 同目录的 `config.json`。

缓存和日志在配置文件旁的 `data\`：文件缓存为 `data\cache\`，日志为 `data\logs\`。例如使用指定配置：

```powershell
$env:CLIPBOARD_SHARE_CONFIG = "D:\ClipboardShare\A\config.json"
.\clipboard_share.exe status
```

常用配置项：

- `listen_port`：同步端口，默认 `24817`；配对端口为它加 `1`。
- `pairing_code`：自动配对使用的六位数字。
- `auto_pair.enabled`：是否在 `start` 时自动配对。
- `auto_pair.mode`：A 使用 `listen`，B 使用 `connect`。
- `auto_pair.peer_address`：B 填 A 的 IP，例如 `192.168.1.10`；也可写 `192.168.1.10:24818`。
- `max_item_bytes`：单次传输的文件总大小上限，默认 `2 GB`。
- `cache_bytes`：本地文件缓存上限，默认 `10 GB`。

## 可选：托盘 UI

CLI 已包含完整同步功能；只有需要托盘面板时才使用 UI：

```powershell
.\clipboard_share_ui.exe
# 或
.\start-ui.bat
```

- 关闭窗口只会隐藏到托盘，不会停止同步。
- 托盘菜单可显示面板、开始/停止同步和退出。
- 面板默认使用 English；如需中文可在面板中切换，选择会保存在本地。
- Windows 缺少 WebView2 时，程序会打开 [安装页面](https://developer.microsoft.com/microsoft-edge/webview2/)；安装后重新启动 UI。

## 构建

需要 [Rust](https://rustup.rs/)（建议最新 stable）。CLI：

```powershell
cargo build --release -p clipboard_share
```

可选 UI 还需要 Node.js：

```powershell
cargo build --release -p clipboard_share_ui
```

产物：

- `target\release\clipboard_share.exe` — CLI
- `target\release\clipboard_share_ui.exe` — 托盘 UI（进程内嵌同步）

## 功能

- 双向实时同步（文本 / HTML / RTF / PNG 图片 / 文件与目录）
- 一次性配对码 + SPAKE2，持久密钥用 Windows DPAPI 保护
- 传输加密：ChaCha20-Poly1305
- 托盘 UI（Tauri 2）：状态面板、配对/启停，可最小化到系统托盘
- 便携目录：`config.json` + `data\`，可直接复制部署
- 可选登录自启（计划任务，延时启动）

## 文件传输

文件与文本、图片走**同一条加密 TCP 通道**（默认端口 `24817`，ChaCha20-Poly1305），没有单独的 HTTP/SMB/文件共享端口。

1. **元数据** — 先发 `Clipboard` 消息，携带文件清单（`relative_path`、`size`、`sha256`、是否目录）。
2. **内容** — 发送端从本地磁盘按 **512 KB** 一块读取，通过 `FileChunk` 消息逐块发送。
3. **接收** — 写入 `data\cache\<item_id>\`，校验 SHA-256 后进入本机剪贴板。

**断点续传：** 不支持跨连接续传。`FileChunk` 里的 `offset` 仅用于同一次连接内拼装分块。传输中断后需重新复制文件，从头再传；未收完的缓存目录可能残留，直到被缓存清理删除。

**上限：**

| 限制 | 默认值 | 说明 |
|------|--------|------|
| `max_item_bytes` | 2 GB | 单次剪贴板项中所有文件总大小（接收端校验） |
| `cache_bytes` | 10 GB | 超出后按时间删除最旧的缓存目录 |
| `FILE_CHUNK_SIZE` | 512 KB | 固定分块大小（不可配置） |
| `MAX_FRAME_SIZE` | 64 MB | 单条消息帧上限；大文件本体拆成多块传输，但嵌在 `Clipboard` 里的文本/图片元数据须低于此值 |

发送端不会预检 `max_item_bytes`，超限项由对端拒绝。可在 `config.json` 中按需调整。

## 登录后自启（可选）

简单自启：

```powershell
.\clipboard_share.exe install
```

需要延时启动时，推荐使用计划任务脚本：

```powershell
.\setup-autostart.ps1 -InstallDir "D:\path\to\ClipboardShare" -DelaySeconds 15
```

## 行为说明

- 默认只同步连接之后的实时剪贴板变更（避免重连回灌历史导致断连）。
- 内容哈希与短暂静默用于抑制回环重发。
- 远端文件先写入本地缓存并校验 SHA-256，再进入剪贴板；跨设备「剪切」按「复制」处理。
- 应用私有自定义剪贴板格式可能无法跨设备还原。

## 许可证

MIT — 见 [LICENSE](LICENSE)。
