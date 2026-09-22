use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    embed_frontend(&manifest);
    embed_windows_resources(&manifest);
}

fn embed_frontend(manifest: &Path) {
    let html = manifest.join("../dist/index.html");
    println!("cargo:rerun-if-changed={}", html.display());
    let locales = manifest.join("locales");
    println!("cargo:rerun-if-changed={}", locales.display());

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let mut arms = Vec::new();

    if html.is_file() {
        let zst_path = out_dir.join("index.html.zst");
        let data = fs::read(&html)
            .unwrap_or_else(|e| panic!("read {} for native host: {e}", html.display()));
        let compressed = zstd::encode_all(data.as_slice(), 22).expect("zstd frontend");
        fs::write(&zst_path, compressed).expect("write index.html.zst");
        arms.push(asset_arm(&zst_path, "index.html", "text/html; charset=utf-8"));
    } else {
        // `cargo test` / `cargo check` 可以在没有前端产物的干净检出上运行。
        // 真正的安装器构建会先执行 `pnpm build:frontend`；这里只跳过这一条资源。
        println!("cargo:warning=dist/index.html not found; native host will not embed UI assets");
    }

    // 文案表：`locales/*.tsv` 按列合并成一张宽表，原生与前端渲染器读同一份。
    let i18n_zst = out_dir.join("i18n.tsv.zst");
    merge_locales(&locales, &i18n_zst);
    arms.push(asset_arm(
        &i18n_zst,
        "i18n.tsv",
        "text/tab-separated-values; charset=utf-8",
    ));

    let mut code = String::from(
        "pub fn get(path: &str) -> Option<(&'static [u8], &'static str)> {\n    match path {\n",
    );
    for arm in &arms {
        code.push_str(arm);
        code.push('\n');
    }
    code.push_str("        _ => None,\n    }\n}\n");
    fs::write(out_dir.join("ui_assets.rs"), code).unwrap();
}

fn asset_arm(zst_path: &Path, name: &str, mime: &str) -> String {
    let abs = zst_path
        .canonicalize()
        .unwrap_or_else(|_| zst_path.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/");
    format!("        \"{name}\" => Some((include_bytes!(r\"{abs}\"), \"{mime}\")),")
}

/// 把 `locales/<lang>.tsv`（`KEY\t文案`）合并成 `KEY\tlang1\tlang2…` 的宽表。
fn merge_locales(locales: &Path, dst_zst: &Path) {
    let mut langs: Vec<(String, BTreeMap<String, String>)> = Vec::new();
    if locales.is_dir() {
        let mut files: Vec<PathBuf> = fs::read_dir(locales)
            .expect("read locales/")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("tsv"))
            .collect();
        files.sort();
        for f in files {
            println!("cargo:rerun-if-changed={}", f.display());
            let lang = f
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let text =
                fs::read_to_string(&f).unwrap_or_else(|e| panic!("read {}: {e}", f.display()));
            let mut map = BTreeMap::new();
            for line in text.lines() {
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let mut parts = line.splitn(2, '\t');
                let key = parts.next().unwrap_or("");
                if key.is_empty() || key == "KEY" {
                    continue;
                }
                map.insert(key.to_string(), parts.next().unwrap_or("").to_string());
            }
            langs.push((lang, map));
        }
    }
    let mut keys = BTreeSet::new();
    for (_, map) in &langs {
        keys.extend(map.keys().cloned());
    }
    let mut wide = String::from("KEY");
    for (lang, _) in &langs {
        wide.push('\t');
        wide.push_str(lang);
    }
    wide.push('\n');
    for key in keys {
        wide.push_str(&key);
        for (_, map) in &langs {
            wide.push('\t');
            if let Some(v) = map.get(&key) {
                wide.push_str(v);
            }
        }
        wide.push('\n');
    }
    let compressed = zstd::encode_all(wide.as_bytes(), 22).expect("zstd i18n");
    fs::write(dst_zst, compressed).expect("write i18n.tsv.zst");
}

fn embed_windows_resources(manifest: &Path) {
    if env::var("CARGO_CFG_TARGET_OS").ok().as_deref() != Some("windows")
        || env::var("CARGO_CFG_TARGET_ENV").ok().as_deref() != Some("msvc")
    {
        return;
    }

    let rc = manifest.join("resources/app.rc");
    let manifest_file = manifest.join("resources/app.manifest");
    let icon = manifest.join("icons/icon.ico");
    println!("cargo:rerun-if-changed={}", rc.display());
    println!("cargo:rerun-if-changed={}", manifest_file.display());
    println!("cargo:rerun-if-changed={}", icon.display());

    // 非 Windows 主机上的 cross-check 没有资源编译器；CI 的 Windows job 会走完整嵌入。
    // embed-resource 既认 `rc.exe`，也认 `llvm-rc` 与 `RC` / `RC_<target>` 环境变量。
    if !has_resource_compiler() {
        println!("cargo:warning=rc.exe not found; skipping manifest/icon embedding");
        return;
    }

    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg=/MANIFESTINPUT:{}",
        manifest_file.display()
    );
    embed_resource::compile_for(
        &rc,
        ["kachina-installer", "kachina-builder"],
        embed_resource::NONE,
    )
    .manifest_optional()
    .expect("embed default exe icon");
}

fn has_resource_compiler() -> bool {
    for key in ["RC", "RC_x86_64_pc_windows_msvc"] {
        if env::var_os(key).is_some_and(|value| !value.is_empty()) {
            return true;
        }
    }
    std::process::Command::new("rc.exe")
        .arg("/?")
        .output()
        .is_ok()
        || std::process::Command::new("llvm-rc")
            .arg("-V")
            .output()
            .is_ok()
}
