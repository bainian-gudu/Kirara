//! 取 Windows 版本号 `(major, minor, build)`。
//!
//! 原来用 `nt_version 0.1`（2020 年后再没更新）。这里用同一个 ntdll 入口自己实现：
//! `RtlGetNtVersionNumbers` 是 `GetVersionEx` 真正查询的那条路径，不受
//! 「进程清单没声明支持 Win10 就报 6.2」的兼容性改写影响。
//!
//! 只做这一件事，所以不引入任何新依赖，也不需要 windows crate 的额外 feature。

/// ntdll 的 `RtlGetNtVersionNumbers`：填 major / minor / build。
///
/// 注意 `build` 的高位带着未文档化的标志位（历史上是 `0xF0000000`），
/// 低 16 位才是真正的构建号，调用方要用 `build & 0xffff`。
#[link(name = "ntdll")]
unsafe extern "system" {
    fn RtlGetNtVersionNumbers(major: *mut u32, minor: *mut u32, build: *mut u32);
}

/// 返回 `(major, minor, build)`；`build` 已按低 16 位裁好。
pub fn get() -> (u32, u32, u32) {
    let (mut major, mut minor, mut build) = (0u32, 0u32, 0u32);
    // SAFETY: 三个指针都指向本栈帧里有效的 u32，调用期间不会被移动。
    unsafe { RtlGetNtVersionNumbers(&mut major, &mut minor, &mut build) };
    (major, minor, build & 0xffff)
}
