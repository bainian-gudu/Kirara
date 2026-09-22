fn main() {
    cc::Build::new()
        // 与 src-tauri/.cargo/config.toml 的 crt-static 以及 rcedit-sys 保持一致。
        // 否则 MSVC 链接阶段会报 RuntimeLibrary MD/MT 不匹配（LNK2038）。
        .static_crt(true)
        // MSVC 默认按本地代码页读取源文件；项目注释使用 UTF-8，显式指定源文件编码。
        .flag_if_supported("/utf-8")
        .file("HPatch/patch.c")
        .compile("hpatch");
}
