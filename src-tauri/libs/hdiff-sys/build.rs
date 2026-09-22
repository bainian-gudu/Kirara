fn main() {
    cc::Build::new()
        .cpp(true)
        // 与 src-tauri/.cargo/config.toml 的 crt-static 以及 rcedit-sys 保持一致。
        // 否则 MSVC 链接阶段会报 RuntimeLibrary MD/MT 不匹配（LNK2038）。
        .static_crt(true)
        .cargo_output(true)
        // MSVC 默认按本地代码页读取源文件；项目注释使用 UTF-8，显式指定源文件编码。
        .flag_if_supported("/utf-8")
        .file("HDiff/diff.cpp")
        .file("HDiff/match_block.cpp")
        .file("HDiff/private_diff/bytes_rle.cpp")
        .file("HDiff/private_diff/compress_detect.cpp")
        .file("HDiff/private_diff/suffix_string.cpp")
        .file("HDiff/private_diff/limit_mem_diff/adler_roll.c")
        .file("HDiff/private_diff/limit_mem_diff/digest_matcher.cpp")
        .file("HDiff/private_diff/limit_mem_diff/stream_serialize.cpp")
        .file("HDiff/private_diff/libdivsufsort/divsufsort.cpp")
        .file("HDiff/private_diff/libdivsufsort/divsufsort64.cpp")
        .file("libParallel/parallel_channel.cpp")
        .file("libParallel/parallel_import.cpp")
        .file("../hpatch-sys/HPatch/patch.c")
        .compile("hdiff");
}
