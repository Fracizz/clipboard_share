use std::{
    collections::HashSet,
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
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
    Win32::System::DataExchange::GetClipboardSequenceNumber,
    core::{HSTRING, Interface},
};
use windows_collections::{IIterable, IVectorView};

use crate::{
    config::AppConfig,
    protocol::{ClipboardFormat, ClipboardItem, FileEntry, MAX_ITEM_BYTES},
};

const TEXT: &str = "text/plain;charset=utf-8";
const HTML: &str = "text/html;charset=utf-8";
const RTF: &str = "text/rtf";
const BITMAP: &str = "image/windows-bitmap";
const PNG: &str = "image/png";
const WINDOWS_PNG: &str = "PNG";

/// Remote writes are suppressed by their actual clipboard generation only.
static REMOTE_SEQUENCE: AtomicU32 = AtomicU32::new(0);

/// 同一 STA 上 watch/apply 交错 await 时仍可能并发碰剪贴板，统一串行化。
static CLIPBOARD_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Debug)]
struct CaptureTooLarge {
    actual: u64,
    limit: u64,
}

impl std::fmt::Display for CaptureTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "剪贴板内容超限: {} bytes > {} bytes",
            self.actual, self.limit
        )
    }
}
impl std::error::Error for CaptureTooLarge {}

fn check_size(actual: u64, limit: u64) -> Result<()> {
    if actual > limit {
        return Err(CaptureTooLarge { actual, limit }.into());
    }
    Ok(())
}

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
        match capture_view(
            config.device_id,
            history_item.Content()?,
            config.max_item_bytes.min(MAX_ITEM_BYTES),
        )
        .await
        {
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
    _suppressed_hashes: Arc<tokio::sync::Mutex<HashSet<String>>>,
    _suppress_capture_until: Arc<tokio::sync::Mutex<tokio::time::Instant>>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let mut last_sequence = None;
    let limit = config.max_item_bytes.min(MAX_ITEM_BYTES);
    let mut interval = tokio::time::interval(Duration::from_millis(400));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                // The sequence is checked under the same STA lock used by remote writes.
                // Errors leave it pending so delayed rendering / a busy clipboard retries.
                let guard = CLIPBOARD_LOCK.lock().await;
                let sequence = unsafe { GetClipboardSequenceNumber() };
                if !should_capture_sequence(&mut last_sequence, sequence, REMOTE_SEQUENCE.load(Ordering::Acquire)) { continue; }
                let result = match Clipboard::GetContent() {
                    Ok(view) => capture_view(config.device_id, view, limit).await,
                    Err(error) => Err(error.into()),
                };
                let after = unsafe { GetClipboardSequenceNumber() };
                drop(guard);
                if after != sequence { continue; }
                match result {
                    Ok(capture) => {
                        last_sequence = Some(sequence);
                        if !capture.item.formats.is_empty() || !capture.item.files.is_empty() {
                            tracing::info!(id = %capture.item.id, files = capture.item.files.len(),
                                "检测到剪贴板变更，准备同步");
                            if sender.send(capture).await.is_err() { return Ok(()); }
                        }
                    }
                    Err(error) => {
                        if let Some(size) = error.downcast_ref::<CaptureTooLarge>() {
                            warn!(actual_bytes = size.actual, limit_bytes = size.limit,
                                "剪贴板内容超过大小限制，拒绝同步");
                            // A permanent size rejection is logged once per clipboard change.
                            last_sequence = Some(sequence);
                        } else {
                            debug!(%error, "当前剪贴板暂不可读");
                        }
                    }
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

fn should_capture_sequence(last: &mut Option<u32>, sequence: u32, remote: u32) -> bool {
    if sequence == 0 || *last == Some(sequence) {
        return false;
    }
    if sequence == remote {
        *last = Some(sequence);
        return false;
    }
    true
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
                let png = normalize_image_to_png(&bytes, MAX_ITEM_BYTES)
                    .await
                    .with_context(|| {
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

    let before = unsafe { GetClipboardSequenceNumber() };
    let committed = commit_clipboard_package(&package).await;
    let after = unsafe { GetClipboardSequenceNumber() };
    if before != after {
        REMOTE_SEQUENCE.store(after, Ordering::Release);
    }
    committed?;
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

async fn capture_view(origin: Uuid, view: DataPackageView, limit: u64) -> Result<ClipboardCapture> {
    let mut used = 0_u64;
    let mut formats = Vec::new();
    for (format, name) in [
        (StandardDataFormats::Text()?, TEXT),
        (StandardDataFormats::Html()?, HTML),
        (StandardDataFormats::Rtf()?, RTF),
    ] {
        if !view.Contains(&format)? {
            continue;
        }
        let value = match name {
            TEXT => view.GetTextAsync()?.await?,
            HTML => view.GetHtmlFormatAsync()?.await?,
            _ => view.GetRtfAsync()?.await?,
        };
        // Count UTF-8 bytes without first allocating a second huge string.
        let size: u64 = char::decode_utf16(value.iter().copied())
            .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER).len_utf8() as u64)
            .sum();
        used = used.saturating_add(size);
        check_size(used, limit)?;
        formats.push(ClipboardFormat::from_bytes(
            name,
            value.to_string().as_bytes(),
        ));
    }
    let images = capture_image_formats(&view, limit - used)
        .await
        .map_err(|error| {
            if let Some(size) = error.downcast_ref::<CaptureTooLarge>() {
                if size.limit == limit - used {
                    return CaptureTooLarge {
                        actual: used.saturating_add(size.actual),
                        limit,
                    }
                    .into();
                }
            }
            error
        });
    let (images, image_error) = match images {
        Ok(images) => (images, None),
        Err(error) if error.downcast_ref::<CaptureTooLarge>().is_some() => return Err(error),
        Err(error) => (Vec::new(), Some(error)),
    };
    for (name, bytes) in images {
        used = used.saturating_add(bytes.len() as u64);
        check_size(used, limit)?;
        formats.push(ClipboardFormat::from_bytes(name, &bytes));
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
            tokio::task::spawn_blocking(move || collect_files(&roots, limit, used)).await??;
        files = entries;
        source_files = sources;
    }

    if let Some(error) = image_error {
        if formats.is_empty() && files.is_empty() {
            return Err(error);
        }
        warn!(%error, "读取剪贴板图片失败，继续同步可读取的文本或文件");
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .try_into()?;
    let item = ClipboardItem::new(origin, now, formats, files)?;
    Ok(ClipboardCapture { item, source_files })
}

async fn capture_image_formats(
    view: &DataPackageView,
    limit: u64,
) -> Result<Vec<(&'static str, Vec<u8>)>> {
    let png = HSTRING::from(WINDOWS_PNG);
    let mut png_error = None;
    if view.Contains(&png)? {
        let result = async {
            let bytes = read_named_stream(view, &png, limit).await?;
            normalize_image_to_png(&bytes, limit).await
        }
        .await;
        match result {
            Ok(bytes) => return Ok(vec![(PNG, bytes)]),
            Err(error) if error.downcast_ref::<CaptureTooLarge>().is_some() => return Err(error),
            Err(error) => {
                debug!(%error, "读取或规范化 PNG 失败，尝试 Bitmap");
                png_error = Some(error);
            }
        }
    }
    if view.Contains(&StandardDataFormats::Bitmap()?)? {
        let reference = view.GetBitmapAsync()?.await?;
        let bytes = stream_reference_to_bytes(reference, limit).await?;
        match normalize_image_to_png(&bytes, limit).await {
            Ok(png) => return Ok(vec![(PNG, png)]),
            Err(error) if error.downcast_ref::<CaptureTooLarge>().is_some() => return Err(error),
            Err(error) => {
                warn!(%error, "Bitmap 转 PNG 失败，发送原始位图");
                return Ok(vec![(BITMAP, bytes)]);
            }
        }
    }
    if let Some(error) = png_error {
        return Err(error);
    }
    Ok(Vec::new())
}

async fn normalize_image_to_png(bytes: &[u8], limit: u64) -> Result<Vec<u8>> {
    check_size(bytes.len() as u64, limit)?;
    if is_png(bytes) {
        return Ok(bytes.to_vec());
    }
    let input = bytes_to_random_access_stream(bytes).await?;
    let input_stream: IRandomAccessStream = input.cast()?;
    let decoder = BitmapDecoder::CreateAsync(&input_stream)?.await?;
    // Reject decompression bombs before asking Windows to allocate pixels.
    let decoded_bytes = u64::from(decoder.PixelWidth()?)
        .saturating_mul(u64::from(decoder.PixelHeight()?))
        .saturating_mul(4);
    check_size(decoded_bytes, MAX_ITEM_BYTES)?;
    let software = decoder.GetSoftwareBitmapAsync()?.await?;
    let output = InMemoryRandomAccessStream::new()?;
    let output_stream: IRandomAccessStream = output.cast()?;
    let encoder =
        BitmapEncoder::CreateAsync(BitmapEncoder::PngEncoderId()?, &output_stream)?.await?;
    encoder.SetSoftwareBitmap(&software)?;
    encoder.FlushAsync()?.await?;
    output.Seek(0)?;
    let png = random_access_stream_to_bytes(&output_stream, limit).await?;
    if png.is_empty() {
        bail!("PNG 编码结果为空");
    }
    Ok(png)
}

async fn read_named_stream(
    view: &DataPackageView,
    format: &HSTRING,
    limit: u64,
) -> Result<Vec<u8>> {
    let inspectable = view.GetDataAsync(format)?.await?;
    let stream: IRandomAccessStream = inspectable.cast()?;
    random_access_stream_to_bytes(&stream, limit).await
}

async fn stream_reference_to_bytes(
    reference: RandomAccessStreamReference,
    limit: u64,
) -> Result<Vec<u8>> {
    let stream = reference.OpenReadAsync()?.await?;
    let stream: IRandomAccessStream = stream.cast()?;
    random_access_stream_to_bytes(&stream, limit).await
}

async fn random_access_stream_to_bytes(
    stream: &IRandomAccessStream,
    limit: u64,
) -> Result<Vec<u8>> {
    let size = stream.Size()?;
    if size == 0 {
        bail!("图片流大小为 0，可能是延迟渲染尚未完成");
    }
    check_size(size, limit)?;
    if size > u32::MAX as u64 {
        bail!("图片超过 4 GiB，无法读取");
    }
    stream.Seek(0)?;
    let input = stream.GetInputStreamAt(0)?;
    let reader = DataReader::CreateDataReader(&input)?;
    reader.SetInputStreamOptions(InputStreamOptions::ReadAhead)?;
    let loaded = reader.LoadAsync(size as u32)?.await?;
    if u64::from(loaded) != size {
        bail!("图片流尚未完整读取");
    }
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

fn collect_files(
    roots: &[PathBuf],
    limit: u64,
    format_bytes: u64,
) -> Result<(Vec<FileEntry>, Vec<SourceFile>)> {
    let mut entries = Vec::new();
    let mut sources = Vec::new();
    let mut total = format_bytes;
    // Enumerate metadata for the entire selection before opening any file contents.
    for root in roots {
        let name = root.file_name().context("剪贴板文件没有文件名")?;
        for child in WalkDir::new(root).follow_links(false) {
            let child = child?;
            if child.file_type().is_symlink() {
                continue;
            }
            let is_directory = child.file_type().is_dir();
            if !is_directory && !child.file_type().is_file() {
                continue;
            }
            let suffix = child.path().strip_prefix(root)?;
            let relative = if suffix.as_os_str().is_empty() {
                PathBuf::from(name)
            } else {
                PathBuf::from(name).join(suffix)
            };
            let size = if is_directory {
                0
            } else {
                child.metadata()?.len()
            };
            total = total.saturating_add(size);
            entries.push(FileEntry {
                relative_path: relative.clone(),
                size,
                sha256: String::new(),
                is_directory,
            });
            if !is_directory {
                sources.push(SourceFile {
                    source: child.path().to_owned(),
                    relative_path: relative,
                });
            }
        }
    }
    check_size(total, limit)?;
    let mut actual_total = format_bytes;
    for (entry, source) in entries.iter_mut().filter(|e| !e.is_directory).zip(&sources) {
        let (size, sha256) = hash_file(&source.source, limit, actual_total)?;
        actual_total = actual_total.saturating_add(size);
        entry.size = size;
        entry.sha256 = sha256;
    }
    Ok((entries, sources))
}

fn hash_file(path: &Path, limit: u64, already_used: u64) -> Result<(u64, String)> {
    let file = File::open(path)?;
    check_size(already_used.saturating_add(file.metadata()?.len()), limit)?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut size = 0_u64;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        size = size.saturating_add(read as u64);
        check_size(already_used.saturating_add(size), limit)?;
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
    fn sequence_tracking_preserves_local_copy_after_unobserved_remote_write() {
        let mut last = Some(10); // Local A was captured.
        // Remote B (11) was applied, but local A (12) replaced it before polling.
        assert!(should_capture_sequence(&mut last, 12, 11));
        // An unreadable attempt must remain pending for retry.
        assert!(should_capture_sequence(&mut last, 12, 11));
        last = Some(12); // Capture succeeded.
        assert!(!should_capture_sequence(&mut last, 12, 11));
        // Explicitly copying A again still represents a local change.
        assert!(should_capture_sequence(&mut last, 13, 11));
        // Polling an actual remote generation suppresses only that generation.
        assert!(!should_capture_sequence(&mut last, 14, 14));
        assert!(should_capture_sequence(&mut last, 15, 14));
        assert!(!should_capture_sequence(&mut last, 0, 14));
    }

    #[test]
    fn file_metadata_budget_includes_all_files_and_formats() {
        let root = std::env::temp_dir().join(format!("clipboard-budget-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a"), b"1234").unwrap();
        std::fs::write(root.join("b"), b"5678").unwrap();
        let error = collect_files(std::slice::from_ref(&root), 9, 2).unwrap_err();
        let oversized = error.downcast_ref::<CaptureTooLarge>().unwrap();
        assert_eq!((oversized.actual, oversized.limit), (10, 9));
        let (entries, sources) = collect_files(std::slice::from_ref(&root), 10, 2).unwrap();
        assert_eq!(sources.len(), 2);
        for entry in &entries {
            entry.validate_path().unwrap();
        }
        assert_eq!(entries.iter().map(|e| e.size).sum::<u64>(), 8);
        assert!(
            entries
                .iter()
                .filter(|e| !e.is_directory)
                .all(|e| e.sha256.len() == 64)
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn single_file_root_has_no_trailing_separator() {
        let path = std::env::temp_dir().join(format!("clipboard-file-{}", Uuid::new_v4()));
        std::fs::write(&path, b"single file").unwrap();
        let (entries, sources) =
            collect_files(std::slice::from_ref(&path), MAX_ITEM_BYTES, 0).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].relative_path.as_os_str(),
            path.file_name().unwrap()
        );
        entries[0].validate_path().unwrap();
        assert_eq!(sources[0].relative_path, entries[0].relative_path);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn empty_roots_produce_no_files() {
        let (entries, sources) = collect_files(&[], MAX_ITEM_BYTES, 0).unwrap();
        assert!(entries.is_empty());
        assert!(sources.is_empty());
    }
}
