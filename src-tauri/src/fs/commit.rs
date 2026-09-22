use anyhow::{Context, Result};
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::staging::{self, JOURNAL_VERSION, NEW_DIR, OLD_DIR};

const HASH_ALGORITHM: &str = "sha256";

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug)]
pub struct CommitArgs {
    pub staging_root: String,
    pub install_dir: String,
    pub version: String,
    #[serde(default)]
    pub deletes: Vec<String>,
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct CommitOutcome {
    pub self_replaced: bool,
    pub recovered: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileEntry {
    rel: String,
    old: Option<String>,
    new: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Unit {
    File(FileEntry),
    Del { rel: String, old: Option<String> },
}

#[derive(Clone, Copy, Debug, Default)]
struct FileState {
    old_moved: bool,
    new_placed: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct DelState {
    old_moved: bool,
}

#[derive(Clone, Copy, Debug)]
enum UnitState {
    Skipped,
    File(FileState),
    Del(DelState),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Done,
    Pending,
    Unrecoverable,
    Changed,
}

fn safe_rel(install_dir: &Path, rel: &str) -> bool {
    !rel.is_empty() && crate::installer::uninstall::is_safe_relative_member(install_dir, rel)
}

fn hash_opt(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    if !path.is_file() {
        return Err(anyhow::anyhow!("Expected a file at {}", path.display())
            .context("STAGING_PATH_INVALID"));
    }
    Ok(Some(
        crate::utils::hash::hash_file(HASH_ALGORITHM, &path.to_string_lossy())
            .context("STAGING_HASH_ERR")?,
    ))
}

fn walk_files(root: &Path, current: &Path, out: &mut Vec<String>) -> Result<()> {
    let entries = match std::fs::read_dir(current) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).context("STAGING_WALK_ERR"),
    };
    for entry in entries {
        let entry = entry.context("STAGING_WALK_ERR")?;
        let path = entry.path();
        let file_type = entry.file_type().context("STAGING_WALK_ERR")?;
        if file_type.is_dir() {
            walk_files(root, &path, out)?;
        } else if file_type.is_file() {
            let rel = path
                .strip_prefix(root)
                .context("STAGING_REL_ERR")?
                .to_string_lossy()
                .replace('\\', "/");
            if rel.contains('\t') || rel.contains('\n') || rel.contains('\r') {
                return Err(anyhow::anyhow!("Unsupported staged path: {rel}")
                    .context("STAGING_PATH_INVALID"));
            }
            out.push(rel);
        }
    }
    Ok(())
}

fn build_units(staging_root: &Path, install_dir: &Path, deletes: &[String]) -> Result<Vec<Unit>> {
    let new_root = staging_root.join(NEW_DIR);
    let mut files = Vec::new();
    walk_files(&new_root, &new_root, &mut files)?;
    files.sort();

    let mut units = Vec::new();
    let mut file_rels = HashSet::new();
    for rel in files {
        if !safe_rel(install_dir, &rel) {
            return Err(
                anyhow::anyhow!("Unsafe staged file path: {rel}").context("STAGING_PATH_INVALID")
            );
        }
        let new_path = new_root.join(&rel);
        let new_hash = hash_opt(&new_path)?
            .ok_or_else(|| anyhow::anyhow!("Staged file disappeared: {rel}"))?;
        let old_hash = hash_opt(&install_dir.join(&rel))?;
        file_rels.insert(rel.replace('\\', "/").to_ascii_lowercase());
        units.push(Unit::File(FileEntry {
            rel,
            old: old_hash,
            new: new_hash,
        }));
    }

    for rel in deletes {
        let rel = rel.replace('\\', "/");
        if !safe_rel(install_dir, &rel) {
            tracing::warn!("跳过不安全的删除路径: {rel}");
            continue;
        }
        if file_rels.contains(&rel.to_ascii_lowercase()) {
            tracing::warn!("删除路径同时存在于本次文件清单，按文件更新处理: {rel}");
            continue;
        }
        let target = install_dir.join(&rel);
        let old = if target.exists() {
            if !target.is_file() {
                tracing::warn!("跳过删除非文件目标: {rel}");
                continue;
            }
            hash_opt(&target)?
        } else {
            None
        };
        units.push(Unit::Del { rel, old });
    }
    Ok(units)
}

fn journal_text(version: &str, units: &[Unit]) -> String {
    let mut text = String::new();
    text.push_str(JOURNAL_VERSION);
    text.push('\n');
    text.push_str("version\t");
    text.push_str(version);
    text.push('\n');
    text.push_str("hash\t");
    text.push_str(HASH_ALGORITHM);
    text.push('\n');
    for unit in units {
        match unit {
            Unit::File(entry) => {
                text.push_str("file\t");
                text.push_str(&entry.rel);
                text.push('\t');
                text.push_str(entry.old.as_deref().unwrap_or("-"));
                text.push('\t');
                text.push_str(&entry.new);
                text.push('\n');
            }
            Unit::Del { rel, old } => {
                text.push_str("del\t");
                text.push_str(rel);
                text.push('\t');
                text.push_str(old.as_deref().unwrap_or("-"));
                text.push('\n');
            }
        }
    }
    text
}

fn parse_opt_hash(value: &str) -> Option<String> {
    if value == "-" {
        None
    } else {
        Some(value.to_string())
    }
}

fn parse_journal(text: &str) -> Result<(String, Vec<Unit>)> {
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some(JOURNAL_VERSION) {
        return Err(anyhow::anyhow!("Unsupported journal version").context("STAGING_JOURNAL_ERR"));
    }
    let mut version = None;
    let mut hash_algorithm = None;
    let mut units = Vec::new();
    for line in lines {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(4, '\t');
        let kind = parts.next().context("STAGING_JOURNAL_ERR")?;
        match kind {
            "version" => {
                version = Some(parts.next().context("STAGING_JOURNAL_ERR")?.to_string());
            }
            "hash" => {
                hash_algorithm = Some(parts.next().context("STAGING_JOURNAL_ERR")?.to_string());
            }
            "file" => {
                let rel = parts
                    .next()
                    .context("STAGING_JOURNAL_ERR")?
                    .replace('\\', "/");
                let old = parse_opt_hash(parts.next().context("STAGING_JOURNAL_ERR")?);
                let new = parts.next().context("STAGING_JOURNAL_ERR")?.to_string();
                units.push(Unit::File(FileEntry { rel, old, new }));
            }
            "del" => {
                let rel = parts
                    .next()
                    .context("STAGING_JOURNAL_ERR")?
                    .replace('\\', "/");
                let old = parse_opt_hash(parts.next().context("STAGING_JOURNAL_ERR")?);
                units.push(Unit::Del { rel, old });
            }
            _ => return Err(anyhow::anyhow!("Unknown journal unit").context("STAGING_JOURNAL_ERR")),
        }
    }
    let version = version.context("STAGING_JOURNAL_ERR")?;
    if hash_algorithm.as_deref() != Some(HASH_ALGORITHM) {
        return Err(anyhow::anyhow!("Unsupported journal hash").context("STAGING_JOURNAL_ERR"));
    }
    Ok((version, units))
}

fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("STAGING_CREATE_ERR")?;
    }
    Ok(())
}

fn rename_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    ensure_parent(to).map_err(|e| std::io::Error::other(format!("{e:#}")))?;
    let mut delay = std::time::Duration::from_millis(50);
    let mut last = None;
    for _ in 0..5 {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(delay);
                delay *= 2;
            }
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("rename failed")))
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).context("STAGING_ROLLBACK_ERR"),
    }
}

fn target_path(install_dir: &Path, rel: &str) -> PathBuf {
    install_dir.join(rel)
}

fn new_path(staging_root: &Path, rel: &str) -> PathBuf {
    staging_root.join(NEW_DIR).join(rel)
}

fn old_path(staging_root: &Path, rel: &str) -> PathBuf {
    staging_root.join(OLD_DIR).join(rel)
}

fn undo_file(staging_root: &Path, install_dir: &Path, rel: &str, state: FileState) -> Result<()> {
    let target = target_path(install_dir, rel);
    let new = new_path(staging_root, rel);
    let old = old_path(staging_root, rel);
    if state.new_placed && target.exists() {
        rename_retry(&target, &new).context("STAGING_ROLLBACK_ERR")?;
    }
    if state.old_moved && old.exists() {
        remove_if_exists(&target)?;
        rename_retry(&old, &target).context("STAGING_ROLLBACK_ERR")?;
    }
    Ok(())
}

fn undo_del(staging_root: &Path, install_dir: &Path, rel: &str, state: DelState) -> Result<()> {
    if !state.old_moved {
        return Ok(());
    }
    let target = target_path(install_dir, rel);
    let old = old_path(staging_root, rel);
    if target.exists() {
        return Err(
            anyhow::anyhow!("Cannot restore deleted target: {}", target.display())
                .context("STAGING_ROLLBACK_ERR"),
        );
    }
    if old.exists() {
        rename_retry(&old, &target).context("STAGING_ROLLBACK_ERR")?;
    }
    Ok(())
}

fn undo_unit(staging_root: &Path, install_dir: &Path, unit: &Unit, state: UnitState) -> Result<()> {
    match (unit, state) {
        (_, UnitState::Skipped) => Ok(()),
        (Unit::File(entry), UnitState::File(state)) => {
            undo_file(staging_root, install_dir, &entry.rel, state)
        }
        (Unit::Del { rel, .. }, UnitState::Del(state)) => {
            undo_del(staging_root, install_dir, rel, state)
        }
        _ => Err(anyhow::anyhow!("Journal state mismatch").context("STAGING_ROLLBACK_ERR")),
    }
}

fn rollback_units(
    staging_root: &Path,
    install_dir: &Path,
    units: &[Unit],
    states: &[UnitState],
) -> Result<()> {
    let mut errors = Vec::new();
    for (unit, state) in units.iter().zip(states.iter()).rev() {
        if let Err(e) = undo_unit(staging_root, install_dir, unit, *state) {
            errors.push(format!("{e:#}"));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(anyhow::anyhow!("rollback failed: {}", errors.join("; ")))
    }
}

fn apply_file(staging_root: &Path, install_dir: &Path, entry: &FileEntry) -> Result<FileState> {
    let target = target_path(install_dir, &entry.rel);
    let new = new_path(staging_root, &entry.rel);
    let old = old_path(staging_root, &entry.rel);
    let mut state = FileState::default();

    if target.is_dir() {
        return Err(
            anyhow::anyhow!("Target is a directory: {}", target.display())
                .context("STAGING_PATH_INVALID"),
        );
    }
    if hash_opt(&target)?.as_deref() == Some(entry.new.as_str()) {
        state.new_placed = true;
        state.old_moved = old.exists();
        return Ok(state);
    }
    if target.exists() && old.exists() {
        return Err(anyhow::anyhow!("Staging conflict for {}", target.display())
            .context("STAGING_PATH_INVALID"));
    }
    if target.exists() {
        ensure_parent(&old)?;
        rename_retry(&target, &old).context("FILE_IN_USE")?;
        state.old_moved = true;
    } else if old.exists() {
        state.old_moved = true;
    }
    if !new.is_file() {
        let undo = undo_file(staging_root, install_dir, &entry.rel, state);
        if let Err(undo_err) = undo {
            return Err(undo_err
                .context("ROLLBACK_FAILED")
                .context(format!("staged file missing: {}", entry.rel)));
        }
        return Err(
            anyhow::anyhow!("Staged file missing: {}", entry.rel).context("STAGING_FILE_MISSING")
        );
    }
    if let Err(e) = rename_retry(&new, &target) {
        let undo = undo_file(staging_root, install_dir, &entry.rel, state);
        if let Err(undo_err) = undo {
            return Err(undo_err
                .context("ROLLBACK_FAILED")
                .context(format!("{e:#}")));
        }
        return Err(anyhow::Error::new(e).context("FILE_IN_USE"));
    }
    state.new_placed = true;
    Ok(state)
}

fn apply_del(staging_root: &Path, install_dir: &Path, rel: &str) -> Result<DelState> {
    let target = target_path(install_dir, rel);
    if !target.exists() {
        return Ok(DelState::default());
    }
    if !target.is_file() {
        return Err(
            anyhow::anyhow!("Target is not a file: {}", target.display())
                .context("STAGING_PATH_INVALID"),
        );
    }
    let old = old_path(staging_root, rel);
    ensure_parent(&old)?;
    remove_if_exists(&old)?;
    rename_retry(&target, &old).context("FILE_IN_USE")?;
    Ok(DelState { old_moved: true })
}

fn apply_unit(staging_root: &Path, install_dir: &Path, unit: &Unit) -> Result<UnitState> {
    match unit {
        Unit::File(entry) => apply_file(staging_root, install_dir, entry).map(UnitState::File),
        Unit::Del { rel, .. } => apply_del(staging_root, install_dir, rel).map(UnitState::Del),
    }
}

fn apply_units(staging_root: &Path, install_dir: &Path, units: &[Unit]) -> Result<()> {
    let mut states = Vec::with_capacity(units.len());
    for unit in units {
        match apply_unit(staging_root, install_dir, unit) {
            Ok(state) => states.push(state),
            Err(e) => {
                if let Err(rb) = rollback_units(staging_root, install_dir, units, &states) {
                    return Err(rb.context("ROLLBACK_FAILED").context(format!("{e:#}")));
                }
                return Err(e);
            }
        }
    }
    Ok(())
}

fn classify_file(staging_root: &Path, install_dir: &Path, entry: &FileEntry) -> Result<Status> {
    let target = target_path(install_dir, &entry.rel);
    let new = new_path(staging_root, &entry.rel);
    let old = old_path(staging_root, &entry.rel);
    let target_hash = hash_opt(&target)?;
    if target_hash.as_deref() == Some(entry.new.as_str()) {
        return Ok(Status::Done);
    }
    if target_hash == entry.old {
        return Ok(if new.is_file() {
            Status::Pending
        } else {
            Status::Changed
        });
    }
    if !target.exists() && old.exists() {
        let old_hash = hash_opt(&old)?;
        if old_hash == entry.old && new.is_file() {
            return Ok(Status::Pending);
        }
        return Ok(Status::Unrecoverable);
    }
    if !target.exists() && !old.exists() && entry.old.is_none() && new.is_file() {
        return Ok(Status::Pending);
    }
    Ok(Status::Changed)
}

fn classify_del(install_dir: &Path, rel: &str, old: Option<&str>) -> Result<Status> {
    let target_hash = hash_opt(&target_path(install_dir, rel))?;
    if target_hash.is_none() {
        return Ok(Status::Done);
    }
    if target_hash.as_deref() == old {
        return Ok(Status::Pending);
    }
    Ok(Status::Changed)
}

fn classify(staging_root: &Path, install_dir: &Path, unit: &Unit) -> Result<Status> {
    match unit {
        Unit::File(entry) => classify_file(staging_root, install_dir, entry),
        Unit::Del { rel, old } => classify_del(install_dir, rel, old.as_deref()),
    }
}

fn done_state(staging_root: &Path, unit: &Unit) -> UnitState {
    match unit {
        Unit::File(entry) => UnitState::File(FileState {
            old_moved: old_path(staging_root, &entry.rel).exists(),
            new_placed: true,
        }),
        Unit::Del { rel, .. } => UnitState::Del(DelState {
            old_moved: old_path(staging_root, rel).exists(),
        }),
    }
}

fn self_replaced(install_dir: &Path, units: &[Unit]) -> bool {
    let Ok(current) = std::env::current_exe() else {
        return false;
    };
    units.iter().any(|unit| match unit {
        Unit::File(entry) => {
            crate::installer::uninstall::path_eq(&current, &install_dir.join(&entry.rel))
        }
        Unit::Del { rel, .. } => {
            crate::installer::uninstall::path_eq(&current, &install_dir.join(rel))
        }
    })
}

fn finish(staging_root: &Path, self_replaced: bool, recovered: bool) -> CommitOutcome {
    if self_replaced {
        crate::installer::uninstall::schedule_delete_on_exit(staging_root);
    } else {
        staging::discard(&staging_root.to_string_lossy());
    }
    CommitOutcome {
        self_replaced,
        recovered,
    }
}

fn write_journal(path: &Path, version: &str, units: &[Unit]) -> Result<()> {
    let mut file = std::fs::File::create(path).context("STAGING_JOURNAL_WRITE_ERR")?;
    file.write_all(journal_text(version, units).as_bytes())
        .context("STAGING_JOURNAL_WRITE_ERR")?;
    file.sync_all().context("STAGING_JOURNAL_SYNC_ERR")?;
    Ok(())
}

pub fn commit(args: CommitArgs) -> Result<CommitOutcome> {
    let staging_root = PathBuf::from(&args.staging_root);
    let install_dir = PathBuf::from(&args.install_dir);
    let units = build_units(&staging_root, &install_dir, &args.deletes)?;
    let journal = staging::journal_path(&staging_root);
    write_journal(&journal, &args.version, &units)?;
    if let Err(e) = apply_units(&staging_root, &install_dir, &units) {
        if !format!("{e:#}").contains("ROLLBACK_FAILED") {
            let _ = std::fs::remove_file(&journal);
            staging::discard(&args.staging_root);
        } else {
            tracing::error!("提交回滚失败，保留 journal 与暂存目录以便下次恢复: {e:#}");
        }
        return Err(e);
    }
    let replaced = self_replaced(&install_dir, &units);
    let _ = std::fs::remove_file(&journal);
    Ok(finish(&staging_root, replaced, false))
}

pub fn recover(args: CommitArgs) -> Result<CommitOutcome> {
    let staging_root = PathBuf::from(&args.staging_root);
    let install_dir = PathBuf::from(&args.install_dir);
    let journal = staging::journal_path(&staging_root);
    let text = match std::fs::read_to_string(&journal) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CommitOutcome {
                self_replaced: false,
                recovered: false,
            })
        }
        Err(e) => return Err(e).context("STAGING_JOURNAL_READ_ERR"),
    };
    let (version, units) = match parse_journal(&text) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("忽略无法解析的 journal: {e:#}");
            staging::discard(&args.staging_root);
            return Ok(CommitOutcome {
                self_replaced: false,
                recovered: false,
            });
        }
    };
    if version != args.version {
        tracing::warn!("暂存目录版本不匹配（{version} != {}），丢弃", args.version);
        staging::discard(&args.staging_root);
        return Ok(CommitOutcome {
            self_replaced: false,
            recovered: false,
        });
    }

    for unit in &units {
        let rel = match unit {
            Unit::File(entry) => &entry.rel,
            Unit::Del { rel, .. } => rel,
        };
        if !safe_rel(&install_dir, rel) {
            tracing::warn!("journal 含不安全的相对路径，丢弃暂存目录");
            staging::discard(&args.staging_root);
            return Ok(CommitOutcome {
                self_replaced: false,
                recovered: false,
            });
        }
    }

    let mut statuses = Vec::with_capacity(units.len());
    for unit in &units {
        statuses.push(classify(&staging_root, &install_dir, unit)?);
    }
    if statuses.iter().any(|status| *status == Status::Changed) {
        tracing::warn!("暂存目录与安装目录现状不一致，丢弃暂存目录");
        staging::discard(&args.staging_root);
        return Ok(CommitOutcome {
            self_replaced: false,
            recovered: false,
        });
    }

    let mut states = vec![UnitState::Skipped; units.len()];
    for (i, (unit, status)) in units.iter().zip(statuses.iter()).enumerate() {
        match status {
            Status::Done => states[i] = done_state(&staging_root, unit),
            Status::Pending => match apply_unit(&staging_root, &install_dir, unit) {
                Ok(state) => states[i] = state,
                Err(e) => {
                    if let Err(rb) = rollback_units(&staging_root, &install_dir, &units, &states) {
                        return Err(rb.context("ROLLBACK_FAILED").context(format!("{e:#}")));
                    }
                    let _ = std::fs::remove_file(&journal);
                    staging::discard(&args.staging_root);
                    return Err(e);
                }
            },
            Status::Unrecoverable => {
                for (i, status) in statuses.iter().enumerate() {
                    match *status {
                        Status::Done => states[i] = done_state(&staging_root, &units[i]),
                        Status::Unrecoverable => {
                            states[i] = match &units[i] {
                                Unit::File(entry) => UnitState::File(FileState {
                                    old_moved: old_path(&staging_root, &entry.rel).exists(),
                                    new_placed: false,
                                }),
                                Unit::Del { rel, .. } => UnitState::Del(DelState {
                                    old_moved: old_path(&staging_root, rel).exists(),
                                }),
                            };
                        }
                        _ => {}
                    }
                }
                if let Err(rb) = rollback_units(&staging_root, &install_dir, &units, &states) {
                    return Err(rb.context("ROLLBACK_FAILED").context("STAGING_RECOVER_ERR"));
                }
                staging::discard(&args.staging_root);
                return Ok(CommitOutcome {
                    self_replaced: false,
                    recovered: false,
                });
            }
            Status::Changed => unreachable!(),
        }
    }

    let replaced = self_replaced(&install_dir, &units);
    let _ = std::fs::remove_file(&journal);
    Ok(finish(&staging_root, replaced, true))
}

pub fn discard(staging_root: &str) {
    staging::discard(staging_root);
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn test_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "kirara-commit-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn sha256(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn staged_file(rel: &str, new: &[u8]) -> Unit {
        Unit::File(FileEntry {
            rel: rel.to_string(),
            old: None,
            new: sha256(new),
        })
    }

    #[test]
    fn journal_roundtrip_and_version_gate() {
        let units = vec![
            staged_file("a.txt", b"new"),
            Unit::Del {
                rel: "b.txt".to_string(),
                old: Some(sha256(b"old-b")),
            },
        ];
        let text = journal_text("1.0.0", &units);
        let (version, parsed) = parse_journal(&text).unwrap();
        assert_eq!(version, "1.0.0");
        assert_eq!(parsed, units);
        assert!(parse_journal("kachina-journal 0\nversion\t1\nhash\tsha256\n").is_err());
        assert!(parse_journal("kachina-journal 1\nversion\t1\nhash\tmd5\n").is_err());
    }

    #[test]
    fn commit_moves_new_files_and_deletes() {
        let root = test_root("commit");
        let install = root.join("install");
        let staging = root.join("install.kachina-staged");
        std::fs::create_dir_all(&install).unwrap();
        std::fs::create_dir_all(staging.join(NEW_DIR)).unwrap();
        std::fs::create_dir_all(staging.join(OLD_DIR)).unwrap();
        std::fs::write(install.join("a.txt"), b"old").unwrap();
        std::fs::write(install.join("gone.txt"), b"gone").unwrap();
        std::fs::write(staging.join(NEW_DIR).join("a.txt"), b"new").unwrap();
        std::fs::write(staging.join(NEW_DIR).join("b.txt"), b"new-b").unwrap();

        let outcome = commit(CommitArgs {
            staging_root: staging.to_string_lossy().to_string(),
            install_dir: install.to_string_lossy().to_string(),
            version: "1.0.0".to_string(),
            deletes: vec!["gone.txt".to_string()],
        })
        .unwrap();

        assert!(!outcome.self_replaced);
        assert_eq!(std::fs::read(install.join("a.txt")).unwrap(), b"new");
        assert_eq!(std::fs::read(install.join("b.txt")).unwrap(), b"new-b");
        assert!(!install.join("gone.txt").exists());
        assert!(!staging.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn commit_rolls_back_when_later_unit_fails() {
        let root = test_root("rollback");
        let install = root.join("install");
        let staging = root.join("install.kachina-staged");
        std::fs::create_dir_all(&install).unwrap();
        std::fs::create_dir_all(staging.join(NEW_DIR)).unwrap();
        std::fs::create_dir_all(staging.join(OLD_DIR)).unwrap();
        std::fs::create_dir_all(install.join("blocked.txt")).unwrap();
        std::fs::write(install.join("a.txt"), b"old").unwrap();
        std::fs::write(staging.join(NEW_DIR).join("a.txt"), b"new").unwrap();
        std::fs::write(staging.join(NEW_DIR).join("blocked.txt"), b"new-blocked").unwrap();

        let units = vec![
            Unit::File(FileEntry {
                rel: "a.txt".to_string(),
                old: Some(sha256(b"old")),
                new: sha256(b"new"),
            }),
            Unit::File(FileEntry {
                rel: "blocked.txt".to_string(),
                old: None,
                new: sha256(b"new-blocked"),
            }),
        ];
        assert!(apply_units(&staging, &install, &units).is_err());
        assert_eq!(std::fs::read(install.join("a.txt")).unwrap(), b"old");
        assert!(staging.join(NEW_DIR).join("a.txt").is_file());
        assert!(install.join("blocked.txt").is_dir());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn recover_completes_journal_and_discards_staging() {
        let root = test_root("recover");
        let install = root.join("install");
        let staging = root.join("install.kachina-staged");
        std::fs::create_dir_all(&install).unwrap();
        std::fs::create_dir_all(staging.join(NEW_DIR)).unwrap();
        std::fs::create_dir_all(staging.join(OLD_DIR)).unwrap();
        std::fs::write(staging.join(NEW_DIR).join("a.txt"), b"new").unwrap();
        let units = vec![staged_file("a.txt", b"new")];
        std::fs::write(
            staging::journal_path(&staging),
            journal_text("1.0.0", &units),
        )
        .unwrap();

        let outcome = recover(CommitArgs {
            staging_root: staging.to_string_lossy().to_string(),
            install_dir: install.to_string_lossy().to_string(),
            version: "1.0.0".to_string(),
            deletes: Vec::new(),
        })
        .unwrap();

        assert!(outcome.recovered);
        assert_eq!(std::fs::read(install.join("a.txt")).unwrap(), b"new");
        assert!(!staging.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn recover_restores_old_file_when_new_file_is_missing() {
        let root = test_root("unrecoverable");
        let install = root.join("install");
        let staging = root.join("install.kachina-staged");
        std::fs::create_dir_all(&install).unwrap();
        std::fs::create_dir_all(staging.join(OLD_DIR)).unwrap();
        let old = b"old";
        std::fs::write(staging.join(OLD_DIR).join("a.txt"), old).unwrap();
        let units = vec![Unit::File(FileEntry {
            rel: "a.txt".to_string(),
            old: Some(sha256(old)),
            new: sha256(b"new"),
        })];
        std::fs::write(
            staging::journal_path(&staging),
            journal_text("1.0.0", &units),
        )
        .unwrap();

        let outcome = recover(CommitArgs {
            staging_root: staging.to_string_lossy().to_string(),
            install_dir: install.to_string_lossy().to_string(),
            version: "1.0.0".to_string(),
            deletes: Vec::new(),
        })
        .unwrap();

        assert!(!outcome.recovered);
        assert_eq!(std::fs::read(install.join("a.txt")).unwrap(), old);
        assert!(!staging.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn recover_discards_when_version_differs() {
        let root = test_root("version");
        let install = root.join("install");
        let staging = root.join("install.kachina-staged");
        std::fs::create_dir_all(&install).unwrap();
        std::fs::create_dir_all(staging.join(NEW_DIR)).unwrap();
        std::fs::write(staging.join(NEW_DIR).join("a.txt"), b"new").unwrap();
        let units = vec![staged_file("a.txt", b"new")];
        std::fs::write(
            staging::journal_path(&staging),
            journal_text("0.9.0", &units),
        )
        .unwrap();

        let outcome = recover(CommitArgs {
            staging_root: staging.to_string_lossy().to_string(),
            install_dir: install.to_string_lossy().to_string(),
            version: "1.0.0".to_string(),
            deletes: Vec::new(),
        })
        .unwrap();

        assert!(!outcome.recovered);
        assert!(!install.join("a.txt").exists());
        assert!(!staging.exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
