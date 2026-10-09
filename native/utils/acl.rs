use windows::{
    core::w,
    Win32::Security::{
        Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION},
        PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
    },
};

/// 提权管道（`\\.\pipe\Kachina-Elevate-<uuid>`）的安全描述符。
///
/// 上游给的是 `D:(A;;GA;;;AC)(A;;GA;;;RC)(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;BU)S:(ML;;NW;;;LW)`：
/// `BU`（所有本地用户）、`AC`（所有 AppContainer）、`RC`（远程桌面用户）都是 GA，
/// 完整性级别还压到 `LW`（Low）。管道服务端由中等完整性的 UI 进程创建、提权子进程作为
/// 客户端连上来，中间那段（UAC 弹窗期间，可能几十秒）管道空着等人 —— 任何本地进程都能
/// 枚举到这个名字并抢先连上冒充 worker：收下 UI 发来的操作与文件流、回一份伪造的结果
/// （界面以为装/卸成功），真 worker 随后 `ERROR_PIPE_BUSY` 重试失败。
///
/// 收紧成「只有创建者本人 + SYSTEM + 管理员」，完整性级别提到 Medium：
/// - `CO`（Creator Owner）在创建时解析成创建者的 SID —— UI 与提权子进程是同一个用户，
///   正常流程不受影响，也不用动态查 SID；
/// - `ML;;NW;;;ME` 让低完整性 / AppContainer 进程写不进来（no-write-up），
///   中等完整性的 UI 与高完整性的子进程照常；
/// - 去掉 `BU` / `AC` / `RC`：别的本地用户、沙箱进程、远程会话都没有理由碰这条管道。
///
/// `reject_remote_clients(true)` 与 `first_pipe_instance(true)` 上游已经开着，前者挡远程、
/// 后者挡同名管道抢注（名字里还有 UUID）。
pub fn create_security_attributes() -> SECURITY_ATTRIBUTES {
    let mut security_descriptor = PSECURITY_DESCRIPTOR::default();
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            w!("D:(A;;GA;;;CO)(A;;GA;;;SY)(A;;GA;;;BA)S:(ML;;NW;;;ME)"),
            SDDL_REVISION,
            &mut security_descriptor,
            None,
        )
        .unwrap();

        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: security_descriptor.0,
            bInheritHandle: false.into(),
        }
    }
}
