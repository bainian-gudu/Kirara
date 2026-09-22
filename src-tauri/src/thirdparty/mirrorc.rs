use std::io::Read;

use anyhow::Context;

use crate::{
    fs::{create_http_stream, create_staged_file, progressed_copy},
    installer::uninstall::is_safe_relative_member,
    utils::{
        error::{return_ta_result, IntoTAResult, TAResult},
        metadata::RepoMetadata,
        url::HttpContextExt,
    },
};

pub static MIRRORC_CRED_PREFIX: &str = "KachinaInstaller_MirrorChyanCDK_";

/// 按原始字节解出 zip 条目的名字。
///
/// zip 的 UTF-8 标志位不可信：部分打包工具写中文名时不置位，此时 zip 会按 CP437
/// 解出乱码（「中文」会变成「Σ╕¡µûç」）。这里统一按 UTF-8 解原始字节，语义与之前
/// 依赖的 zip fork 完全一致（见 LOCAL_PATCHES.md 第 14 节），因此不要改回
/// `ZipFile::name()` 或 `ZipArchive::file_names()`。
fn decode_entry_name(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).into_owned()
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug)]
pub struct MirrorcChangeset {
    pub added: Option<Vec<String>>,
    pub deleted: Option<Vec<String>>,
    pub modified: Option<Vec<String>>,
}

pub async fn run_mirrorc_install(
    zip_path: &str,
    target_path: &str,
    notify: impl Fn(serde_json::Value) + std::marker::Send + 'static,
) -> TAResult<(Option<RepoMetadata>, Option<MirrorcChangeset>)> {
    let zip_path = zip_path.to_string();
    let target_path = target_path.to_string();
    tokio::task::spawn_blocking(move || run_mirrorc_install_sync(&zip_path, &target_path, notify))
        .await
        .into_ta_result()?
}

pub fn run_mirrorc_install_sync(
    zip_path: &str,
    target_path: &str,
    notify: impl Fn(serde_json::Value) + std::marker::Send + 'static,
) -> TAResult<(Option<RepoMetadata>, Option<MirrorcChangeset>)> {
    run_mirrorc_install_inner(zip_path, target_path, notify)
}

fn run_mirrorc_install_inner(
    zip_path: &str,
    target_path: &str,
    notify: impl Fn(serde_json::Value) + std::marker::Send + 'static,
) -> TAResult<(Option<RepoMetadata>, Option<MirrorcChangeset>)> {
    let file = std::fs::File::open(zip_path).into_ta_result()?;
    let mut archive = zip::ZipArchive::new(file).into_ta_result()?;
    let total_len = archive.len();
    let target_root = std::path::Path::new(target_path);
    if !target_root.is_absolute() || crate::installer::uninstall::has_reparse_point(target_root) {
        return crate::utils::error::return_ta_result(
            "Invalid or unsafe mirrorc target path".to_string(),
            "MIRRORC_TARGET_ERR",
        );
    }

    // 逐个按索引取名字，而不是用 archive.file_names()：后者按标志位解码，
    // 没置位的中文名会变成乱码，前缀计算和后续的路径安全判定都会跟着错。
    let mut file_lists = Vec::with_capacity(total_len);
    for i in 0..total_len {
        let file = archive.by_index(i).into_ta_result()?;
        let name = decode_entry_name(file.name_raw());
        if name != "changes.json" && name != ".metadata.json" {
            file_lists.push(name);
        }
    }
    let prefix = longest_common_prefix(file_lists);
    // 拆分最后一个“/”，获取前缀
    let mut prefix = prefix.split('/').collect::<Vec<&str>>();
    prefix.pop();
    let mut prefix = prefix.join("/");
    if !prefix.is_empty() && !prefix.ends_with('/') {
        prefix.push('/');
    }

    // changes.json
    let changeset: Option<MirrorcChangeset> = match archive.by_name("changes.json") {
        Ok(mut changeset) => {
            let mut changeset_str = String::new();
            changeset
                .read_to_string(&mut changeset_str)
                .into_ta_result()?;
            Some(serde_json::from_str(&changeset_str).into_ta_result()?)
        }
        Err(_) => None,
    };

    // .元数据.json
    let metadata: Option<RepoMetadata> = match archive.by_name(&format!("{prefix}.metadata.json")) {
        Ok(mut metadata) => {
            let mut metadata_str = String::new();
            metadata
                .read_to_string(&mut metadata_str)
                .into_ta_result()?;
            Some(serde_json::from_str(&metadata_str).into_ta_result()?)
        }
        Err(_) => None,
    };

    // changeset 与 metadata 均为 None 时返回错误
    if changeset.is_none() && metadata.is_none() {
        return return_ta_result(
            "Not a valid mirrorc archive: neither changes.json nor .metadata.json found"
                .to_string(),
            "MIRRORC_ARCHIVE_ERR",
        );
    }

    for i in 0..total_len {
        let mut file = archive.by_index(i).into_ta_result()?;
        let raw_name = decode_entry_name(file.name_raw());
        let file_name = raw_name
            .strip_prefix(&prefix)
            .unwrap_or(&raw_name)
            .to_string();
        if file_name == "changes.json"
            || file_name == ".metadata.json"
            || file_name == format!("{prefix}.metadata.json")
        {
            continue;
        }
        if !is_safe_relative_member(target_root, &file_name) {
            return crate::utils::error::return_ta_result(
                format!("Unsafe archive member path: {file_name}"),
                "MIRRORC_ARCHIVE_PATH_ERR",
            );
        }
        let mut out_path = std::path::PathBuf::from(target_path);
        out_path.push(file_name.clone());
        if file.is_dir() {
            continue;
        }
        if crate::installer::uninstall::has_reparse_point(&out_path) {
            return crate::utils::error::return_ta_result(
                format!(
                    "Archive output path is a reparse point: {}",
                    out_path.display()
                ),
                "MIRRORC_ARCHIVE_PATH_ERR",
            );
        }
        let parent = out_path.parent();
        if let Some(parent) = parent {
            if !parent.exists() {
                std::fs::create_dir_all(parent)
                    .into_ta_result()
                    .context("CREATE_DIR_ERR")?;
            }
        }
        let mut out_file = std::fs::File::create(&out_path)
            .into_ta_result()
            .context(format!("CREATE_FILE_ERR: {}", out_path.display()))?;
        std::io::copy(&mut file, &mut out_file)
            .into_ta_result()
            .context(format!("WRITE_FILE_ERR: {}", out_path.display()))?;
        out_file
            .sync_all()
            .into_ta_result()
            .context(format!("SYNC_FILE_ERR: {}", out_path.display()))?;
        notify(
            serde_json::json!({"type": "extract", "file": file_name, "count": i, "total": total_len}),
        );
    }

    // 删除清单交给同一份 journal 在阶段二处理：阶段一不碰安装目录。
    let mut all_deletes: Vec<String> = Vec::new();
    if let Some(changeset) = changeset.as_ref() {
        if let Some(deletes) = changeset.deleted.as_ref() {
            for file in deletes {
                let strip_path = file.strip_prefix(&prefix).unwrap_or(file);
                if !is_safe_relative_member(target_root, strip_path) {
                    tracing::warn!("跳过不安全的 Mirrorc 删除路径: {strip_path}");
                    continue;
                }
                all_deletes.push(strip_path.to_string());
            }
        }
    }
    if let Some(metadata) = metadata.as_ref() {
        if let Some(deletes) = metadata.deletes.as_ref() {
            for file in deletes {
                if !is_safe_relative_member(target_root, file) {
                    tracing::warn!("跳过不安全的 metadata 删除路径: {file}");
                    continue;
                }
                all_deletes.push(file.clone());
            }
        }
    }
    all_deletes.sort();
    all_deletes.dedup();
    let changeset = Some(MirrorcChangeset {
        added: changeset.as_ref().and_then(|c| c.added.clone()),
        deleted: Some(all_deletes),
        modified: changeset.as_ref().and_then(|c| c.modified.clone()),
    });
    // 删除 zip 文件
    let _ = std::fs::remove_file(zip_path);
    Ok((metadata, changeset))
}

pub async fn get_mirrorc_status(
    resource_id: &str,
    current_version: &str,
    cdk: &str,
    channel: &str,
    arch: Option<&str>,
    os: Option<&str>,
) -> TAResult<serde_json::Value> {
    if resource_id.is_empty() || channel.is_empty() {
        return return_ta_result(
            "Invalid parameters for get_mirrorc_status: rid or channel is empty".to_string(),
            "MIRRORC_INVALID_PARAMS",
        );
    }
    let mut opts = String::new();
    if let Some(arch) = arch {
        opts.push_str(&format!("&arch={arch}"));
    }
    if let Some(os) = os {
        opts.push_str(&format!("&os={os}"));
    }
    let mirrorc_url = format!("https://mirrorchyan.com/api/resources/{resource_id}/latest?current_version={current_version}&cdk={cdk}&channel={channel}{opts}&user_agent=KachinaInstaller");
    let resp = crate::REQUEST_CLIENT
        .get(&mirrorc_url)
        .send()
        .await
        .with_http_context("get_mirrorc_status", &mirrorc_url)?;

    let body_text = resp
        .text()
        .await
        .with_http_context("get_mirrorc_status", &mirrorc_url)?;
    let status: serde_json::Value = serde_json::from_str(&body_text)
        .map_err(|e| anyhow::anyhow!("Failed to parse JSON ({}): {}", e, body_text))?;
    Ok(status)
}

pub async fn run_mirrorc_download(
    zip_path: &str,
    url: &str,
    sha256: Option<&str>,
    notify: impl Fn(serde_json::Value) + std::marker::Send + 'static,
) -> TAResult<()> {
    let (stream, len, _insight) = create_http_stream(url, 0, 0, true).await?;
    let target = create_staged_file(zip_path).await?;
    progressed_copy(stream, target, |downloaded| {
        notify(serde_json::json!({"type": "download", "downloaded": downloaded, "total": len}));
    })
    .await
    .context("MIRRORC_DOWNLOAD_ERR")?;

    // 接口返回的 `sha256` 此前只用来拼归档文件名，下载内容从未与之比对 —— 镜像站返回
    // 错误内容、中途被代理截断、连接复用串包都会被当成正常归档解压进安装目录。
    if let Some(expected) = sha256.map(str::trim).filter(|s| !s.is_empty()) {
        let actual = crate::utils::hash::hash_file("sha256", zip_path)
            .into_ta_result()
            .context("MIRRORC_HASH_ERR")?;
        if !actual.eq_ignore_ascii_case(expected) {
            let _ = tokio::fs::remove_file(zip_path).await;
            return crate::utils::error::return_ta_result(
                format!("Mirrorc archive digest mismatch: expected {expected}, got {actual}"),
                "MIRRORC_HASH_ERR",
            );
        }
    }
    Ok(())
}

pub fn longest_common_prefix(strs: Vec<String>) -> String {
    if strs.is_empty() {
        return String::new();
    }
    let mut prefix = strs[0].clone();
    for s in strs.iter() {
        while !s.starts_with(&prefix) {
            if prefix.is_empty() {
                return String::new();
            }
            prefix.pop();
        }
    }
    prefix
}
