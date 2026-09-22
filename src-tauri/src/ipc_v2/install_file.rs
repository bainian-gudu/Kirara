use crate::utils::code::Attach;
use crate::{
    dfs::{apply_insight_error, InsightItem},
    fs::{
        create_http_stream, create_local_stream, create_multi_http_stream, create_staged_file,
        progressed_copy, progressed_hpatch, sync_staged_file, verify_hash,
    },
    ipc_v2::{progress_notify, IpcError, Progress, ProgressNotify},
    utils::error::{IntoTAResult, TAResult},
};

use anyhow::Result;
use async_compression::tokio::bufread::ZstdDecoder as TokioZstdDecoder;
use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, BufReader};
use tracing::info;

fn default_as_false() -> bool {
    false
}

// Helper function to check if decompression should be performed based on InstallFileArgs
fn should_decompress_chunk(args: &InstallFileArgs) -> bool {
    match &args.mode {
        InstallFileMode::Direct(source) => match source {
            InstallFileSource::Url {
                skip_decompress, ..
            } => !skip_decompress,
            InstallFileSource::Local {
                skip_decompress, ..
            } => !skip_decompress,
        },
        InstallFileMode::Patch { source, .. } => match source {
            InstallFileSource::Url {
                skip_decompress, ..
            } => !skip_decompress,
            InstallFileSource::Local {
                skip_decompress, ..
            } => !skip_decompress,
        },
        InstallFileMode::HybridPatch { diff, .. } => match diff {
            InstallFileSource::Url {
                skip_decompress, ..
            } => !skip_decompress,
            InstallFileSource::Local {
                skip_decompress, ..
            } => !skip_decompress,
        },
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct InstallResult {
    pub bytes_transferred: usize,
    pub insight: Option<InsightItem>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MultichunkResult {
    pub results: Vec<Result<usize, IpcError>>,
    pub insight: InsightItem,
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug)]
pub enum InstallFileSource {
    Url {
        url: String,
        offset: usize,
        size: usize,
        #[serde(default = "default_as_false")]
        skip_decompress: bool,
        #[serde(default)]
        request_range: Option<String>,
    },
    Local {
        offset: usize,
        size: usize,
        #[serde(default = "default_as_false")]
        skip_decompress: bool,
    },
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug)]
pub enum InstallFileMode {
    Direct(InstallFileSource),
    Patch {
        source: InstallFileSource,
        diff_size: usize,
    },
    HybridPatch {
        diff: InstallFileSource,
        source: InstallFileSource,
    },
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug)]
pub struct InstallFileArgs {
    pub mode: InstallFileMode,
    /// Output path under the staging directory's `new\`. Never a path inside
    /// the install directory.
    pub target: String,
    /// The file currently in the install directory; the base for `Patch`.
    pub old: Option<String>,
    pub md5: Option<String>,
    pub xxh: Option<String>,
    pub clear_installer_index_mark: Option<bool>,
}

/// Post-write steps shared by every mode: clear the packed index mark when
/// asked, verify the hash, flush to disk. Any failure deletes the staged file.
async fn finalize_staged(args: &InstallFileArgs, target: &Path) -> Result<()> {
    let res = async {
        if args.md5.is_some() || args.xxh.is_some() {
            if args.clear_installer_index_mark.unwrap_or(false) {
                info!("Clearing installer index mark for: {}", target.display());
                crate::installer::uninstall::clear_index_mark(&target.to_path_buf()).await?;
            }
            verify_hash(
                &target.to_string_lossy(),
                args.md5.clone(),
                args.xxh.clone(),
            )
            .await?;
        }
        sync_staged_file(&target.to_string_lossy()).await
    }
    .await;
    if res.is_err() {
        let _ = tokio::fs::remove_file(target).await;
    }
    res
}

fn old_path(args: &InstallFileArgs) -> Result<PathBuf> {
    args.old
        .as_deref()
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("patch without an old file"))
}

fn snapshot_insight(handle: &Option<Arc<Mutex<InsightItem>>>) -> Option<InsightItem> {
    handle
        .as_ref()
        .and_then(|h| h.lock().ok().map(|insight| insight.clone()))
}

fn fail_with_insight(
    err: anyhow::Error,
    handle: &Option<Arc<Mutex<InsightItem>>>,
) -> crate::utils::error::TACommandError {
    if let Some(handle) = handle {
        if let Ok(mut insight) = handle.lock() {
            apply_insight_error(&mut insight, &err);
        }
        crate::utils::error::TACommandError::with_insight_handle(err, handle.clone())
    } else {
        crate::utils::error::TACommandError::new(err)
    }
}

async fn finalize_keep_insight(
    args: &InstallFileArgs,
    target: &Path,
    handle: &Option<Arc<Mutex<InsightItem>>>,
) -> TAResult<()> {
    match finalize_staged(args, target).await {
        Ok(()) => Ok(()),
        Err(e) => {
            // verify_hash hangs HASH_MISMATCH itself; anything else here is a local io failure.
            let e = e.attach(crate::utils::code::FILE_IO_FAILED);
            if let Some(handle) = handle {
                if let Ok(mut insight) = handle.lock() {
                    apply_insight_error(&mut insight, &e);
                }
                return Err(crate::utils::error::TACommandError::with_insight_handle(
                    e,
                    handle.clone(),
                ));
            }
            Err(crate::utils::error::TACommandError::new(e))
        }
    }
}

async fn create_stream_by_source(
    source: InstallFileSource,
) -> TAResult<(
    Box<dyn tokio::io::AsyncRead + Unpin + std::marker::Send>,
    Option<Arc<Mutex<InsightItem>>>,
)> {
    match source {
        InstallFileSource::Url {
            url,
            offset,
            size,
            skip_decompress,
            request_range: _,
        } => {
            let (stream, _content_length, insight_handle) =
                create_http_stream(&url, offset, size, skip_decompress).await?;
            Ok((stream, Some(insight_handle)))
        }
        InstallFileSource::Local {
            offset,
            size,
            skip_decompress,
        } => Ok((
            create_local_stream(offset, size, skip_decompress).await?,
            None,
        )),
    }
}
pub async fn ipc_install_file(
    args: InstallFileArgs,
    notify: ProgressNotify,
) -> TAResult<InstallResult> {
    let target = PathBuf::from(&args.target);
    let progress_noti = move |downloaded: usize| {
        notify(Progress::Bytes(downloaded as u64));
    };
    match args.mode.clone() {
        InstallFileMode::Direct(source) => {
            let (mut stream, insight_handle) = create_stream_by_source(source).await?;
            let target_str = target.to_string_lossy();
            let mut target_fs = create_staged_file(&target_str).await?;
            let bytes_transferred =
                match crate::fs::progressed_copy(stream.as_mut(), &mut target_fs, &progress_noti)
                    .await
                {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        drop(target_fs);
                        let _ = tokio::fs::remove_file(&target).await;
                        return Err(fail_with_insight(e, &insight_handle));
                    }
                };
            drop(target_fs);
            let final_insight = snapshot_insight(&insight_handle);
            finalize_keep_insight(&args, &target, &insight_handle).await?;
            Ok(InstallResult {
                bytes_transferred,
                insight: final_insight,
            })
        }
        InstallFileMode::Patch { source, diff_size } => {
            let old = old_path(&args)?;
            let (stream, insight_handle) = create_stream_by_source(source).await?;
            let target_str = target.to_string_lossy();
            let bytes_transferred = match progressed_hpatch(
                stream,
                &target_str,
                diff_size,
                progress_noti,
                Some(old),
                None,
            )
            .await
            {
                Ok((v, _)) => v,
                Err(e) => return Err(fail_with_insight(e, &insight_handle)),
            };
            let final_insight = snapshot_insight(&insight_handle);
            finalize_keep_insight(&args, &target, &insight_handle).await?;
            Ok(InstallResult {
                bytes_transferred,
                insight: final_insight,
            })
        }
        InstallFileMode::HybridPatch { diff, source } => {
            // first extract the packed base next to the output (local, no insight)
            let mut base = target.as_os_str().to_owned();
            base.push(".hybrid-base");
            let base = PathBuf::from(base);
            let target_str = target.to_string_lossy();
            let base_str = base.to_string_lossy();
            let (mut source_stream, _) = create_stream_by_source(source).await?;
            let mut base_fs = create_staged_file(&base_str).await?;
            let copied =
                progressed_copy(source_stream.as_mut(), &mut base_fs, &progress_noti).await;
            drop(base_fs);
            if let Err(e) = copied {
                let _ = tokio::fs::remove_file(&base).await;
                return Err(e.into());
            }

            let size: usize = match diff {
                InstallFileSource::Url { size, .. } => size,
                InstallFileSource::Local { size, .. } => size,
            };
            let (diff_stream, insight_handle) = create_stream_by_source(diff).await?;
            let patched = progressed_hpatch(
                diff_stream,
                &target_str,
                size,
                |_| {},
                Some(base.clone()),
                None,
            )
            .await;
            let _ = tokio::fs::remove_file(&base).await;
            let diff_bytes = match patched {
                Ok((v, _)) => v,
                Err(e) => return Err(fail_with_insight(e, &insight_handle)),
            };
            let final_insight = snapshot_insight(&insight_handle);
            finalize_keep_insight(&args, &target, &insight_handle).await?;
            Ok(InstallResult {
                bytes_transferred: diff_bytes,
                insight: final_insight,
            })
        }
    }
}

pub async fn install_file_by_reader(
    args: InstallFileArgs,
    reader: &mut (dyn tokio::io::AsyncRead + Unpin + Send),
    notify: ProgressNotify,
) -> Result<usize> {
    let target = PathBuf::from(&args.target);
    let progress_noti = move |downloaded: usize| {
        notify(Progress::Bytes(downloaded as u64));
    };
    match args.mode.clone() {
        InstallFileMode::Direct(..) => {
            let target_str = target.to_string_lossy();
            let mut target_fs = create_staged_file(&target_str).await?;
            let copied = progressed_copy(reader, &mut target_fs, &progress_noti).await;
            drop(target_fs);
            let res = match copied {
                Ok(n) => n,
                Err(e) => {
                    let _ = tokio::fs::remove_file(&target).await;
                    return Err(e);
                }
            };
            finalize_staged(&args, &target).await?;
            Ok(res)
        }
        InstallFileMode::Patch { diff_size, .. } => {
            let old = old_path(&args)?;
            // copy to local buffer using progressed_copy
            let mut buffer: Vec<u8> = vec![0; diff_size];
            progressed_copy(reader, &mut buffer, &progress_noti).await?;
            let reader = std::io::Cursor::new(buffer);
            let target_str = target.to_string_lossy();
            let (res, _) = progressed_hpatch(
                Box::new(reader),
                &target_str,
                diff_size,
                |_| {},
                Some(old),
                None,
            )
            .await?;
            finalize_staged(&args, &target).await?;
            Ok(res)
        }
        InstallFileMode::HybridPatch { .. } => {
            // Hybrid patch is not supported in this function
            Err(anyhow::anyhow!(
                "Hybrid patch is not supported in this function"
            ))
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug)]
pub struct InstallMultiStreamArgs {
    pub url: String,
    pub range: String,
    pub chunks: Vec<InstallFileArgs>,
}
// Helper function to extract chunk size from InstallFileArgs
fn get_chunk_size(args: &InstallFileArgs) -> usize {
    match &args.mode {
        InstallFileMode::Direct(source) => match source {
            InstallFileSource::Url { size, .. } | InstallFileSource::Local { size, .. } => *size,
        },
        InstallFileMode::Patch { diff_size, .. } => *diff_size,
        InstallFileMode::HybridPatch { diff, .. } => match diff {
            InstallFileSource::Url { size, .. } | InstallFileSource::Local { size, .. } => *size,
        },
    }
}

// Helper function to extract chunk position from InstallFileArgs
fn get_chunk_position(args: &InstallFileArgs) -> usize {
    match &args.mode {
        InstallFileMode::Direct(source) => match source {
            InstallFileSource::Url { offset, .. } | InstallFileSource::Local { offset, .. } => {
                *offset
            }
        },
        InstallFileMode::Patch { source, .. } => match source {
            InstallFileSource::Url { offset, .. } | InstallFileSource::Local { offset, .. } => {
                *offset
            }
        },
        InstallFileMode::HybridPatch { diff, .. } => match diff {
            InstallFileSource::Url { offset, .. } | InstallFileSource::Local { offset, .. } => {
                *offset
            }
        },
    }
}

#[derive(Debug, Clone)]
struct ChunkWithPosition {
    position: usize,
    args: InstallFileArgs,
}

pub async fn ipc_install_multichunk_stream(
    args: InstallMultiStreamArgs,
    notify: ProgressNotify,
) -> TAResult<MultichunkResult> {
    // Extract chunk positions from InstallFileArgs
    let mut chunks_with_positions: Vec<ChunkWithPosition> = Vec::new();

    for chunk in &args.chunks {
        let position = get_chunk_position(chunk);
        chunks_with_positions.push(ChunkWithPosition {
            position,
            args: chunk.clone(),
        });
    }

    // Sort chunks by position to ensure proper streaming order
    chunks_with_positions.sort_by_key(|chunk| chunk.position);

    let mut results: Vec<Result<usize, IpcError>> = Vec::new();
    let mut stream_position = 0usize;
    let (insight_stream, _content_length, _content_type, insight_handle) =
        create_multi_http_stream(&args.url, &args.range).await?;

    // Convert the HTTP stream to AsyncRead
    let stream = insight_stream.map_err(std::io::Error::other);
    let mut reader = tokio_util::io::StreamReader::new(stream);

    for (chunk_index, chunk_info) in chunks_with_positions.iter().enumerate() {
        let chunk_size = get_chunk_size(&chunk_info.args);
        let chunk_offset = chunk_info.position;

        // Create enhanced notification callback with chunk info
        let chunk_notify = {
            let notify = notify.clone();
            let chunk_index = chunk_index as u32;
            progress_notify(move |progress| {
                if let Progress::Bytes(bytes) = progress {
                    notify(Progress::Chunk(chunk_index, bytes));
                }
            })
        };

        // Skip bytes until we reach the chunk position
        if stream_position < chunk_info.position {
            let skip_bytes = chunk_info.position - stream_position;
            let mut buffer = vec![0u8; 8192]; // 8KB buffer
            let mut remaining = skip_bytes;

            while remaining > 0 {
                let to_read = std::cmp::min(buffer.len(), remaining);
                let bytes_read = reader.read(&mut buffer[..to_read]).await.map_err(|e| {
                    if let Ok(mut insight) = insight_handle.lock() {
                        crate::dfs::apply_insight_io_error(&mut insight, &e);
                    }
                    crate::utils::error::TACommandError::with_insight_handle(
                        anyhow::Error::new(e).context("skip bytes"),
                        insight_handle.clone(),
                    )
                })?;

                if bytes_read == 0 {
                    return Err(crate::utils::error::TACommandError::with_insight_handle(
                        anyhow::anyhow!("Unexpected EOF while skipping bytes"),
                        insight_handle.clone(),
                    ));
                }

                remaining -= bytes_read;
            }

            stream_position = chunk_offset;
        }

        // Process chunk
        let should_decompress = should_decompress_chunk(&chunk_info.args);

        // Read chunk data into memory buffer first
        let mut chunk_buffer = vec![0u8; chunk_size];
        reader.read_exact(&mut chunk_buffer).await.map_err(|e| {
            if let Ok(mut insight) = insight_handle.lock() {
                crate::dfs::apply_insight_io_error(&mut insight, &e);
            }
            crate::utils::error::TACommandError::with_insight_handle(
                anyhow::Error::new(e).context("read chunk data"),
                insight_handle.clone(),
            )
        })?;

        let chunk_reader = std::io::Cursor::new(chunk_buffer);

        // Process chunk directly without timeout monitoring (NetworkInsightStream handles it)
        let chunk_result = if should_decompress {
            let buf_reader = BufReader::new(chunk_reader);
            let mut decompressed_reader = TokioZstdDecoder::new(buf_reader);
            install_file_by_reader(
                chunk_info.args.clone(),
                &mut decompressed_reader,
                chunk_notify,
            )
            .await
            .into_ta_result()
        } else {
            let mut raw_reader = chunk_reader;
            install_file_by_reader(chunk_info.args.clone(), &mut raw_reader, chunk_notify)
                .await
                .into_ta_result()
        };

        // Handle chunk result and update insight if there's an error
        let final_result = chunk_result.inspect_err(|e| {
            if let Ok(mut insight) = insight_handle.lock() {
                apply_insight_error(&mut insight, &e.error);
            }
        });

        results.push(match final_result {
            Ok(n) => Ok(n),
            Err(e) => Err(IpcError::from_ta(&e)),
        });
        stream_position += chunk_size;
    }

    // 获取最终的insight统计
    let final_insight = if let Ok(insight) = insight_handle.lock() {
        insight.clone()
    } else {
        InsightItem {
            url: args.url.clone(),
            ttfb: 0,
            time: 0,
            size: 0,
            error: Some("Failed to get insight".to_string()),
            range: vec![],
            mode: None,
        }
    };

    Ok(MultichunkResult {
        results,
        insight: final_insight,
    })
}

fn final_insight(handle: &Arc<Mutex<InsightItem>>, url: &str, content_length: u64) -> InsightItem {
    handle.lock().map_or_else(
        |_| InsightItem {
            url: url.to_string(),
            ttfb: 0,
            time: 0,
            size: content_length as u32,
            error: Some("Failed to get insight".to_string()),
            range: vec![],
            mode: None,
        },
        |insight| insight.clone(),
    )
}

fn chunk_error(err: anyhow::Error, handle: &Option<Arc<Mutex<InsightItem>>>) -> IpcError {
    if let Some(handle) = handle {
        if let Ok(mut insight) = handle.lock() {
            apply_insight_error(&mut insight, &err);
        }
        IpcError::from_ta(&crate::utils::error::TACommandError::with_insight_handle(
            err,
            handle.clone(),
        ))
    } else {
        IpcError::from_ta(&crate::utils::error::TACommandError::new(err))
    }
}

/// Legacy multipart/byteranges installation. The newer frontend uses
/// `InstallMultichunkStream`, but older bundles still send this operation.
pub async fn ipc_install_multipart_stream(
    args: InstallMultiStreamArgs,
    notify: ProgressNotify,
) -> TAResult<MultichunkResult> {
    let (http_stream, content_length, content_type, insight_handle) =
        create_multi_http_stream(&args.url, &args.range).await?;

    if content_type.starts_with("multipart/") {
        let boundary = content_type
            .split("boundary=")
            .nth(1)
            .ok_or_else(|| anyhow::anyhow!("Content-Type does not contain boundary"))?
            .split(';')
            .next()
            .unwrap_or_default()
            .trim();
        let mut multipart = multer::Multipart::new(http_stream, boundary);
        let mut results = Vec::new();

        while let Some(mut field) = multipart
            .next_field()
            .await
            .map_err(|e| anyhow::anyhow!("Multipart parsing error: {e}"))?
        {
            let content_range = field
                .headers()
                .get("Content-Range")
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| anyhow::anyhow!("Field does not contain Content-Range"))?;
            let range = content_range
                .split('/')
                .next()
                .and_then(|part| part.split("bytes ").nth(1))
                .map(str::trim)
                .ok_or_else(|| anyhow::anyhow!("Invalid Content-Range format: {content_range}"))?;
            let (start, end) = range.split_once('-').ok_or_else(|| {
                anyhow::anyhow!("Invalid range format in Content-Range: {content_range}")
            })?;
            let start: usize = start
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid start range: {content_range}"))?;
            let end: usize = end
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid end range: {content_range}"))?;

            let Some((chunk_index, chunk)) = args.chunks.iter().enumerate().find(|(_, chunk)| {
                let source_size = get_chunk_size(chunk);
                let source_pos = get_chunk_position(chunk);
                start == source_pos && end == source_pos + source_size - 1
            }) else {
                return Err(
                    anyhow::anyhow!("No matching chunk found for range: {content_range}").into(),
                );
            };

            let mut field_data = Vec::new();
            while let Some(chunk_bytes) = field.chunk().await.map_err(|e| {
                if let Ok(mut insight) = insight_handle.lock() {
                    insight.error = Some(e.to_string());
                }
                anyhow::anyhow!("Field chunk read error: {e}")
            })? {
                field_data.extend_from_slice(&chunk_bytes);
            }

            let chunk_notify = {
                let notify = notify.clone();
                let chunk_index = chunk_index as u32;
                progress_notify(move |progress| {
                    if let Progress::Bytes(bytes) = progress {
                        notify(Progress::Chunk(chunk_index, bytes));
                    }
                })
            };
            let reader = std::io::Cursor::new(field_data);
            let result = if should_decompress_chunk(chunk) {
                let mut reader = TokioZstdDecoder::new(reader);
                install_file_by_reader(chunk.clone(), &mut reader, chunk_notify).await
            } else {
                let mut reader = reader;
                install_file_by_reader(chunk.clone(), &mut reader, chunk_notify).await
            };
            results.push(result.map_err(|e| chunk_error(e, &Some(insight_handle.clone()))));
        }

        return Ok(MultichunkResult {
            results,
            insight: final_insight(&insight_handle, &args.url, content_length),
        });
    }

    let Some(first_chunk) = args.chunks.first() else {
        return Err(anyhow::anyhow!("No chunks provided for multi-stream installation").into());
    };
    let source_size = get_chunk_size(first_chunk);
    if content_length != source_size as u64 {
        return Err(anyhow::anyhow!(
            "Server does not support multipart range, and cannot send the first chunk correctly (expected size: {source_size}, got: {content_length})"
        )
        .into());
    }

    let stream = http_stream.map_err(std::io::Error::other);
    let reader = tokio_util::io::StreamReader::new(stream);
    let chunk_notify = {
        let notify = notify.clone();
        progress_notify(move |progress| {
            if let Progress::Bytes(bytes) = progress {
                notify(Progress::Chunk(0, bytes));
            }
        })
    };
    let result = if should_decompress_chunk(first_chunk) {
        let mut reader = TokioZstdDecoder::new(reader);
        install_file_by_reader(first_chunk.clone(), &mut reader, chunk_notify).await
    } else {
        let mut reader = reader;
        install_file_by_reader(first_chunk.clone(), &mut reader, chunk_notify).await
    };
    Ok(MultichunkResult {
        results: vec![result.map_err(|e| chunk_error(e, &Some(insight_handle.clone())))],
        insight: final_insight(&insight_handle, &args.url, content_length),
    })
}
