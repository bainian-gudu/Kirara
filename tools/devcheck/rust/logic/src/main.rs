//! Kirara 安全阀与纯逻辑的行为断言（devcheck 的 logic 层）。
//!
//! 被测代码是 `src/gen/extracted.rs` —— 由 `tools/devcheck/devcheck.ps1` 按名字从
//! `native/` 下抽取（清单见 `tools/devcheck/lib/Generate.ps1`）。这里只补两样东西：
//!
//! - 仓库模块路径的 shim：抽取出来的函数按真实路径互相调用（`crate::fs::staging`、
//!   `crate::session::plan`、`crate::utils::config_keys`），在最小 crate 里补上同名模块；
//! - `has_reparse_point` 的跨平台桩：真实实现在 Windows 上调 Win32 API。
//!
//! 其余全是仓库原码。断言失败 → 退出码 1。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

// ---- 仓库模块路径的 shim ----
pub mod fs {
    pub mod staging {
        pub use crate::is_safe_rel;
    }
}
pub mod session {
    pub mod plan {
        pub use crate::normalize_full;
    }
}
pub mod utils {
    pub mod config_keys {
        pub use crate::PROJECT_CONFIG_KEYS;
    }
}

include!("gen/extracted.rs");

static CHECKS: AtomicU32 = AtomicU32::new(0);
static FAILURES: AtomicU32 = AtomicU32::new(0);

/// 失败明细。断言的详细差异不跟断言一起打，而是攒起来在最后统一打印：
/// devcheck 只回显子进程输出的**尾部**（`Invoke-Native -Tail`），失败夹在 160 条
/// `ok` 中间时会被截掉 —— 真发生过一次：CI 上只有 1 条明细进了日志，另外 5 条
/// 只剩一个「6 条失败」的汇总。
static FAILURE_DETAILS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn record_failure(detail: String) {
    FAILURE_DETAILS.lock().expect("失败明细锁").push(detail);
}

fn check(cond: bool, label: &str) {
    CHECKS.fetch_add(1, Ordering::Relaxed);
    if cond {
        println!("  ok    {label}");
    } else {
        FAILURES.fetch_add(1, Ordering::Relaxed);
        record_failure(label.to_string());
        println!("  FAIL  {label}");
    }
}

fn eq<T: std::fmt::Debug + PartialEq>(actual: T, expected: T, label: &str) {
    CHECKS.fetch_add(1, Ordering::Relaxed);
    if actual == expected {
        println!("  ok    {label}");
    } else {
        FAILURES.fetch_add(1, Ordering::Relaxed);
        record_failure(format!("{label}\n        actual:   {actual:?}\n        expected: {expected:?}"));
        println!("  FAIL  {label}");
    }
}

fn eqs(actual: String, expected: &str, label: &str) {
    CHECKS.fetch_add(1, Ordering::Relaxed);
    if actual == expected {
        println!("  ok    {label}");
    } else {
        FAILURES.fetch_add(1, Ordering::Relaxed);
        record_failure(format!("{label}\n        actual:   {actual:?}\n        expected: {expected:?}"));
        println!("  FAIL  {label}");
    }
}

fn group(title: &str) {
    println!("\n== {title} ==");
}

// devcheck 用的自签证书（EC prime256v1，CN=devcheck.example）。期望值由 openssl 算出，
// 复现命令见 tools/devcheck/README.md 的「H3 固定值」一节。
const CERT_DER_HEX: &str = "3082018e30820133a00302010202145aaafab278e5232183d4957b2928acf8f1058922300a06082a8648ce3d040302301b3119301706035504030c10646576636865636b2e6578616d706c653020170d3236313030393131343330335a180f32313236303931353131343330335a301b3119301706035504030c10646576636865636b2e6578616d706c653059301306072a8648ce3d020106082a8648ce3d0301070342000464f823e1a003e66581b5e62c2f301ff3ec4b0c14d3e78cb9ee19ae6023aa8808e059da99230f44c72c57f86c83fb11b84b8cb81c9010ca19e0a2210a6c9de56fa3533051301d0603551d0e041604149d67c5a0ab0c37b7e6cca709c47469a9b1d0d985301f0603551d230418301680149d67c5a0ab0c37b7e6cca709c47469a9b1d0d985300f0603551d130101ff040530030101ff300a06082a8648ce3d0403020349003046022100a78fda78821b02f738213c0b830026b75d9090e38e1358b86326469614ad86a3022100cfb5d32d4154da9e9fad5170cdf873432a50c2bcf5e0ba10b8acd5923d89f5a4";
const CERT_SHA256_HEX: &str = "eb2d279d48b7c1f28c256a3f1f2cbf46454489fbcdadc3ae2798fcaad2ff1c59";
const SPKI_DER_HEX: &str = "3059301306072a8648ce3d020106082a8648ce3d0301070342000464f823e1a003e66581b5e62c2f301ff3ec4b0c14d3e78cb9ee19ae6023aa8808e059da99230f44c72c57f86c83fb11b84b8cb81c9010ca19e0a2210a6c9de56f";
const SPKI_SHA256_HEX: &str = "b53c26133259b82f2941fe4c2f5c98fdbcad2c38e2286a7b3ea0e0df1c3d4f3f";

fn hex_bytes(s: &str) -> Vec<u8> {
    hex::decode(s).expect("内嵌的测试证书 hex 必须可解码")
}

fn hex_of(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

fn hash32(hex_str: &str) -> [u8; 32] {
    hex_bytes(hex_str).try_into().expect("SHA-256 是 32 字节")
}

/// 用例里的路径都按 POSIX 写法给，再由它变成**本平台上真的绝对**的路径。
///
/// 不能直接用 `Path::new("/home/x")`：在 Windows 上那不是绝对路径（没有盘符 / UNC
/// 前缀），`is_absolute()` 相关的判定会整组失去意义 —— 而「必须是绝对路径」正是这些
/// 安全阀最关键的一条前提。补一个盘符后，同一组断言在两个平台上测的是同一件事。
fn abs(posix: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(format!("C:{}", posix.replace('/', "\\")))
    } else {
        PathBuf::from(posix)
    }
}

/// 路径转成 `/` 分隔的字符串：Windows 的 `to_string_lossy` 给的是 `\`，直接断言会
/// 变成平台相关的期望值。
fn posix(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn tail_str(path: &str) -> Option<String> {
    profile_relative_tail(&abs(path)).map(|p| posix(&p))
}

fn main() {
    // 临时目录要在改环境变量之前取好（下面会清掉 TEMP / TMP，Windows 上 temp_dir 依赖它们）。
    let scratch = std::env::temp_dir().join(format!("kirara-devcheck-logic-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).expect("建临时目录");

    // 让判定与开发机环境无关：只保留我们显式设置的变量（都用本平台的绝对路径，
    // 否则 Windows 上 `path_eq` / `strip_prefix` 这些按路径比较的判定会对不上）。
    std::env::set_var("USERPROFILE", abs("/home/devcheck"));
    std::env::set_var("SystemRoot", abs("/sysroot"));
    for name in [
        "SystemDrive",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        "ProgramData",
        "HOMEDRIVE",
        "HOMEPATH",
        "APPDATA",
        "LOCALAPPDATA",
        "PUBLIC",
        "TEMP",
        "TMP",
        "KCHECK_MYVAR",
    ] {
        std::env::remove_var(name);
    }

    group("路径越界判定 is_safe_rel / try_join_rel");
    check(is_safe_rel("a"), "单段文件名放行");
    check(is_safe_rel("a\\b"), "反斜杠相对路径放行");
    check(is_safe_rel("a/b"), "正斜杠相对路径放行");
    check(is_safe_rel(""), "空串 = 目录本身，放行");
    check(!is_safe_rel("."), "`.` 拒绝");
    check(!is_safe_rel(".."), "`..` 拒绝");
    check(!is_safe_rel("..\\a"), "开头 `..` 拒绝");
    check(!is_safe_rel("a\\..\\b"), "中间 `..` 拒绝");
    check(!is_safe_rel("\\abs"), "根路径拒绝");
    check(!is_safe_rel("C:evil"), "盘符相对路径拒绝");
    check(!is_safe_rel("a:b"), "含冒号拒绝");
    let base = abs("/base");
    eq(
        try_join_rel(&base, "a\\b").map(|p| posix(&p)),
        Some(format!("{}/a/b", posix(&base))),
        "try_join_rel 拼在 base 下",
    );
    eq(try_join_rel(&base, "..\\x"), None, "try_join_rel 拒绝越界");

    group("删目录安全阀 is_safe_delete_root");
    check(
        !is_safe_delete_root(Path::new("relative/dir")),
        "相对路径拒绝",
    );
    check(!is_safe_delete_root(&abs("/one")), "只有一段拒绝");
    check(!is_safe_delete_root(&abs("/a/../b")), "含 `..` 拒绝");
    check(
        !is_safe_delete_root(&abs("/home/REPARSE/App")),
        "重解析点拒绝",
    );
    check(
        !is_safe_delete_root(&abs("/home/devcheck")),
        "受保护位置本身（USERPROFILE）拒绝",
    );
    check(
        !is_safe_delete_root(&abs("/sysroot/app/data")),
        "SystemRoot 之下拒绝",
    );
    check(
        is_safe_delete_root(&abs("/home/devcheck/AppData/Local/App")),
        "用户数据目录放行",
    );
    check(
        is_safe_delete_root(&abs("/opt/Kirara/log")),
        "普通两级目录放行",
    );
    std::env::set_var("TEMP", abs("/tmpzone/sub"));
    check(
        !is_safe_delete_root(&abs("/tmpzone/sub")),
        "TEMP 指向的目录拒绝",
    );
    std::env::remove_var("TEMP");
    check(
        is_safe_delete_root(&abs("/tmpzone/sub")),
        "去掉 TEMP 后同一路径放行（证明上一条是被 TEMP 拦下的）",
    );

    group("快捷方式安全阀 is_safe_shortcut_path");
    check(
        is_safe_shortcut_path(&abs("/home/devcheck/Desktop/App.lnk")),
        ".lnk 放行",
    );
    check(
        is_safe_shortcut_path(&abs("/home/devcheck/Desktop/App.LNK")),
        "扩展名大小写不敏感",
    );
    check(
        !is_safe_shortcut_path(Path::new("App.lnk")),
        "相对路径拒绝",
    );
    check(
        !is_safe_shortcut_path(&abs("/home/devcheck/Desktop/App.txt")),
        "非 .lnk 拒绝",
    );
    check(
        !is_safe_shortcut_path(&abs("/home/devcheck/Desktop/REPARSE/App.lnk")),
        "重解析点拒绝",
    );
    check(
        !is_safe_shortcut_path(&abs("/home/devcheck/Desktop/../App.lnk")),
        "含 `..` 拒绝",
    );
    check(
        !is_safe_shortcut_path(&abs("/sysroot/Desktop/App.lnk")),
        "系统根内拒绝",
    );

    group("计划任务名安全阀 is_safe_task_name");
    check(is_safe_task_name("App", "App.AutoStart"), "产品名开头放行");
    check(is_safe_task_name("App", "App AutoStart"), "空格放行");
    check(is_safe_task_name("App", "App"), "等于产品名放行");
    check(
        is_safe_task_name("App", &format!("App{}", "x".repeat(97))),
        "恰好 100 字节放行",
    );
    check(!is_safe_task_name("App", ""), "空名拒绝");
    check(!is_safe_task_name("", "App.X"), "空产品名拒绝");
    check(!is_safe_task_name("App", "Other.Task"), "别的产品名拒绝");
    check(!is_safe_task_name("App", "*"), "通配符拒绝");
    check(!is_safe_task_name("App", "App\\Sub"), "目录形式拒绝");
    check(
        !is_safe_task_name("App", &format!("App{}", "x".repeat(98))),
        "101 字节拒绝",
    );
    check(!is_safe_task_name("App", "App$(x)"), "命令行注入字符拒绝");
    check(!is_safe_task_name("App", "App中"), "非 ASCII 拒绝");

    group("注册表清理安全阀 key_belongs_to_product / value_belongs_to_product");
    check(key_belongs_to_product(r"Software\App", "App"), "产品键放行");
    check(
        key_belongs_to_product(r"Software\pub\App\Sub", "App"),
        "发布者下的产品键放行",
    );
    check(key_belongs_to_product(r"software\app", "App"), "键名大小写不敏感");
    check(
        !key_belongs_to_product(r"Software\Microsoft\Windows\CurrentVersion\Run", "App"),
        "共享容器拒绝整棵删",
    );
    check(!key_belongs_to_product(r"Software", "App"), "hive 根拒绝");
    check(
        !key_belongs_to_product(r"Software\AppBackup", "App"),
        "段内包含不算整段命中",
    );
    check(!key_belongs_to_product(r"Software\App", ""), "空产品名拒绝键判定");
    check(value_belongs_to_product("App", "App"), "值名等于产品名放行");
    check(
        value_belongs_to_product("App AutoStart", "App"),
        "值名带产品名前缀放行",
    );
    check(
        !value_belongs_to_product("OtherApp", "App"),
        "别的产品的值拒绝",
    );
    check(!value_belongs_to_product("App", ""), "空产品名拒绝值判定");

    group("环境变量展开 expand_env_vars / expand_path_list");
    std::env::set_var("KCHECK_MYVAR", "/val");
    eqs(
        expand_env_vars("%KCHECK_MYVAR%\\x"),
        "/val\\x",
        "%VAR% 展开",
    );
    eqs(expand_env_vars("no percent"), "no percent", "无百分号原样");
    eqs(expand_env_vars("100% done"), "100% done", "落单的 % 原样");
    eqs(
        expand_env_vars("%KCHECK_NOPE%\\x"),
        "%KCHECK_NOPE%\\x",
        "未知变量原样保留",
    );
    eq(
        expand_path_list(&[
            " a ".to_string(),
            "A".to_string(),
            String::new(),
            "%KCHECK_MYVAR%".to_string(),
        ]),
        vec!["a".to_string(), "/val".to_string()],
        "去空白、丢空串、大小写不敏感去重",
    );

    group("跨用户重放 profile_relative_tail");
    eq(
        tail_str("/home/devcheck/AppData/Local/App"),
        Some("AppData/Local/App".to_string()),
        "AppData 下的产品目录取尾巴",
    );
    eq(
        tail_str("/home/devcheck/Documents/App"),
        Some("Documents/App".to_string()),
        "Documents 放行",
    );
    eq(
        tail_str("/home/devcheck/Desktop/App.lnk"),
        Some("Desktop/App.lnk".to_string()),
        "桌面快捷方式放行",
    );
    eq(
        tail_str("/home/devcheck/Desktop/App.txt"),
        None,
        "桌面上非 .lnk 拒绝",
    );
    eq(tail_str("/home/devcheck/AppData"), None, "只有一段拒绝");
    eq(
        tail_str("/home/devcheck/Downloads/x"),
        None,
        "不在每用户容器白名单里拒绝",
    );
    eq(tail_str("/other/AppData/x"), None, "不在 USERPROFILE 下拒绝");
    eq(
        tail_str("/home/devcheck/AppData/../x"),
        None,
        "含 `..` 拒绝",
    );

    group("%TEMP% 白名单 is_installer_temp_artifact");
    check(
        is_installer_temp_artifact("Kachina.MicrosoftEdgeWebView2Setup.exe"),
        "WebView2 引导器放行",
    );
    check(
        is_installer_temp_artifact("kachina-agreement.txt"),
        "协议查看临时文件放行",
    );
    check(
        is_installer_temp_artifact("KACHINA-AGREEMENT.TXT"),
        "大小写不敏感",
    );
    check(
        !is_installer_temp_artifact("KachinaInstaller.log"),
        "会话日志不删",
    );
    check(
        !is_installer_temp_artifact("kachina-agreement.txt.bak"),
        "后缀伪装拒绝",
    );

    group("安装计划 plan.rs");
    // 归一化只做「反斜杠转正斜杠 + 去前导斜杠 + 转小写」，不裁尾部分隔符。
    eqs(normalize_rel("A\\B/"), "a/b/", "反斜杠、前导斜杠与大小写归一");
    eqs(normalize_rel("/X/Y"), "x/y", "去掉前导斜杠");
    check(is_safe_member("a\\b"), "清单成员放行");
    check(!is_safe_member(""), "空清单成员拒绝");
    check(!is_safe_member("   "), "全空白清单成员拒绝");
    check(!is_safe_member("..\\a"), "清单成员含 `..` 拒绝");
    check(!is_safe_member("C:\\x"), "清单成员带盘符拒绝");
    check(!is_safe_member("a/./b"), "清单成员含 `.` 拒绝");
    eqs(normalize_full("C:\\App\\"), "c:\\app", "整路径归一");
    eqs(
        expand_template("${INSTALL_PATH}/User-${APP_NAME}", "C:\\App", "K"),
        "C:\\App/User-K",
        "模板展开",
    );
    eqs(join_install("C:\\App\\", "a/b"), "C:\\App\\a\\b", "拼安装路径");
    check(is_under("C:\\App\\a", "C:\\App"), "子路径在目录下");
    check(is_under("C:\\App", "C:\\App"), "同路径算在目录下");
    check(!is_under("C:\\App2\\a", "C:\\App"), "前缀相近但不同目录不算");
    eqs(
        strip_install_prefix("C:\\App\\a\\b", "C:\\App"),
        "a/b",
        "剥掉安装前缀",
    );
    eqs(strip_install_prefix("C:\\App", "C:\\App"), "", "同路径剥成空");
    eqs(
        strip_install_prefix("D:\\Other\\x", "C:\\App"),
        "D:/Other/x",
        "不在安装目录下时退化成去掉前导斜杠",
    );

    group("配置键：未识别键点名 pack.rs");
    let known = serde_json::json!({
        "source": "https://example.com/App.Install.exe",
        "appName": "App",
        "legacyExeNames": ["Old.exe"],
        "legacyUninstallNames": ["Old.uninst.exe"],
        "legacyProgramFilesPaths": ["OldApp"],
        "extraUninstallLnkNames": ["App.lnk"],
        "extraUninstallPath": [],
        "userDataPath": [],
        "ignoreFolderPath": [],
        "agreementFile": "USER_AGREEMENT.txt",
        "agreementFormat": "md",
        "agreementTitle": "用户协议",
    });
    eq(unknown_config_keys(&known), Vec::<String>::new(), "认识的键不报");
    eq(
        unknown_config_keys(&serde_json::json!({
            "appName": "App",
            "legacyExeName": "Old.exe",
            "shortcutName": "App",
        })),
        vec!["legacyExeName".to_string(), "shortcutName".to_string()],
        "拼错的键与上游已移除的键都报，按名字排序",
    );
    eq(
        unknown_config_keys(&serde_json::json!("App")),
        Vec::<String>::new(),
        "非对象配置不误报",
    );

    group("协议内联 resolve_agreement");
    let agreement_dir = scratch.join("agreement");
    std::fs::create_dir_all(&agreement_dir).expect("建协议目录");
    let config_path = agreement_dir.join("kirara.config.json");
    std::fs::write(
        agreement_dir.join("USER_AGREEMENT.txt"),
        "\u{feff}第一行\r\n第二行\r\n".as_bytes(),
    )
    .expect("写协议正文");
    let mut config = serde_json::json!({
        "appName": "App",
        "agreementFile": "USER_AGREEMENT.txt",
        "agreementFormat": "md",
        "agreementTitle": "用户协议",
    });
    resolve_agreement(&mut config, &config_path);
    check(config.get("agreementFile").is_none(), "源字段不再写进包内配置");
    eq(
        config["agreement"]["format"].as_str(),
        Some("markdown"),
        "md 归一成 markdown",
    );
    eq(
        config["agreement"]["content"].as_str(),
        Some("第一行\n第二行\n"),
        "去 BOM、CRLF 归一成 LF",
    );
    eq(
        config["agreement"]["title"].as_str(),
        Some("用户协议"),
        "标题原样保留",
    );
    let mut default_title = serde_json::json!({ "agreementFile": "USER_AGREEMENT.txt" });
    resolve_agreement(&mut default_title, &config_path);
    eq(
        default_title["agreement"]["title"].as_str(),
        Some("用户协议"),
        "缺省标题是「用户协议」",
    );
    eq(
        default_title["agreement"]["format"].as_str(),
        Some("text"),
        "缺省格式是 text",
    );
    let mut html = serde_json::json!({
        "agreementFile": "USER_AGREEMENT.txt",
        "agreementFormat": "html",
    });
    resolve_agreement(&mut html, &config_path);
    eq(html["agreement"]["format"].as_str(), Some("html"), "html 保留");
    let mut unknown_format = serde_json::json!({
        "agreementFile": "USER_AGREEMENT.txt",
        "agreementFormat": "HTML",
    });
    resolve_agreement(&mut unknown_format, &config_path);
    eq(
        unknown_format["agreement"]["format"].as_str(),
        Some("text"),
        "不认识的格式退化成纯文本",
    );
    let mut empty = serde_json::json!({ "agreementFile": "" });
    resolve_agreement(&mut empty, &config_path);
    check(empty.get("agreement").is_none(), "空 agreementFile 不内联");
    let mut missing = serde_json::json!({ "agreementFile": "NO_SUCH_FILE.txt" });
    resolve_agreement(&mut missing, &config_path);
    check(
        missing.get("agreement").is_none(),
        "读不到文件时不留 agreement",
    );
    check(
        missing.get("agreementFile").is_none(),
        "读不到文件时源字段仍被删掉",
    );
    let mut untouched = serde_json::json!({ "appName": "App" });
    resolve_agreement(&mut untouched, &config_path);
    eq(
        untouched,
        serde_json::json!({ "appName": "App" }),
        "没配协议时配置一个字节不动",
    );

    group("包体识别与嵌入名 builder/local.rs、embedded_name.rs");
    eq(pe_image_starts(&[]), Vec::<usize>::new(), "空 buffer 没有 PE 起点");
    let mut blob = pe_blob(0x40);
    eq(pe_image_starts(&blob), vec![0], "单个 PE 起点");
    let second = 0x100;
    blob.resize(second + 0x100, 0);
    write_pe(&mut blob, second);
    eq(
        pe_image_starts(&blob),
        vec![0, second],
        "拼接体里两个 PE 起点都找到",
    );
    check(!is_pe_at(&blob, second + 1), "非 MZ 处不算 PE");
    let bad_small = pe_blob(0x10);
    check(!is_pe_at(&bad_small, 0), "e_lfanew 太小拒绝");
    let bad_big = pe_blob(0x2000);
    check(!is_pe_at(&bad_big, 0), "e_lfanew 太大拒绝");
    check(!is_pe_at(&blob[..8], 0), "buffer 太短拒绝");
    eq(TLV_NAME_MAX, 65535usize, "TLV 名字上限是 u16");
    eq(INDEX_NAME_MAX, 255usize, "索引名字上限是 u8");
    check(is_internal_name("\0CONFIG"), "内部槽位名");
    check(!is_internal_name("CONFIG"), "去掉前缀就不是内部名");
    check(is_embedded_name("\0CONFIG"), "内部名是合法嵌入名");
    check(is_embedded_name("ok.txt"), "普通文件名合法");
    check(!is_embedded_name(""), "空名拒绝");
    check(!is_embedded_name("."), "`.` 拒绝");
    check(!is_embedded_name(".."), "`..` 拒绝");
    check(!is_embedded_name("a "), "结尾空格拒绝");
    check(!is_embedded_name("a."), "结尾句点拒绝");
    check(!is_embedded_name("a/b"), "路径分隔符拒绝");
    check(!is_embedded_name("a:b"), "冒号拒绝");
    check(!is_embedded_name("CON"), "设备名拒绝");
    check(!is_embedded_name("con.txt"), "带扩展名的设备名拒绝");
    check(!is_embedded_name("COM1"), "串口设备名拒绝");
    check(!is_embedded_name("LPT9.log"), "并口设备名拒绝");
    check(is_embedded_name("COMX"), "COM 开头但不是设备名放行");
    check(
        check_tlv_name(&"a".repeat(TLV_NAME_MAX)).is_ok(),
        "TLV 名字恰好到上限放行",
    );
    check(
        check_tlv_name(&"a".repeat(TLV_NAME_MAX + 1)).is_err(),
        "TLV 名字超一个字节拒绝",
    );
    check(
        check_index_name(&"a".repeat(INDEX_NAME_MAX)).is_ok(),
        "索引名字恰好到上限放行",
    );
    check(
        check_index_name(&"a".repeat(INDEX_NAME_MAX + 1)).is_err(),
        "索引名字超一个字节拒绝",
    );

    group("解包路径安全阀 builder/extract.rs");
    let out_root = abs("/out");
    eq(
        relative_under_root(&out_root, "a/b").map(|p| posix(&p)),
        Ok(format!("{}/a/b", posix(&out_root))),
        "普通相对路径拼在输出根下",
    );
    check(
        relative_under_root(&out_root, "../x").is_err(),
        "`..` 拒绝",
    );
    check(
        relative_under_root(&out_root, "/abs").is_err(),
        "绝对路径拒绝",
    );
    check(
        relative_under_root(&out_root, "").is_err(),
        "空路径拒绝",
    );
    // `Path::components()` 会把中间的 `.` 归一掉（只有开头的 `.` 会留下 CurDir
    // 而被拒绝），所以 `a/./b` 落在输出根内，属于放行而不是越界。
    eqs(
        posix(&relative_under_root(&out_root, "a/./b")
            .expect("中间的 `.` 由 components() 归一，不越界")),
        &format!("{}/a/b", posix(&out_root)),
        "中间的 `.` 归一后仍在根内",
    );
    check(
        relative_under_root(&out_root, "./a").is_err(),
        "开头的 `.` 拒绝",
    );
    eqs(sanitize_output_name(""), "_unnamed", "空名换成占位名");
    eqs(sanitize_output_name("a\0b"), "a_b", "内部槽位前缀换成下划线");

    group("MirrorChyan 条目名解码 mirrorc.rs");
    eqs(decode_entry_name(b"ok.txt"), "ok.txt", "ASCII 原样");
    eqs(
        decode_entry_name("中文.txt".as_bytes()),
        "中文.txt",
        "UTF-8 中文名不乱码",
    );
    eqs(
        decode_entry_name(b"\xff\xfe"),
        "\u{fffd}\u{fffd}",
        "非法字节按 lossy 替换",
    );
    eqs(decode_entry_name(b""), "", "空名");

    group("下载验签结论 secure_temp.rs");
    check(
        is_trusted_microsoft_signature(
            "Valid",
            "CN=Microsoft Corporation, O=Microsoft Corporation, L=Redmond, S=Washington, C=US",
        ),
        "Valid + 微软 Subject 放行",
    );
    check(
        is_trusted_microsoft_signature("valid", "o=microsoft corporation"),
        "状态与 Subject 都大小写不敏感",
    );
    check(
        !is_trusted_microsoft_signature("Valid", "CN=Not Microsoft Corporation Ltd"),
        "子串冒名拒绝",
    );
    check(
        !is_trusted_microsoft_signature("Valid", "CN=Microsoft Corporation Ltd"),
        "不是完整段落的写法拒绝",
    );
    check(
        !is_trusted_microsoft_signature("UnknownError", "CN=Microsoft Corporation"),
        "状态不是 Valid 拒绝",
    );
    check(
        is_trusted_microsoft_signature("Valid", "CN=evil, O=Microsoft Corporation"),
        "命中任意一段即可（CA/B 下 CN 与 O 必须一致）",
    );

    group("H3 证书固定 capabilities/h3.rs");
    let cert_der = hex_bytes(CERT_DER_HEX);
    eqs(
        hex_of(extract_spki_der(&cert_der).expect("证书里能定位到 SPKI")),
        SPKI_DER_HEX,
        "SPKI 的 DER 与 openssl 输出逐字节一致",
    );
    eqs(
        hex_of(&compute_spki_hash(&cert_der).expect("能算 SPKI 哈希")),
        SPKI_SHA256_HEX,
        "SPKI 哈希与 openssl 一致",
    );
    eqs(
        hex_of(&compute_cert_hash(&cert_der)),
        CERT_SHA256_HEX,
        "整证书哈希与 openssl 一致",
    );
    eqs(
        hex_of(&sha256(b"")),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        "sha256 与标准值一致",
    );
    let url_spki = url::Url::parse(&format!("http3://h/p#spki={SPKI_SHA256_HEX}")).expect("URL");
    eq(
        parse_pin_from_fragment(&url_spki),
        Some(PinConfig {
            target: PinTarget::Spki(hash32(SPKI_SHA256_HEX)),
            mode: PinningMode::Force,
        }),
        "spki= 解析成 Spki + 默认 Force",
    );
    let url_cert_add =
        url::Url::parse(&format!("http3://h/p#cert={CERT_SHA256_HEX}&pinning_mode=add"))
            .expect("URL");
    eq(
        parse_pin_from_fragment(&url_cert_add),
        Some(PinConfig {
            target: PinTarget::Cert(hash32(CERT_SHA256_HEX)),
            mode: PinningMode::Add,
        }),
        "cert= 优先且 pinning_mode=add 生效",
    );
    let url_both = url::Url::parse(&format!(
        "http3://h/p#spki={SPKI_SHA256_HEX}&cert={CERT_SHA256_HEX}"
    ))
    .expect("URL");
    eq(
        parse_pin_from_fragment(&url_both).map(|c| c.target),
        Some(PinTarget::Cert(hash32(CERT_SHA256_HEX))),
        "cert 优先于 spki",
    );
    let url_weird = url::Url::parse(&format!(
        "http3://h/p#spki={SPKI_SHA256_HEX}&pinning_mode=weird"
    ))
    .expect("URL");
    eq(
        parse_pin_from_fragment(&url_weird).map(|c| c.mode),
        Some(PinningMode::Force),
        "不认识的 pinning_mode 退回 Force",
    );
    let url_short = url::Url::parse("http3://h/p#spki=abcd").expect("URL");
    eq(parse_pin_from_fragment(&url_short), None, "长度不是 64 的 hex 拒绝");
    let url_no_fragment = url::Url::parse("http3://h/p").expect("URL");
    eq(
        parse_pin_from_fragment(&url_no_fragment),
        None,
        "没有 fragment 时没有固定配置",
    );

    group("配置键清单 config_keys.rs");
    for key in [
        "legacyExeNames",
        "legacyUninstallNames",
        "legacyProgramFilesPaths",
        "extraUninstallLnkNames",
        "extraUninstallRegistry",
        "extraUninstallScheduledTasks",
        "userDataPath",
        "ignoreFolderPath",
        "agreement",
    ] {
        check(
            PROJECT_CONFIG_KEYS.contains(&key),
            &format!("清单里有 {key}"),
        );
    }
    check(
        !PROJECT_CONFIG_KEYS.contains(&"shortcutName"),
        "上游已移除的 shortcutName 不在清单里",
    );
    let mut sorted = PROJECT_CONFIG_KEYS.to_vec();
    sorted.sort_unstable();
    let mut deduped = sorted.clone();
    deduped.dedup();
    eq(
        PROJECT_CONFIG_KEYS.to_vec(),
        sorted.clone(),
        "清单按名字排序，便于人工比对",
    );
    eq(sorted, deduped, "清单没有重复项");

    let _ = std::fs::remove_dir_all(&scratch);

    let checks = CHECKS.load(Ordering::Relaxed);
    let failures = FAILURES.load(Ordering::Relaxed);
    println!("\n──── logic：{checks} 条断言，{failures} 条失败 ────");
    if failures > 0 {
        // 明细放在最末尾：devcheck 只回显尾部输出，夹在中间会被截掉。
        println!("\n──── 失败明细（{failures} 条）────");
        for detail in FAILURE_DETAILS.lock().expect("失败明细锁").iter() {
            println!("  ✗ {detail}");
        }
        std::process::exit(1);
    }
}

/// 造一个最小 PE 头：MZ + e_lfanew + "PE\0\0"。
fn pe_blob(lfanew: usize) -> Vec<u8> {
    let mut blob = vec![0u8; 0x40 + 4];
    blob[0] = b'M';
    blob[1] = b'Z';
    blob[0x3C..0x40].copy_from_slice(&(lfanew as u32).to_le_bytes());
    if lfanew + 4 <= blob.len() {
        blob[lfanew..lfanew + 4].copy_from_slice(b"PE\0\0");
    }
    blob
}

/// 在已有 buffer 的 offset 处写入一个最小 PE 头。
fn write_pe(blob: &mut [u8], offset: usize) {
    blob[offset] = b'M';
    blob[offset + 1] = b'Z';
    blob[offset + 0x3C..offset + 0x40].copy_from_slice(&0x40u32.to_le_bytes());
    blob[offset + 0x40..offset + 0x44].copy_from_slice(b"PE\0\0");
}
