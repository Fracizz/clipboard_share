# ClipboardShare

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

[English](README.md) | **中文**

Windows 局域网剪贴板同步工具（无 UI）。两台电脑完成一次配对后，可双向同步文本、HTML、RTF、图片以及文件/目录。

> Windows 10 1809+ / Windows 11。运行在当前登录用户会话中（不能做成 Session 0 服务，否则访问不了用户剪贴板）。

## 功能

- 双向实时同步（文本 / HTML / RTF / PNG 图片 / 文件与目录）
- 一次性配对码 + SPAKE2，持久密钥用 Windows DPAPI 保护
- 传输加密：ChaCha20-Poly1305
- 便携目录：`config.json` + `data\`，可直接复制部署
- 可选登录自启（计划任务，延时启动）

## 构建

需要 [Rust](https://rustup.rs/)（建议最新 stable）：

```powershell
cargo build --release
```

产物：`target\release\clipboard_share.exe`

生成 A/B 便携包：

```powershell
.\build-portable.ps1
```

输出：`dist\ClipboardShare-A.zip` / `dist\ClipboardShare-B.zip`

## 快速开始

1. 在 Windows 设置中开启「剪贴板历史记录」（Win+V）。
2. 在 A 机器：

   ```powershell
   .\clipboard_share.exe pair-listen
   ```

   记下显示的六位配对码。

3. 在 B 机器（把 IP 换成 A 的局域网地址）：

   ```powershell
   .\clipboard_share.exe pair 192.168.1.10 123456
   ```

4. 两边分别：

   ```powershell
   .\clipboard_share.exe start
   .\clipboard_share.exe status
   ```

也可编辑便携模板 `portable\A|B\config.json`：配置相同的 `pairing_code`，A 用 `auto_pair.mode=listen`，B 用 `connect` 并填写 `peer_address`，然后打包后直接 `start.bat`。

## 配置

优先读取当前工作目录的 `config.json`，其次 EXE 同目录；也可用环境变量 `CLIPBOARD_SHARE_CONFIG` 指定绝对路径。缓存与日志写在配置旁的 `data\`。

| 字段 | 说明 |
|------|------|
| `listen_port` | 默认同步端口 `24817`（配对临时端口为 +1） |
| `pairing_code` | 六位自动配对码（可选） |
| `auto_pair.mode` | `listen` / `connect` |
| `auto_pair.peer_address` | connect 模式对端地址，如 `192.168.1.10` 或 `192.168.1.10:24818` |
| `history_limit` | 启用历史回放时的剪贴板条数（默认 `20`） |
| `max_item_bytes` | 单次剪贴板项中所有文件总大小上限，接收端校验（默认 `2 GB`） |
| `cache_bytes` | 本地 `data\cache\` 缓存总容量（默认 `10 GB`） |

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

## 命令

```powershell
.\clipboard_share.exe daemon          # 前台运行（便于排错）
.\clipboard_share.exe start           # 后台启动
.\clipboard_share.exe stop            # 停止
.\clipboard_share.exe status          # 状态
.\clipboard_share.exe pair-listen     # 等待配对
.\clipboard_share.exe pair <ip> <code>
.\clipboard_share.exe unpair <uuid>
.\clipboard_share.exe install         # 安装到 %LOCALAPPDATA% 并写注册表自启
.\clipboard_share.exe uninstall
```

登录后延时自启（计划任务，推荐）：

```powershell
.\setup-autostart.ps1 -InstallDir "D:\path\to\ClipboardShare" -DelaySeconds 15
```

防火墙请仅允许「专用网络」上的 TCP `24817` / `24818`。

## 行为说明

- 默认只同步连接之后的实时剪贴板变更（避免重连回灌历史导致断连）。
- 内容哈希与短暂静默用于抑制回环重发。
- 远端文件先写入本地缓存并校验 SHA-256，再进入剪贴板；跨设备「剪切」按「复制」处理。
- 应用私有自定义剪贴板格式可能无法跨设备还原。

## 许可证

MIT — 见 [LICENSE](LICENSE)。
