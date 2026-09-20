use anyhow::Context;
use fmmap::tokio::{AsyncMmapFile, AsyncMmapFileExt, AsyncMmapFileReader};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::sync::OnceCell;
static MMAP_SELF: OnceCell<AsyncMmapFile> = OnceCell::const_new();

pub async fn mmap() -> &'static AsyncMmapFile {
    MMAP_SELF
        .get_or_init(|| async {
            let exe_path = {
                #[cfg(debug_assertions)]
                {
                    // 使用上一次发布构建
                    let exe_path = std::env::current_exe().unwrap();
                    // ../发布/${basename}
                    let exe_path = exe_path
                        .parent()
                        .ok_or("Failed to get parent dir".to_string())
                        .unwrap()
                        .parent()
                        .ok_or("Failed to get parent dir".to_string())
                        .unwrap()
                        .join("release")
                        .join("kachina-builder-bundle.exe");
                    let debug_path = exe_path
                        .parent()
                        .ok_or("Failed to get parent dir".to_string())
                        .unwrap()
                        .parent()
                        .ok_or("Failed to get parent dir".to_string())
                        .unwrap()
                        .join("debug")
                        .join("kachina-builder-bundle.exe");
                    if exe_path.exists() {
                        exe_path
                    } else if debug_path.exists() {
                        debug_path
                    } else {
                        // 回退到当前 exe
                        std::env::current_exe().unwrap()
                    }
                }
                #[cfg(not(debug_assertions))]
                {
                    std::env::current_exe().unwrap()
                }
            };
            AsyncMmapFile::open(exe_path).await.unwrap()
        })
        .await
}

async fn search_pattern_for_extract(file: &AsyncMmapFile) -> anyhow::Result<Vec<usize>> {
    let pattern = "!in\0".to_ascii_uppercase();
    let pattern: &[u8; 4] = pattern.as_bytes().try_into().unwrap();
    let mut reader = file.reader(0).context("MMAP_ERR")?;
    let mut buffer = [0u8; 4096];
    let mut offset: usize = 0;
    let mut founds = Vec::new();
    let mut read = 0;

    loop {
        // 将最后 4 个字节移到缓冲区开头
        if read > 4 {
            buffer[0] = buffer[read + 4 - 4];
            buffer[1] = buffer[read + 4 - 3];
            buffer[2] = buffer[read + 4 - 2];
            buffer[3] = buffer[read + 4 - 1];
        }
        read = reader.read(&mut buffer[4..]).await.context("MMAP_ERR")?;
        if read == 0 {
            break;
        }
        for i in 0..read + 4 - 1 {
            if buffer[i] == pattern[0] {
                if i + 3 > read + 3 {
                    continue;
                }
                if buffer[i + 1] == pattern[1]
                    && buffer[i + 2] == pattern[2]
                    && buffer[i + 3] == pattern[3]
                {
                    founds.push(offset + i - 4);
                }
            }
        }
        offset += read;
    }

    Ok(founds)
}
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Embedded {
    pub name: String,
    pub offset: usize,
    pub raw_offset: usize,
    pub size: usize,
}

/// 从一组 md5 / xxh 里挑一个用：与安装器其它地方一致，md5 优先。
pub fn preferred_file_hash<'a>(
    md5: &'a Option<String>,
    xxh: &'a Option<String>,
) -> Option<&'a String> {
    md5.as_ref().or(xxh.as_ref())
}

/// 读取器（`get_embedded`）只接受内置 `\0` 名称，以及 ASCII 字母/数字/`.`/`_`/`-`
/// 组成的名称。append 写入端必须用同一规则，否则数据进了包却永远读不出来
/// （`--list` / `--name` 都看不到它）。
///
/// 空名字额外拒绝：`chars().all(..)` 对空串恒为真，而读取端会因「名称长度为 0」
/// 直接跳过 —— 两边放行的话就是一个静默丢数据的口子。
pub fn is_embedded_name(name: &str) -> bool {
    matches!(
        name,
        "\0CONFIG" | "\0META" | "\0INDEX" | "\0IMAGE" | "\0THEME"
    ) || (!name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-'))
}

pub async fn get_embedded(file: &AsyncMmapFile) -> anyhow::Result<Vec<Embedded>> {
    let offsets = search_pattern_for_extract(file).await?;
    let mut entries = Vec::new();
    let mut last_offset: usize = 0;
    let file_len = file.len();
    for offset in offsets.iter() {
        if *offset < last_offset {
            // 处理内容包含头部的情况
            continue;
        }
        // 相关实现：TLV
        // 头部：!IN\0
        // 名称长度：2 字节，大端序
        // 名称：可变长度
        // 内容长度：4 字节，大端序
        // 内容：可变长度
        let mem_pos_name_length = *offset + 4;
        if mem_pos_name_length + 2 > file_len {
            continue;
        }
        let name_length =
            u16::from_be_bytes(file.slice(mem_pos_name_length, 2).try_into().unwrap()) as usize;
        // 名称长度是个上界保护：畸形包里这个字段可以是任意 u16
        if name_length == 0 || name_length > 512 {
            continue;
        }
        let mem_pos_name = mem_pos_name_length + 2;
        let mem_pos_content_length = mem_pos_name + name_length;
        if mem_pos_content_length + 4 > file_len {
            continue;
        }
        let name = file.slice(mem_pos_name, name_length);
        let Ok(name) = std::str::from_utf8(name) else {
            continue;
        };
        if !is_embedded_name(name) {
            continue;
        }
        let content_length =
            u32::from_be_bytes(file.slice(mem_pos_content_length, 4).try_into().unwrap()) as usize;
        let mem_pos_content = mem_pos_content_length + 4;
        if mem_pos_content.saturating_add(content_length) > file_len {
            continue;
        }
        entries.push(Embedded {
            name: name.to_string(),
            offset: mem_pos_content,
            size: content_length,
            raw_offset: *offset,
        });
        last_offset = mem_pos_content + content_length;
    }
    Ok(entries)
}

/// DOS 头里的 `e_lfanew` 指向 `PE\0\0`。真实链接器把它放在这个窗口内；
/// `.rdata` / zstd 流 / ico 里偶然出现的 `MZ\x90\x00` 几乎不可能同时满足。
const PE_LFANEW_MIN: usize = 0x40;
const PE_LFANEW_MAX: usize = 0x1000;

/// 打包产物是「kachina-builder 的字节 + kachina-installer 的字节」拼接而成。
/// 只有真正的 PE 映像起点才算数，最后一个是追加进去的安装器。
///
/// 上游早期版本按 `MZ\x90\x00` 四个字节扫：安装器体内（压缩数据、图标、资源段）
/// 一旦出现这串字节就会被当成映像起点，于是 rcedit 加载到半截文件直接失败。
pub fn pe_image_starts(bytes: &[u8]) -> Vec<usize> {
    let mut found = Vec::new();
    let mut i = 0;
    while i + 2 <= bytes.len() {
        if bytes[i] == b'M' && bytes[i + 1] == b'Z' && is_pe_at(bytes, i) {
            found.push(i);
        }
        i += 1;
    }
    found
}

fn is_pe_at(bytes: &[u8], offset: usize) -> bool {
    if offset.saturating_add(0x40) > bytes.len() {
        return false;
    }
    if bytes[offset] != b'M' || bytes[offset + 1] != b'Z' {
        return false;
    }
    let e_lfanew =
        u32::from_le_bytes(bytes[offset + 0x3C..offset + 0x40].try_into().unwrap()) as usize;
    if e_lfanew < PE_LFANEW_MIN || e_lfanew > PE_LFANEW_MAX {
        return false;
    }
    let pe = offset.saturating_add(e_lfanew);
    bytes.get(pe..pe + 4) == Some(&b"PE\0\0"[..])
}

pub async fn get_reader_for_bundle() -> Result<AsyncMmapFileReader<'static>, String> {
    let file = mmap().await;
    let bytes = file.slice(0, file.len());
    let headers = pe_image_starts(bytes);
    if headers.len() < 2 {
        println!("Found PE images: {headers:?}");
        return Err("Failed to find packed exe: ".to_string());
    }
    let exe_offset = headers[headers.len() - 1];
    let reader = file.reader(exe_offset).map_err(|e| e.to_string())?;
    Ok(reader)
}

#[cfg(test)]
mod tests {
    use super::{is_pe_at, pe_image_starts};

    /// 最小 PE 映像：DOS 头 + `e_lfanew` 指向的 `PE\0\0`。
    fn mini_pe(tag: u8) -> Vec<u8> {
        let e_lfanew = 0x80usize;
        let mut bytes = vec![0u8; 0x200];
        bytes[0] = b'M';
        bytes[1] = b'Z';
        bytes[2] = 0x90;
        bytes[3] = 0x00;
        bytes[0x3C..0x40].copy_from_slice(&(e_lfanew as u32).to_le_bytes());
        bytes[e_lfanew..e_lfanew + 4].copy_from_slice(b"PE\0\0");
        bytes[e_lfanew + 4] = tag;
        bytes
    }

    #[test]
    fn pe_at_requires_pe_signature() {
        let pe = mini_pe(1);
        assert!(is_pe_at(&pe, 0));
        let mut false_mz = vec![0x4D, 0x5A, 0x90, 0x00];
        false_mz.extend_from_slice(&[0u8; 60]);
        assert!(!is_pe_at(&false_mz, 0));
    }

    #[test]
    fn bundle_uses_last_real_pe_not_last_mz90() {
        let builder = mini_pe(1);
        let installer = mini_pe(2);
        let mut bundle = builder.clone();
        bundle.extend_from_slice(&installer);
        // 安装器体内再放一个 DOS 魔数：旧扫描会把它当成映像起点，rcedit 加载失败。
        let planted = builder.len() + 0x40;
        bundle[planted..planted + 4].copy_from_slice(&[0x4D, 0x5A, 0x90, 0x00]);
        assert_eq!(pe_image_starts(&bundle), vec![0, builder.len()]);
    }
}
