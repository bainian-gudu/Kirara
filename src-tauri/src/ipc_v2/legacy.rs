//! JSON compatibility layer for the existing frontend.
//!
//! The native session protocol uses typed, postcard-framed IPC. The current
//! Vue frontend still sends the older `{ "type": ... }` JSON shape, so this
//! module translates both directions without exposing postcard to the UI.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::fs::commit::{FrontendCommitArgs, RecoverOutcome};
use crate::installer::registry::WriteRegistryParams;
use crate::installer::uninstall::RunUninstallArgs;
use crate::ipc_v2::install_file::{
    InstallFileArgs, InstallFileMode, InstallFileSource, InstallMultiStreamArgs,
};
use crate::ipc_v2::operation::IpcOperation;
use crate::ipc_v2::{IpcResult, Progress, StagingOpened};
use crate::utils::error::{TACommandError, TAResult};
use crate::utils::metadata::RepoMetadata;

#[derive(Debug, Clone, Copy)]
pub enum LegacyProgressKind {
    InstallFile,
    Multichunk,
    Runtime,
    LocalScan,
    Commit,
    MirrorcDownload,
    MirrorcInstall,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum LegacyInstallFileSource {
    Url {
        url: String,
        offset: usize,
        size: usize,
        #[serde(default)]
        skip_decompress: bool,
    },
    Local {
        offset: usize,
        size: usize,
        #[serde(default)]
        skip_decompress: bool,
    },
}

impl LegacyInstallFileSource {
    fn into_v2(self) -> InstallFileSource {
        match self {
            Self::Url {
                url,
                offset,
                size,
                skip_decompress,
            } => InstallFileSource::Url {
                url,
                offset,
                size,
                skip_decompress,
                request_range: None,
            },
            Self::Local {
                offset,
                size,
                skip_decompress,
            } => InstallFileSource::Local {
                offset,
                size,
                skip_decompress,
            },
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum LegacyInstallFileMode {
    Direct {
        source: LegacyInstallFileSource,
    },
    Patch {
        source: LegacyInstallFileSource,
        diff_size: usize,
    },
    HybridPatch {
        diff: LegacyInstallFileSource,
        source: LegacyInstallFileSource,
    },
}

impl LegacyInstallFileMode {
    fn into_v2(self) -> InstallFileMode {
        match self {
            Self::Direct { source } => InstallFileMode::Direct(source.into_v2()),
            Self::Patch { source, diff_size } => InstallFileMode::Patch {
                source: source.into_v2(),
                diff_size,
            },
            Self::HybridPatch { diff, source } => InstallFileMode::HybridPatch {
                diff: diff.into_v2(),
                source: source.into_v2(),
            },
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct LegacyInstallFileArgs {
    mode: LegacyInstallFileMode,
    target: String,
    #[serde(default)]
    old: Option<String>,
    #[serde(default)]
    md5: Option<String>,
    #[serde(default)]
    xxh: Option<String>,
    #[serde(default)]
    clear_installer_index_mark: Option<bool>,
}

impl LegacyInstallFileArgs {
    fn into_v2(self) -> InstallFileArgs {
        InstallFileArgs {
            mode: self.mode.into_v2(),
            target: self.target,
            old: self.old,
            md5: self.md5,
            xxh: self.xxh,
            clear_installer_index_mark: self.clear_installer_index_mark,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct LegacyInstallMultiStreamArgs {
    url: String,
    range: String,
    chunks: Vec<LegacyInstallFileArgs>,
}

impl LegacyInstallMultiStreamArgs {
    fn into_v2(self) -> InstallMultiStreamArgs {
        InstallMultiStreamArgs {
            url: self.url,
            range: self.range,
            chunks: self
                .chunks
                .into_iter()
                .map(LegacyInstallFileArgs::into_v2)
                .collect(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum LegacyIpcOperation {
    Ping,
    InstallFile(LegacyInstallFileArgs),
    InstallMultipartStream(LegacyInstallMultiStreamArgs),
    InstallMultichunkStream(LegacyInstallMultiStreamArgs),
    CreateLnk(crate::installer::lnk::CreateLnkArgs),
    WriteRegistry(WriteRegistryParams),
    CreateUninstaller {
        source: String,
        uninstaller_name: String,
        updater_name: String,
    },
    RunUninstall(RunUninstallArgs),
    FindProcessByName {
        name: String,
    },
    KillProcess {
        pid: u32,
    },
    RmList {
        list: Vec<String>,
    },
    InstallRuntime {
        tag: String,
        #[serde(default)]
        offset: Option<usize>,
        #[serde(default)]
        size: Option<usize>,
    },
    CheckLocalFiles {
        source: String,
        hash_algorithm: String,
        file_list: Vec<String>,
    },
    RunMirrorcDownload {
        url: String,
        zip_path: String,
        #[serde(default)]
        sha256: Option<String>,
    },
    RunMirrorcInstall {
        zip_path: String,
        target_path: String,
    },
    OpenStaging {
        install_dir: String,
    },
    Commit {
        staging_root: String,
        install_dir: String,
        version: String,
        #[serde(default)]
        deletes: Vec<String>,
    },
    Recover {
        staging_root: String,
        install_dir: String,
        version: String,
    },
    DiscardStaging {
        staging_root: String,
    },
}

impl LegacyIpcOperation {
    pub fn progress_kind(&self) -> LegacyProgressKind {
        match self {
            Self::InstallFile(_) => LegacyProgressKind::InstallFile,
            Self::InstallMultipartStream(_) | Self::InstallMultichunkStream(_) => {
                LegacyProgressKind::Multichunk
            }
            Self::InstallRuntime { .. } => LegacyProgressKind::Runtime,
            Self::CheckLocalFiles { .. } => LegacyProgressKind::LocalScan,
            Self::Commit { .. } | Self::Recover { .. } => LegacyProgressKind::Commit,
            Self::RunMirrorcDownload { .. } => LegacyProgressKind::MirrorcDownload,
            Self::RunMirrorcInstall { .. } => LegacyProgressKind::MirrorcInstall,
            _ => LegacyProgressKind::Commit,
        }
    }

    pub fn into_v2(self) -> IpcOperation {
        match self {
            Self::Ping => IpcOperation::Ping,
            Self::InstallFile(args) => IpcOperation::InstallFile(args.into_v2()),
            Self::InstallMultipartStream(args) => {
                IpcOperation::InstallMultipartStream(args.into_v2())
            }
            Self::InstallMultichunkStream(args) => {
                IpcOperation::InstallMultichunkStream(args.into_v2())
            }
            Self::CreateLnk(args) => IpcOperation::CreateLnk(args),
            Self::WriteRegistry(params) => IpcOperation::WriteRegistry(params),
            Self::CreateUninstaller {
                source,
                uninstaller_name,
                updater_name,
            } => IpcOperation::CreateUninstaller {
                source,
                uninstaller_name,
                updater_name,
            },
            Self::RunUninstall(args) => IpcOperation::RunUninstall(args),
            Self::FindProcessByName { name } => IpcOperation::FindProcessByName(name),
            Self::KillProcess { pid } => IpcOperation::KillProcess(pid),
            Self::RmList { list } => IpcOperation::RmList(list),
            Self::InstallRuntime { tag, offset, size } => IpcOperation::InstallRuntime {
                tag,
                offset,
                size,
                dl_dir: legacy_runtime_dl_dir(),
            },
            Self::CheckLocalFiles {
                source,
                hash_algorithm,
                file_list,
            } => IpcOperation::CheckLocalFiles {
                source,
                hash_algorithm,
                file_list,
                skip_hash: Vec::new(),
                legacy_unmanaged: true,
            },
            Self::RunMirrorcDownload {
                url,
                zip_path,
                sha256,
            } => IpcOperation::RunMirrorcDownload {
                zip_path,
                url,
                sha256,
            },
            Self::RunMirrorcInstall {
                zip_path,
                target_path,
            } => IpcOperation::RunMirrorcInstall {
                zip_path,
                new_dir: target_path,
                sha256: String::new(),
            },
            Self::OpenStaging { install_dir } => IpcOperation::OpenStaging(install_dir),
            Self::Commit {
                staging_root,
                install_dir,
                version,
                deletes,
            } => IpcOperation::CommitFrontend(FrontendCommitArgs {
                staging_root,
                install_dir,
                version,
                deletes,
            }),
            Self::Recover {
                staging_root,
                install_dir,
                version,
            } => IpcOperation::RecoverFrontend(FrontendCommitArgs {
                staging_root,
                install_dir,
                version,
                deletes: Vec::new(),
            }),
            Self::DiscardStaging { staging_root } => IpcOperation::DiscardStaging(staging_root),
        }
    }
}

fn legacy_runtime_dl_dir() -> String {
    std::env::temp_dir()
        .join("KachinaInstaller")
        .join("runtime")
        .to_string_lossy()
        .to_string()
}

pub fn progress_payload(kind: LegacyProgressKind, progress: Progress) -> Value {
    match kind {
        LegacyProgressKind::InstallFile => match progress {
            Progress::Bytes(downloaded) => json!(downloaded),
            other => generic_progress(other),
        },
        LegacyProgressKind::Multichunk => match progress {
            Progress::Chunk(index, bytes) => json!({
                "progress": bytes,
                "chunk_index": index,
            }),
            other => generic_progress(other),
        },
        LegacyProgressKind::Runtime | LegacyProgressKind::LocalScan => match progress {
            Progress::CountOf { done, total } => json!([done, total]),
            other => generic_progress(other),
        },
        LegacyProgressKind::Commit => match progress {
            Progress::CountOf { done, total } => json!([done, total]),
            other => generic_progress(other),
        },
        LegacyProgressKind::MirrorcDownload => match progress {
            Progress::BytesOf { done, total } => json!({
                "type": "download",
                "downloaded": done,
                "total": total,
            }),
            other => generic_progress(other),
        },
        LegacyProgressKind::MirrorcInstall => match progress {
            Progress::Extract { file, done, total } => json!({
                "type": "extract",
                "file": file,
                "count": done,
                "total": total,
            }),
            Progress::Delete(file) => json!({
                "type": "delete",
                "file": file,
            }),
            other => generic_progress(other),
        },
    }
}

fn generic_progress(progress: Progress) -> Value {
    match progress {
        Progress::Bytes(bytes) => json!(bytes),
        Progress::Chunk(index, bytes) => json!({
            "progress": bytes,
            "chunk_index": index,
        }),
        Progress::BytesOf { done, total } | Progress::CountOf { done, total } => {
            json!([done, total])
        }
        Progress::Extract { file, done, total } => json!({
            "type": "extract",
            "file": file,
            "count": done,
            "total": total,
        }),
        Progress::Delete(file) => json!({
            "type": "delete",
            "file": file,
        }),
    }
}

fn value<T: serde::Serialize>(value: T) -> TAResult<Value> {
    serde_json::to_value(value).map_err(|error| TACommandError::new(anyhow::Error::new(error)))
}

fn multichunk_value(result: crate::ipc_v2::install_file::MultichunkResult) -> TAResult<Value> {
    let results = result
        .results
        .into_iter()
        .map(|item| match item {
            Ok(bytes) => json!({ "Ok": bytes }),
            Err(error) => json!({ "Err": error }),
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "results": results,
        "insight": result.insight,
    }))
}

fn mirrorc_value(extract: crate::thirdparty::mirrorc::MirrorcExtract) -> TAResult<Value> {
    let metadata: Option<RepoMetadata> = extract
        .metadata
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|error| TACommandError::new(anyhow::Error::new(error)))?;
    let changeset = if extract.deletes.is_empty() {
        None
    } else {
        Some(crate::thirdparty::mirrorc::MirrorcChangeset {
            added: None,
            deleted: Some(extract.deletes),
            modified: None,
        })
    };
    value((metadata, changeset))
}

pub fn result_payload(result: IpcResult) -> TAResult<Value> {
    match result {
        IpcResult::Ping
        | IpcResult::CreateLnk
        | IpcResult::WriteRegistry
        | IpcResult::CreateUninstaller
        | IpcResult::KillProcess
        | IpcResult::RunMirrorcDownload
        | IpcResult::DiscardStaging => Ok(Value::Null),
        IpcResult::InstallFile(result) => value(result),
        IpcResult::InstallMultipartStream(result) | IpcResult::InstallMultichunkStream(result) => {
            multichunk_value(result)
        }
        IpcResult::StageSelfImage(images) => value(images),
        IpcResult::RunUninstall(outcome) => value(outcome.errors),
        IpcResult::FindProcessByName(processes) => value(processes),
        IpcResult::RmList(errors) => value(errors),
        IpcResult::InstallRuntime(status) => value(status),
        IpcResult::CheckLocalFiles(scan) => value(scan),
        IpcResult::ProbeWritable(paths) => value(paths),
        IpcResult::RunMirrorcInstall(extract) => mirrorc_value(extract),
        IpcResult::OpenStaging(StagingOpened { root, journal }) => Ok(json!({
            "staging_root": root,
            "journal": journal,
        })),
        IpcResult::Commit(outcome) => Ok(json!({
            "self_replaced": outcome.self_replaced,
            "recovered": false,
        })),
        IpcResult::Recover(outcome) => match outcome {
            RecoverOutcome::Completed { self_replaced } => Ok(json!({
                "self_replaced": self_replaced,
                "recovered": true,
            })),
            RecoverOutcome::Discarded => Ok(json!({
                "self_replaced": false,
                "recovered": false,
            })),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_legacy_install_file_shape() {
        let op: LegacyIpcOperation = serde_json::from_value(json!({
            "type": "InstallFile",
            "mode": {
                "type": "Direct",
                "source": {
                    "url": "https://example.invalid/a.bin",
                    "offset": 1,
                    "size": 2,
                    "skip_decompress": false
                }
            },
            "target": "C:\\staging\\new\\a.bin",
            "md5": "abc"
        }))
        .unwrap();
        let IpcOperation::InstallFile(args) = op.into_v2() else {
            panic!("wrong operation");
        };
        assert_eq!(args.target, "C:\\staging\\new\\a.bin");
        assert!(matches!(
            args.mode,
            InstallFileMode::Direct(InstallFileSource::Url {
                offset: 1,
                size: 2,
                ..
            })
        ));
    }

    #[test]
    fn legacy_results_keep_frontend_shapes() {
        let staging = result_payload(IpcResult::OpenStaging(StagingOpened {
            root: "C:\\staging".into(),
            journal: Some("journal".into()),
        }))
        .unwrap();
        assert_eq!(staging["staging_root"], "C:\\staging");
        assert_eq!(staging["journal"], "journal");

        let commit = result_payload(IpcResult::Commit(crate::fs::commit::CommitOutcome {
            self_replaced: true,
        }))
        .unwrap();
        assert_eq!(commit["self_replaced"], true);
        assert_eq!(commit["recovered"], false);
    }
}
