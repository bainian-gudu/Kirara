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

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    if !html.is_file() {
        // `cargo test` / `cargo check` 可以在没有前端产物的干净检出上运行。
        // 真正的安装器构建会先执行 `pnpm build:frontend`；这里只嵌入一个空资源表。
        println!("cargo:warning=dist/index.html not found; native host will not embed UI assets");
        fs::write(
            out_dir.join("ui_assets.rs"),
            "pub fn get(_path: &str) -> Option<(&'static [u8], &'static str)> { None }\n",
        )
        .unwrap();
        return;
    }

    let zst_path = out_dir.join("index.html.zst");
    let data =
        fs::read(&html).unwrap_or_else(|e| panic!("read {} for native host: {e}", html.display()));
    let compressed = zstd::encode_all(data.as_slice(), 22).expect("zstd frontend");
    fs::write(&zst_path, compressed).expect("write index.html.zst");

    let abs = zst_path
        .canonicalize()
        .unwrap_or(zst_path)
        .to_string_lossy()
        .replace('\\', "/");
    let code = format!(
        "pub fn get(path: &str) -> Option<(&'static [u8], &'static str)> {{\n    match path {{\n        \"index.html\" => Some((include_bytes!(r\"{abs}\"), \"text/html; charset=utf-8\")),\n        _ => None,\n    }}\n}}\n"
    );
    fs::write(out_dir.join("ui_assets.rs"), code).unwrap();
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
