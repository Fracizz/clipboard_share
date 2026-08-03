use std::{
    collections::HashSet,
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tracing::{debug, warn};
use uuid::Uuid;
use walkdir::WalkDir;
use windows::{
    ApplicationModel::DataTransfer::{
        Clipboard, ClipboardContentOptions, ClipboardHistoryItemsResultStatus, DataPackage,
        DataPackageView, StandardDataFormats,
    },
    Graphics::Imaging::{BitmapDecoder, BitmapEncoder},
    Storage::{
        IStorageItem, StorageFile, StorageFolder,
        Streams::{
            DataReader, DataWriter, IRandomAccessStream, InMemoryRandomAccessStream,
            InputStreamOptions, RandomAccessStreamReference,
        },
    },
    core::{HSTRING, Interface},
};
use windows_collections::{IIterable, IVectorView};

use crate::{
    config::AppConfig,
    protocol::{ClipboardFormat, ClipboardItem, FileEntry},
};

const TEXT: &str = "text/plain;charset=utf-8";
const HTML: &str = "text/html;charset=utf-8";
const RTF: &str = "text/rtf";
const BITMAP: &str = "image/windows-bitmap";
const PNG: &str = "image/png";
const WINDOWS_PNG: &str = "PNG";

/// 同一 STA 上 watch/apply 交错 await 时仍可能并发碰剪贴板，统一串行化。
static CLIPBOARD_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Debug, Clone)]
pub struct ClipboardCapture {
    pub item: ClipboardItem,
    pub source_files: Vec<SourceFile>,
}

#[derive(Debug, Clone)]
pub struct SourceFile {
    pub source: PathBuf,
    pub relative_path: PathBuf,
}

#[allow(dead_code)]
pub async fn initial_history(config: &AppConfig) -> Result<Vec<ClipboardCapture>> {
    if !Clipboard::IsHistoryEnabled()? {
        bail!("Windows 剪贴板历史未开启，请先按 Win+V 并启用");
    }
    let result = Clipboard::GetHistoryItemsAsync()?.await?;
    match result.Status()? {
        ClipboardHistoryItemsResultStatus::Success => {}
        ClipboardHistoryItemsResultStatus::AccessDenied => {
            bail!("Windows 拒绝访问剪贴板历史");
        }
        ClipboardHistoryItemsResultStatus::ClipboardHistoryDisabled => {
            bail!("Windows 剪贴板历史未开启");
        }
        status => bail!("读取剪贴板历史失败，状态码 {}", status.0),
    }
    let items = result.Items()?;
    let total = items.Size()? as usize;
    let take = total.min(config.history_limit);
    let mut captures = Vec::with_capacity(take);

    // Windows 返回顺序为新到旧，反向读取后发送可保持 Win+V 顺序。
    for index in (0..take).rev() {
        let history_item = items.GetAt(index as u32)?;
        let _guard = CLIPBOARD_LOCK.lock().await;
        match capture_view(config.device_id, history_item.Content()?).await {
            Ok(capture) if !capture.item.formats.is_empty() || !capture.item.files.is_empty() => {
                drop(_guard);
                captures.push(capture);
            }
            Ok(_) => {}
            Err(error) => warn!(%error, "跳过无法读取的历史项"),
        }
    }
    Ok(captures)
}

pub async fn watch(
    config: AppConfig,
    sender: mpsc::Sender<ClipboardCapture>,
    suppressed_hashes: Arc<tokio::sync::Mutex<HashSet<String>>>,
    suppress_capture_until: Arc<tokio::sync::Mutex<tokio::time::Instant>>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let mut seen = HashSet::new();
    let mut interval = tokio::time::interval(Duration::from_millis(400));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                match capture_current(config.device_id).await {
                    Ok(capture) if !capture.item.formats.is_empty() || !capture.item.files.is_empty() => {
                        let hash = capture.item.content_hash.clone();
                        let quiet = tokio::time::Instant::now()
                            < *suppress_capture_until.lock().await;
                        if quiet {
                            // 远端写入后的回读可能与原 hash 不同，静默期内一律不外发。
                            suppressed_hashes.lock().await.insert(hash.clone());
                            seen.insert(hash);
                            continue;
                        }
                        if suppressed_hashes.lock().await.remove(&hash) {
                            seen.insert(hash);
                            continue;
                        }
                        if seen.insert(hash.clone()) {
                            if seen.len() > 512 {
                                seen.clear();
                                seen.insert(hash.clone());
                            }
                            let format_names: Vec<&str> =
                                capture.item.formats.iter().map(|f| f.name.as_str()).collect();
                            tracing::info!(
                                id = %capture.item.id,
                                formats = ?format_names,
                                files = capture.item.files.len(),
                                "检测到剪贴板变更，准备同步"
                            );
                            if sender.send(capture).await.is_err() {
                                return Ok(());
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(error) => debug!(%error, "当前剪贴板暂不可读"),
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
        }
    }
}

pub async fn apply(item: &ClipboardItem, cache_root: &Path) -> Result<()> {
    item.verify_hash()?;
    let _guard = CLIPBOARD_LOCK.lock().await;
    let package = DataPackage::new()?;

    for format in &item.formats {
        let bytes = format.bytes()?;
        match format.name.as_str() {
            TEXT => package.SetText(&HSTRING::from(String::from_utf8(bytes)?))?,
            HTML => package.SetHtmlFormat(&HSTRING::from(String::from_utf8(bytes)?))?,
            RTF => package.SetRtf(&HSTRING::from(String::from_utf8(bytes)?))?,
            BITMAP | PNG => {
                if bytes.is_empty() {
                    warn!(format = %format.name, "收到空图片数据，已跳过");
                    continue;
                }
                let png = normalize_image_to_png(&bytes).await.with_context(|| {
                    format!(
                        "图片规范化失败，原始格式 {} 大小 {}",
                        format.name,
                        bytes.len()
                    )
                })?;
                let reference = bytes_to_stream_reference(&png, cache_root, PNG).await?;
                package.SetBitmap(&reference)?;
                let png_stream = bytes_to_random_access_stream(&png).await?;
                let inspectable: windows::core::IInspectable = png_stream.cast()?;
                package.SetData(&HSTRING::from(WINDOWS_PNG), &inspectable)?;
                tracing::info!(
                    bytes = png.len(),
                    source_format = %format.name,
                    "已写入 PNG 图片到剪贴板"
                );
            }
            other => debug!(format = other, "跳过不能通用写回的自定义格式"),
        }
    }

    let root_items = top_level_received_items(item, cache_root).await?;
    if !root_items.is_empty() {
        let values: Vec<Option<IStorageItem>> = root_items.into_iter().map(Some).collect();
        let view: IVectorView<IStorageItem> = values.into();
        let iterable: IIterable<IStorageItem> = view.cast()?;
        package.SetStorageItems(&iterable, true)?;
    }

    commit_clipboard_package(&package).await?;
    let format_names: Vec<&str> = item.formats.iter().map(|f| f.name.as_str()).collect();
    tracing::info!(
        id = %item.id,
        formats = ?format_names,
        files = item.files.len(),
        "已应用远端剪贴板"
    );
    Ok(())
}

async fn commit_clipboard_package(package: &DataPackage) -> Result<()> {
    let mut last_error = None;
    for attempt in 1..=5 {
        let options = ClipboardContentOptions::new()?;
        options.SetIsAllowedInHistory(true)?;
        options.SetIsRoamable(false)?;
        match Clipboard::SetContentWithOptions(package, &options) {
            Ok(true) => {
                Clipboard::Flush()?;
                return Ok(());
            }
            Ok(false) => {
                // 部分环境对 History 选项更挑剔，回退到普通 SetContent。
                if let Err(error) = Clipboard::SetContent(package) {
                    last_error = Some(anyhow::anyhow!("SetContent 失败: {error}"));
                } else if let Err(error) = Clipboard::Flush() {
                    last_error = Some(anyhow::anyhow!("Flush 失败: {error}"));
                } else {
                    return Ok(());
                }
            }
            Err(error) => {
                last_error = Some(anyhow::anyhow!("SetContentWithOptions 失败: {error}"));
            }
        }
        warn!(attempt, "写入剪贴板失败，稍后重试");
        tokio::time::sleep(Duration::from_millis(120 * attempt as u64)).await;
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Windows 拒绝写入剪贴板")))
}

async fn capture_current(origin: Uuid) -> Result<ClipboardCapture> {
    let _guard = CLIPBOARD_LOCK.lock().await;
    capture_view(origin, Clipboard::GetContent()?).await
}

async fn capture_view(origin: Uuid, view: DataPackageView) -> Result<ClipboardCapture> {
    let mut formats = Vec::new();
    if view.Contains(&StandardDataFormats::Text()?)? {
        let value = view.GetTextAsync()?.await?;
        formats.push(ClipboardFormat::from_bytes(
            TEXT,
            value.to_string().as_bytes(),
        ));
    }
    if view.Contains(&StandardDataFormats::Html()?)? {
        let value = view.GetHtmlFormatAsync()?.await?;
        formats.push(ClipboardFormat::from_bytes(
            HTML,
            value.to_string().as_bytes(),
        ));
    }
    if view.Contains(&StandardDataFormats::Rtf()?)? {
        let value = view.GetRtfAsync()?.await?;
        formats.push(ClipboardFormat::from_bytes(
            RTF,
            value.to_string().as_bytes(),
        ));
    }
    match capture_image_formats(&view).await {
        Ok(images) => {
            for (name, bytes) in images {
                tracing::info!(bytes = bytes.len(), format = %name, "已捕获剪贴板图片");
                formats.push(ClipboardFormat::from_bytes(name, &bytes));
            }
        }
        Err(error) => warn!(%error, "读取剪贴板图片失败"),
    }

    let mut files = Vec::new();
    let mut source_files = Vec::new();
    if view.Contains(&StandardDataFormats::StorageItems()?)? {
        let storage_items = view.GetStorageItemsAsync()?.await?;
        let mut roots = Vec::new();
        for item in storage_items {
            let path = PathBuf::from(item.Path()?.to_string());
            if path.exists() {
                roots.push(path);
            }
        }
        let (entries, sources) =
            tokio::task::spawn_blocking(move || collect_files(&roots)).await??;
        files = entries;
        source_files = sources;
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .try_into()?;
    let item = ClipboardItem::new(origin, now, formats, files)?;
    Ok(ClipboardCapture { item, source_files })
}

async fn capture_image_formats(view: &DataPackageView) -> Result<Vec<(&'static str, Vec<u8>)>> {
    let mut images = Vec::new();

    let png = HSTRING::from(WINDOWS_PNG);
    if view.Contains(&png)? {
        match read_named_stream(view, &png).await {
            Ok(bytes) if !bytes.is_empty() => match normalize_image_to_png(&bytes).await {
                Ok(png_bytes) => images.push((PNG, png_bytes)),
                Err(error) => warn!(%error, "PNG 规范化失败，尝试原始数据"),
            },
            Ok(_) => warn!("PNG 格式存在但内容为空"),
            Err(error) => warn!(%error, "读取 PNG 格式失败"),
        }
    }

    if images.is_empty() && view.Contains(&StandardDataFormats::Bitmap()?)? {
        match view.GetBitmapAsync()?.await {
            Ok(reference) => match stream_reference_to_bytes(reference).await {
                Ok(bytes) if !bytes.is_empty() => match normalize_image_to_png(&bytes).await {
                    Ok(png_bytes) => images.push((PNG, png_bytes)),
                    Err(error) => {
                        warn!(%error, raw_bytes = bytes.len(), "Bitmap 转 PNG 失败，发送原始位图");
                        images.push((BITMAP, bytes));
                    }
                },
                Ok(_) => warn!("Bitmap 格式存在但内容为空"),
                Err(error) => warn!(%error, "读取 Bitmap 流失败"),
            },
            Err(error) => warn!(%error, "GetBitmapAsync 失败"),
        }
    }

    Ok(images)
}

async fn normalize_image_to_png(bytes: &[u8]) -> Result<Vec<u8>> {
    if is_png(bytes) {
        return Ok(bytes.to_vec());
    }
    let input = bytes_to_random_access_stream(bytes).await?;
    let input_stream: IRandomAccessStream = input.cast()?;
    let decoder = BitmapDecoder::CreateAsync(&input_stream)?.await?;
    let software = decoder.GetSoftwareBitmapAsync()?.await?;
    let output = InMemoryRandomAccessStream::new()?;
    let output_stream: IRandomAccessStream = output.cast()?;
    let encoder =
        BitmapEncoder::CreateAsync(BitmapEncoder::PngEncoderId()?, &output_stream)?.await?;
    encoder.SetSoftwareBitmap(&software)?;
    encoder.FlushAsync()?.await?;
    output.Seek(0)?;
    let png = random_access_stream_to_bytes(&output_stream).await?;
    if png.is_empty() {
        bail!("PNG 编码结果为空");
    }
    Ok(png)
}

async fn read_named_stream(view: &DataPackageView, format: &HSTRING) -> Result<Vec<u8>> {
    let inspectable = view.GetDataAsync(format)?.await?;
    let stream: IRandomAccessStream = inspectable.cast()?;
    random_access_stream_to_bytes(&stream).await
}

async fn stream_reference_to_bytes(reference: RandomAccessStreamReference) -> Result<Vec<u8>> {
    let stream = reference.OpenReadAsync()?.await?;
    let stream: IRandomAccessStream = stream.cast()?;
    random_access_stream_to_bytes(&stream).await
}

async fn random_access_stream_to_bytes(stream: &IRandomAccessStream) -> Result<Vec<u8>> {
    let size = stream.Size()?;
    if size == 0 {
        bail!("图片流大小为 0，可能是延迟渲染尚未完成");
    }
    if size > u32::MAX as u64 {
        bail!("图片超过 4 GiB，无法读取");
    }
    stream.Seek(0)?;
    let input = stream.GetInputStreamAt(0)?;
    let reader = DataReader::CreateDataReader(&input)?;
    reader.SetInputStreamOptions(InputStreamOptions::ReadAhead)?;
    let loaded = reader.LoadAsync(size as u32)?.await?;
    let mut bytes = vec![0_u8; loaded as usize];
    reader.ReadBytes(&mut bytes)?;
    let _ = reader.DetachStream();
    Ok(bytes)
}

async fn bytes_to_stream_reference(
    bytes: &[u8],
    cache_root: &Path,
    format_name: &str,
) -> Result<RandomAccessStreamReference> {
    // 文件流比纯内存流更稳定，Win+V 缩略图也更容易生成。
    let folder = cache_root.join("images");
    tokio::fs::create_dir_all(&folder).await?;
    let extension = if is_png(bytes) || format_name == PNG {
        "png"
    } else {
        "bmp"
    };
    let path = folder.join(format!("{}.{}", Uuid::new_v4(), extension));
    tokio::fs::write(&path, bytes).await?;
    let value = HSTRING::from(path.to_string_lossy().as_ref());
    let file = StorageFile::GetFileFromPathAsync(&value)?.await?;
    Ok(RandomAccessStreamReference::CreateFromFile(&file)?)
}

async fn bytes_to_random_access_stream(bytes: &[u8]) -> Result<InMemoryRandomAccessStream> {
    let stream = InMemoryRandomAccessStream::new()?;
    stream.SetSize(bytes.len() as u64)?;
    let output = stream.GetOutputStreamAt(0)?;
    let writer = DataWriter::CreateDataWriter(&output)?;
    writer.WriteBytes(bytes)?;
    writer.StoreAsync()?.await?;
    writer.FlushAsync()?.await?;
    let _ = writer.DetachStream();
    stream.Seek(0)?;
    Ok(stream)
}

fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'])
}

fn collect_files(roots: &[PathBuf]) -> Result<(Vec<FileEntry>, Vec<SourceFile>)> {
    let mut entries = Vec::new();
    let mut sources = Vec::new();
    for root in roots {
        let name = root.file_name().context("剪贴板文件没有文件名")?.to_owned();
        if root.is_file() {
            let relative = PathBuf::from(name);
            let (size, sha256) = hash_file(root)?;
            entries.push(FileEntry {
                relative_path: relative.clone(),
                size,
                sha256,
                is_directory: false,
            });
            sources.push(SourceFile {
                source: root.clone(),
                relative_path: relative,
            });
            continue;
        }

        if root.is_dir() {
            entries.push(FileEntry {
                relative_path: PathBuf::from(&name),
                size: 0,
                sha256: String::new(),
                is_directory: true,
            });
            for child in WalkDir::new(root).follow_links(false).min_depth(1) {
                let child = child?;
                let relative = PathBuf::from(&name).join(child.path().strip_prefix(root)?);
                if child.file_type().is_symlink() {
                    continue;
                }
                if child.file_type().is_dir() {
                    entries.push(FileEntry {
                        relative_path: relative,
                        size: 0,
                        sha256: String::new(),
                        is_directory: true,
                    });
                } else if child.file_type().is_file() {
                    let (size, sha256) = hash_file(child.path())?;
                    entries.push(FileEntry {
                        relative_path: relative.clone(),
                        size,
                        sha256,
                        is_directory: false,
                    });
                    sources.push(SourceFile {
                        source: child.path().to_owned(),
                        relative_path: relative,
                    });
                }
            }
        }
    }
    Ok((entries, sources))
}

fn hash_file(path: &Path) -> Result<(u64, String)> {
    let file = File::open(path)?;
    let size = file.metadata()?.len();
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok((size, hex::encode(hasher.finalize())))
}

async fn top_level_received_items(
    item: &ClipboardItem,
    cache_root: &Path,
) -> Result<Vec<IStorageItem>> {
    let item_root = cache_root.join(item.id.to_string());
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    for entry in &item.files {
        entry.validate_path()?;
        let Some(first) = entry.relative_path.components().next() else {
            continue;
        };
        let root = PathBuf::from(first.as_os_str());
        if seen.insert(root.clone()) {
            paths.push(item_root.join(root));
        }
    }

    let mut result = Vec::new();
    for path in paths {
        let value = HSTRING::from(path.to_string_lossy().as_ref());
        let storage_item: IStorageItem = if path.is_dir() {
            StorageFolder::GetFolderFromPathAsync(&value)?
                .await?
                .cast()?
        } else {
            StorageFile::GetFileFromPathAsync(&value)?.await?.cast()?
        };
        result.push(storage_item);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_roots_produce_no_files() {
        let (entries, sources) = collect_files(&[]).unwrap();
        assert!(entries.is_empty());
        assert!(sources.is_empty());
    }
}
