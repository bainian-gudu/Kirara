use anyhow::{Context, Result};
use std::{io::Read, path::Path};

const HASH_BUFFER_SIZE: usize = 1024 * 1024;
/// `FILE_FLAG_SEQUENTIAL_SCAN`：告诉 Windows 这条读流是顺序的，缓存管理器会预读、
/// 少留脏页。被哈希的文件动辄几百 MB，默认的随机访问启发式在这里是反效果。
const FILE_FLAG_SEQUENTIAL_SCAN: u32 = 0x0800_0000;

/// 对**已经打开**的流做哈希。
///
/// 与 `hash_file` 分开是为了能在 devcheck 的 logic 层用内存流断言摘要值 ——
/// 那里跑在 Linux 上，拿不到 `FILE_FLAG_SEQUENTIAL_SCAN` 这种 Windows 专有开关。
/// 签名与 `anyhow::Context` 都写成自包含形式：devcheck 的 logic 层按名字抽取
/// 单个函数，文件顶部的 `use` 不会跟着过去。
pub fn hash_reader<R: Read>(hash_algorithm: &str, mut reader: R) -> anyhow::Result<String> {
    use anyhow::Context as _;

    let mut buffer = vec![0u8; HASH_BUFFER_SIZE];
    if hash_algorithm == "md5" {
        let mut hasher = chksum_md5::MD5::new();
        loop {
            let read = reader.read(&mut buffer).context("READ_FILE_ERR")?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        Ok(hasher.digest().to_hex_lowercase())
    } else if hash_algorithm == "xxh" {
        use twox_hash::XxHash3_128;
        let mut hasher = XxHash3_128::new();
        loop {
            let read = reader.read(&mut buffer).context("READ_FILE_ERR")?;
            if read == 0 {
                break;
            }
            hasher.write(&buffer[..read]);
        }
        Ok(format!("{:x}", hasher.finish_128()))
    } else {
        Err(anyhow::anyhow!("NO_HASH_ALGO_ERR"))
    }
}

/// 读文件并算摘要。整条路径只有一次顺序读，md5 与 xxh 共用同一套循环。
pub fn hash_file(hash_algorithm: &str, path: &str) -> Result<String> {
    use std::os::windows::fs::OpenOptionsExt;

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(false)
        .custom_flags(FILE_FLAG_SEQUENTIAL_SCAN)
        .open(Path::new(path))
        .context("OPEN_TARGET_ERR")?;
    hash_reader(hash_algorithm, file)
}

pub async fn run_hash(hash_algorithm: &str, path: &str) -> Result<String> {
    let hash_algorithm = hash_algorithm.to_string();
    let path = path.to_string();
    // 文件 IO 与哈希都是同步阻塞的：丢到 blocking 线程池，别占住 async 工作线程。
    tokio::task::spawn_blocking(move || hash_file(&hash_algorithm, &path))
        .await
        .context("HASH_THREAD_ERR")?
        .context("HASH_COMPLETE_ERR")
}

// 这些用例跑在 Windows 上（CI 的 unit-test job，`cargo test --bin kachina-builder`）：
// 它们要的正是 Linux 上验不了的东西 —— 真实文件打开、只读文件、
// FILE_FLAG_SEQUENTIAL_SCAN 这条 Windows 专有路径。跨平台的摘要一致性在
// tools/devcheck 的 logic 层 [22] 组断言里。
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_FILE_ID: AtomicU64 = AtomicU64::new(0);

    fn temp_file(bytes: &[u8]) -> (std::path::PathBuf, String) {
        let dir = std::env::temp_dir().join(format!(
            "kachina-hash-{}-{}",
            std::process::id(),
            TEMP_FILE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sample.bin");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(bytes).unwrap();
        file.flush().unwrap();
        (dir, path.to_string_lossy().to_string())
    }

    fn cleanup(dir: std::path::PathBuf, path: &str) {
        // 只读文件在 Windows 上删不掉，先摘掉只读位
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_readonly(false);
        let _ = std::fs::set_permissions(path, perms);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn md5_matches_known_digest() {
        let (dir, path) = temp_file(b"hello");
        let hash = hash_file("md5", &path).unwrap();
        cleanup(dir, &path);
        assert_eq!(hash, "5d41402abc4b2a76b9719d911017c592");
    }

    #[test]
    fn hashes_read_only_file() {
        let (dir, path) = temp_file(b"readonly-hash");
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&path, perms).unwrap();
        let hash = hash_file("md5", &path).unwrap();
        let expected = chksum_md5::hash(b"readonly-hash").to_hex_lowercase();
        cleanup(dir, &path);
        assert_eq!(hash, expected);
    }

    #[test]
    fn rejects_unknown_algorithm() {
        let (dir, path) = temp_file(b"x");
        let err = hash_file("sha1", &path).is_err();
        cleanup(dir, &path);
        assert!(err);
    }
}
