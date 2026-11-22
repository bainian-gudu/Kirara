use crate::{
    cli::arg::InstallArgs,
    local::{get_config_from_embedded, get_embedded, mmap},
    pack_format::Embedded,
    utils::uac::check_elevated,
};
use anyhow::Context;
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct InstallerConfig {
    pub install_path: String,
    pub install_path_exists: bool,
    pub install_path_source: &'static str,
    pub is_uninstall: bool,
    pub embedded_files: Option<Vec<Embedded>>,
    pub embedded_index: Option<Vec<Embedded>>,
    pub embedded_config: Option<Value>,
    pub enbedded_metadata: Option<crate::utils::metadata::RepoMetadata>,
    pub embedded_image: Option<Vec<u8>>,
    pub exe_path: String,
    pub args: crate::cli::arg::InstallArgs,
    pub elevated: bool,
    pub preset: Option<crate::session::types::SessionInput>,
}

pub async fn get_config_pre(
    exe_path_path: &Path,
    args: InstallArgs,
    scan_exe: bool,
) -> anyhow::Result<InstallerConfig> {
    let exe_path = exe_path_path.to_string_lossy().to_string();
    let mut embedded_files = None;
    let mut embedded_config = None;
    let mut enbedded_metadata = None;
    let mut embedded_index = None;
    let mut embedded_image = None;
    if scan_exe {
        let file = mmap().await;
        let embedded_files_res = get_embedded(file);
        if let Ok(res) = get_config_from_embedded(&embedded_files_res).await {
            embedded_config = res.0;
            enbedded_metadata = res.1;
            embedded_index = res.2;
            embedded_image = res.3;
        }
        embedded_files = Some(embedded_files_res);
        #[cfg(debug_assertions)]
        {
            if embedded_config.is_none() {
                let exe_dir = exe_path_path.parent();
                if exe_dir.is_none() {
                    return Err(anyhow::anyhow!("Failed to get exe dir").context("GET_EXE_DIR_ERR"));
                }
                let exe_dir = exe_dir.unwrap();
                let config_json = exe_dir.join(".config.json");
                if config_json.exists() {
                    let config = tokio::fs::read(&config_json)
                        .await
                        .context("DEBUG_READ_CONFIG_ERR")?;
                    embedded_config =
                        Some(serde_json::from_slice(&config).context("DEBUG_READ_CONFIG_ERR")?);
                }
            }
        }
        #[cfg(debug_assertions)]
        {
            if embedded_image.is_none() {
                let exe_dir = exe_path_path.parent();
                if let Some(exe_dir) = exe_dir {
                    let image_bin = exe_dir.join(".image.bin");
                    if image_bin.exists() {
                        if let Ok(image_content) = tokio::fs::read(&image_bin).await {
                            embedded_image = Some(image_content);
                        }
                    }
                }
            }
        }
        let embed_name = embedded_config
            .as_ref()
            .and_then(|c| c["appName"].as_str())
            .unwrap_or("Unknown");
        let embed_source = embedded_config
            .as_ref()
            .and_then(|c| c["source"].as_str())
            .unwrap_or("Unknown");
        tracing::info!(
            "installer config: name={embed_name} source={embed_source} \
             metadata={} files={} index={} uninstall={} override_source={:?} \
             non_interactive={} silent={}",
            enbedded_metadata.is_some(),
            embedded_files.is_some(),
            embedded_index.is_some(),
            args.uninstall,
            args.source,
            args.non_interactive,
            args.silent,
        );
    }
    Ok(InstallerConfig {
        install_path: "".to_string(),
        install_path_exists: false,
        install_path_source: "",
        is_uninstall: false,
        embedded_files,
        embedded_index,
        embedded_config,
        enbedded_metadata,
        embedded_image,
        exe_path,
        args,
        elevated: check_elevated().unwrap_or(false),
        preset: None,
    })
}

impl InstallerConfig {
    /// Every block the package's `\0INDEX` lists was found by the scan where
    /// the index places it; a truncated or damaged package fails this.
    pub fn index_matches_blocks(&self) -> bool {
        let (Some(index), Some(files)) = (&self.embedded_index, &self.embedded_files) else {
            return true;
        };
        index.iter().all(|i| {
            files
                .iter()
                .find(|e| e.name == i.name)
                .is_some_and(|e| e.offset == i.offset && e.raw_offset == i.raw_offset)
        })
    }

    pub fn fill(
        mut self,
        install_path: &Path,
        install_path_exists: bool,
        install_path_source: &'static str,
    ) -> InstallerConfig {
        self.install_path = crate::utils::path::long_path(&install_path.to_string_lossy());
        self.install_path_exists = install_path_exists;
        self.install_path_source = install_path_source;
        self
    }
}

pub async fn resolve_installer_config(
    args: InstallArgs,
    scan_exe: bool,
) -> anyhow::Result<InstallerConfig> {
    let exe_path = std::env::current_exe().context("GET_EXE_PATH_ERR")?;
    let mut config = get_config_pre(&exe_path, args, scan_exe).await?;
    let mut uninstall_name = "uninst.exe";
    let mut exe_name = "main.exe";
    let mut program_files_path = "KachinaInstaller";
    let mut reg_name = "KachinaInstaller";
    let mut legacy_exe_names: Vec<String> = Vec::new();
    let mut legacy_uninstall_names: Vec<String> = Vec::new();
    let mut legacy_program_files_paths: Vec<String> = Vec::new();
    if let Some(embedded) = config.embedded_config.as_ref() {
        uninstall_name = embedded["uninstallName"].as_str().unwrap_or("uninst.exe");
        exe_name = embedded["exeName"].as_str().unwrap_or("main.exe");
        program_files_path = embedded["programFilesPath"]
            .as_str()
            .unwrap_or("KachinaInstaller");
        reg_name = embedded["regName"].as_str().unwrap_or("KachinaInstaller");
        legacy_exe_names = string_list(embedded, "legacyExeNames");
        legacy_uninstall_names = string_list(embedded, "legacyUninstallNames");
        legacy_program_files_paths = string_list(embedded, "legacyProgramFilesPaths");
    }
    let installer_file_name = exe_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let is_uninstall = installer_file_name == uninstall_name
        || legacy_uninstall_names.iter().any(|n| *n == installer_file_name);
    config.is_uninstall = is_uninstall;
    // 旧主程序名也算「本目录已有安装」：品牌改名后旧目录要能原地升级。
    let has_install = |dir: &Path| {
        dir.join(exe_name).exists() || legacy_exe_names.iter().any(|n| dir.join(n).exists())
    };
    let exe_dir = exe_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Failed to get exe dir").context("GET_EXE_PATH_ERR"))?;
    if has_install(exe_dir) {
        return Ok(config.fill(exe_dir, true, "CURRENT_DIR"));
    }
    if let Some(exe_parent_dir) = exe_dir.parent() {
        if has_install(exe_parent_dir) {
            return Ok(config.fill(exe_parent_dir, true, "PARENT_DIR"));
        }
    }
    let key_path = format!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{reg_name}");

    let key = windows_registry::LOCAL_MACHINE
        .options()
        .read()
        .open(&key_path)
        .or_else(|_| {
            windows_registry::CURRENT_USER
                .options()
                .read()
                .open(&key_path)
        });
    if let Ok(key) = key {
        match key.get_string("InstallLocation") {
            Ok(path) => {
                let path = Path::new(&path);
                if has_install(path) {
                    return Ok(config.fill(path, true, "REG"));
                }
                let sub_exe_dir = path.join(reg_name);
                if has_install(&sub_exe_dir) {
                    return Ok(config.fill(&sub_exe_dir, true, "REG_FOLDED"));
                }
            }
            Err(err) => {
                tracing::warn!(%err, "READ_REG_ERR");
            }
        }
    }

    let program_files = std::env::var("ProgramFiles").context("GET_KNOWNFOLDER_ERR")?;
    let program_files_real_path = Path::new(&program_files).join(program_files_path);
    if has_install(&program_files_real_path) {
        return Ok(config.fill(&program_files_real_path, true, "DEFAULT"));
    }
    for legacy_path in &legacy_program_files_paths {
        let legacy_real_path = Path::new(&program_files).join(legacy_path);
        if has_install(&legacy_real_path) {
            return Ok(config.fill(&legacy_real_path, true, "DEFAULT_LEGACY"));
        }
    }
    Ok(config.fill(&program_files_real_path, false, "DEFAULT"))
}

/// 配置里的字符串数组字段。缺字段、写错类型都按空表处理。
fn string_list(embedded: &Value, key: &str) -> Vec<String> {
    embedded[key]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}
