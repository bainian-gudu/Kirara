//! kachina 卸载/打包逻辑的行为断言（devcheck 第 2 层）。
//!
//! 被测代码是 `src/gen/extracted.rs` —— 由 `tools/devcheck/devcheck.ps1` 按名字从
//! `src-tauri/src/` 下的 `installer/uninstall.rs`、`builder/pack.rs`、
//! `utils/secure_temp.rs` 原样抽取（清单见 `tools/devcheck/lib/Generate.ps1`）。
//! 这里只 mock 两样东西：
//!
//! - `windows_registry`：记录调用，验证「删了什么、没删什么」
//! - `has_reparse_point` / `is_under_system_root`：Windows 专有 API，换成按路径名触发的桩
//!
//! 其余全是上游/本项目的真实代码。断言失败 → 进程退出码 1。

use std::io::Read;
use std::path::{Path, PathBuf};

use std::sync::Mutex;

// ---------- windows-registry 0.5.3 的最小桩 ----------
pub mod windows_registry {
        pub struct Key {
        pub name: &'static str,
    }
    impl Key {
        pub const fn hive(name: &'static str) -> Key {
            Key { name }
        }
        pub fn options(&self) -> OpenOptions<'_> {
            OpenOptions { key: self }
        }
        pub fn open(&self, path: &str) -> Result<Key, Box<dyn std::error::Error>> {
            super::log(&format!("open {}\\{}", self.name, path));
            Ok(Key {
                name: Box::leak(format!("{}\\{}", self.name, path).into_boxed_str()),
            })
        }
        pub fn remove_tree(&self, path: &str) -> Result<(), Box<dyn std::error::Error>> {
            super::log(&format!("TREE {}\\{}", self.name, path));
            Ok(())
        }
        pub fn remove_value(&self, name: &str) -> Result<(), Box<dyn std::error::Error>> {
            super::log(&format!("VALUE {}\\{}", self.name, name));
            Ok(())
        }
        /// 只认 `ProfileImagePath`：返回 `<KCHECK_MOCK_PROFILE_BASE>\<SID>`。
        /// 用绝对路径（而不是 %SystemDrive%\Users\xxx）是为了在 Linux 上也能跑。
        pub fn get_string(&self, name: &str) -> Result<String, Box<dyn std::error::Error>> {
            super::log(&format!("GET {} {name}", self.name));
            if name != "ProfileImagePath" {
                return Err("no such value".into());
            }
            let base = std::env::var("KCHECK_MOCK_PROFILE_BASE").unwrap_or_else(|_| "/kcheck-none".to_string());
            let sid = self.name.rsplit('\\').next().unwrap_or("sid");
            Ok(format!("{base}/{sid}"))
        }
        pub fn keys(&self) -> Result<KeyIterator, Box<dyn std::error::Error>> {
            Ok(KeyIterator {
                items: vec![
                    "S-1-5-18".to_string(),
                    "S-1-5-21-111-222-333-1001".to_string(),
                    "S-1-5-21-111-222-333-1001_Classes".to_string(),
                    ".DEFAULT".to_string(),
                ],
                i: 0,
            })
        }
    }
    pub struct OpenOptions<'a> {
        key: &'a Key,
    }
    impl<'a> OpenOptions<'a> {
        pub fn read(self) -> Self {
            self
        }
        pub fn write(self) -> Self {
            self
        }
        pub fn open(self, path: &str) -> Result<Key, Box<dyn std::error::Error>> {
            self.key.open(path)
        }
    }
    pub struct KeyIterator {
        items: Vec<String>,
        i: usize,
    }
    impl Iterator for KeyIterator {
        type Item = String;
        fn next(&mut self) -> Option<String> {
            if self.i < self.items.len() {
                let v = self.items[self.i].clone();
                self.i += 1;
                Some(v)
            } else {
                None
            }
        }
    }
    pub static CURRENT_USER: &Key = &Key::hive("HKCU");
    pub static LOCAL_MACHINE: &Key = &Key::hive("HKLM");
    pub static CLASSES_ROOT: &Key = &Key::hive("HKCR");
    pub static USERS: &Key = &Key::hive("HKU");
}

static CALLS: Mutex<Vec<String>> = Mutex::new(Vec::new());
fn log(s: &str) {
    CALLS.lock().unwrap().push(s.to_string());
}
fn take_calls() -> Vec<String> {
    std::mem::take(&mut *CALLS.lock().unwrap())
}

// ---------- 从源码原样抽取的被测代码 ----------
include!("gen/extracted.rs");

// ---------- 用例 ----------
use std::sync::atomic::{AtomicU32, Ordering};
static PASS: AtomicU32 = AtomicU32::new(0);
static FAIL: AtomicU32 = AtomicU32::new(0);
fn check(name: &str, ok: bool, detail: impl AsRef<str>) {
    if ok {
        PASS.fetch_add(1, Ordering::Relaxed);
        println!("  ok   {name}");
    } else {
        FAIL.fetch_add(1, Ordering::Relaxed);
        println!("  FAIL {name} :: {}", detail.as_ref());
    }
}

fn reg_target_cases() {
    println!("[1] 注册表安全阀 is_safe_registry_target");
    let cases: Vec<(&str, Option<&str>, bool)> = vec![
        // 正常：删 Run 下的自启动值
        (r"Software\Microsoft\Windows\CurrentVersion\Run", Some("App"), true),
        // 深度不够
        ("Software", Some("App"), false),
        (r"Software\App", None, false),
        // 共享容器一律不许整棵删
        (r"Software\Microsoft\Windows\CurrentVersion\Run", None, false),
        (r"Software\Microsoft\Windows\CurrentVersion\RunOnce", None, false),
        (r"Software\Microsoft\Windows\CurrentVersion\Uninstall", None, false),
        (r"Software\Microsoft\Windows\CurrentVersion\Policies", None, false),
        (r"Software\Microsoft\Windows\CurrentVersion\Explorer", None, false),
        (r"Software\Classes", None, false),
        // 单个 ARP 项 / 产品自己的键：允许
        (r"Software\Microsoft\Windows\CurrentVersion\Uninstall\MyApp", None, true),
        (r"Software\MyCompany\MyApp", None, true),
    ];
    for (k, v, want) in cases {
        let got = is_safe_registry_target(k, v);
        check(
            &format!("{k} value={v:?} => {want}"),
            got == want,
            format!("got {got}"),
        );
    }
}

fn clean_registry_cases() {
    println!("[2] clean_extra_registry 实际调用（含 value 为空的回归）");
    let items = vec![
        // 正常项：HKCU + 每个已加载 SID
        RegistryCleanupItem {
            hive: "HKCU".into(),
            key: r"Software\Microsoft\Windows\CurrentVersion\Run".into(),
            value: Some("GenshinFpsUnlocker".into()),
        },
        // value 为空字符串：必须整条跳过，绝不能变成 remove_tree
        RegistryCleanupItem {
            hive: "HKCU".into(),
            key: r"Software\Microsoft\Windows\CurrentVersion\Run".into(),
            value: Some("   ".into()),
        },
        // 不安全的整棵删除：必须跳过
        RegistryCleanupItem {
            hive: "HKLM".into(),
            key: r"Software\Microsoft\Windows\CurrentVersion\Run".into(),
            value: None,
        },
        // 未知根键：跳过
        RegistryCleanupItem {
            hive: "HKPD".into(),
            key: r"Software\Foo\Bar".into(),
            value: None,
        },
    ];
    let _ = take_calls();
    clean_extra_registry(&items);
    let calls = take_calls();
    let value_calls = calls.iter().filter(|c| c.starts_with("VALUE ")).count();
    let tree_calls = calls.iter().filter(|c| c.starts_with("TREE ")).count();
    check(
        "HKCU 自启动值：主 hive + 1 个真实 SID = 2 次删值",
        value_calls == 2,
        format!("{calls:?}"),
    );
    check(
        "全程没有任何 remove_tree（空 value / 共享容器都被拦）",
        tree_calls == 0,
        format!("{calls:?}"),
    );
    check(
        "跳过 S-1-5-18 / .DEFAULT / *_Classes",
        !calls.iter().any(|c| c.contains("S-1-5-18")
            || c.contains(".DEFAULT")
            || c.contains("_Classes")),
        format!("{calls:?}"),
    );
}

fn shortcut_cases() {
    println!("[3] 快捷方式安全阀 is_safe_shortcut_target");
    let tmp: PathBuf = std::env::temp_dir().join("kcheck-lnk");
    let _ = std::fs::remove_dir_all(&tmp);
    let programs = tmp.join("Start Menu").join("Programs");
    std::fs::create_dir_all(programs.join("GenshinFpsUnlocker")).unwrap();
    std::fs::create_dir_all(programs.join("OtherApp")).unwrap();
    std::fs::create_dir_all(tmp.join("Desktop")).unwrap();
    std::fs::create_dir_all(tmp.join("Downloads")).unwrap();
    std::fs::write(tmp.join("Desktop").join("原神帧率解锁.lnk"), b"x").unwrap();
    std::fs::write(tmp.join("Desktop").join("evil.exe"), b"x").unwrap();
    std::fs::write(tmp.join("Downloads").join("原神帧率解锁.lnk"), b"x").unwrap();
    std::fs::write(
        programs.join("GenshinFpsUnlocker").join("原神帧率解锁.lnk"),
        b"x",
    )
    .unwrap();

    let allowed = vec!["GenshinFpsUnlocker".to_string()];
    let p = |v: &Path| v.to_string_lossy().to_string();
    let cases: Vec<(String, bool)> = vec![
        (p(&tmp.join("Desktop").join("原神帧率解锁.lnk")), true),
        (
            p(&programs.join("GenshinFpsUnlocker").join("原神帧率解锁.lnk")),
            true,
        ),
        (p(&programs.join("GenshinFpsUnlocker")), true),
        // 别人的开始菜单程序目录
        (p(&programs.join("OtherApp")), false),
        (p(&programs.join("OtherApp").join("x.lnk")), false),
        // 非 Desktop / 非 Programs 下的 .lnk
        (p(&tmp.join("Downloads").join("原神帧率解锁.lnk")), false),
        // 扩展名不对
        (p(&tmp.join("Desktop").join("evil.exe")), false),
        (p(&tmp.join("Desktop")), false),
        // 相对路径
        ("Desktop\\原神帧率解锁.lnk".to_string(), false),
        ("../x/原神帧率解锁.lnk".to_string(), false),
        // 路径穿越
        (p(&tmp.join("Desktop").join("..").join("..").join("evil.lnk")), false),
        // 系统目录（桩：以 /windows/ 开头）
        ("/windows/system32/evil.lnk".to_string(), false),
        // 符号链接 / junction（桩：路径里含 REPARSE）
        (
            p(&tmp.join("REPARSE").join("Desktop").join("evil.lnk")),
            false,
        ),
        // 名字不属于本产品，且不在 Desktop / Programs\<产品名> 下
        ("/home/u/.config/autostart/app.desktop".to_string(), false),
    ];
    for (path, want) in cases {
        let got = is_safe_shortcut_target(Path::new(&path), &allowed);
        check(&format!("{path} => {want}"), got == want, format!("got {got}"));
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

async fn rm_best_effort_cases() {
    println!("[4] rm_best_effort 只删安全路径");
    let tmp: PathBuf = std::env::temp_dir().join("kcheck-rm");
    let _ = std::fs::remove_dir_all(&tmp);
    let programs = tmp.join("Start Menu").join("Programs");
    std::fs::create_dir_all(programs.join("MyApp")).unwrap();
    std::fs::create_dir_all(tmp.join("Desktop")).unwrap();
    let good = programs.join("MyApp").join("MyApp.lnk");
    let desktop = tmp.join("Desktop").join("MyApp.lnk");
    let other = tmp.join("Desktop").join("keep.exe");
    std::fs::write(&good, b"x").unwrap();
    std::fs::write(&desktop, b"x").unwrap();
    std::fs::write(&other, b"x").unwrap();
    rm_best_effort(
        &[
            good.to_string_lossy().to_string(),
            desktop.to_string_lossy().to_string(),
            other.to_string_lossy().to_string(),
            "/windows/system32/cmd.lnk".to_string(),
        ],
        &["MyApp".to_string()],
    )
    .await;
    check("安全路径已删除", !good.exists() && !desktop.exists(), "");
    check("不安全路径原样保留", other.exists(), p(&other));
    let _ = std::fs::remove_dir_all(&tmp);
}

fn p(v: &Path) -> String {
    v.to_string_lossy().to_string()
}

fn agreement_wiring_case() {
    println!("[5] 协议内联到安装器（临时配置 + 临时协议文件）");
    // 仓库根：CARGO_MANIFEST_DIR = <repo>/tools/devcheck/rust/logic
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .expect("repo root")
        .to_path_buf();
    // 下游应用的配置与协议正文在 HoYoEnhance 仓库，本仓库不再持有它们：
    // 这里用临时文件把 resolve_agreement 的完整链路（读文件 → 内联 → 仍可序列化）跑一遍。
    // 「配置里的任务名与宿主 Autostart.cs 一致」这类跨仓库接线断言在下游仓库里做。
    let _ = &repo;
    let tmp = std::env::temp_dir().join(format!("kirara-agreement-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create tmp dir");
    let agreement_src = "用户协议（自检用正文）\n\
        本段文字只用于验证协议正文会被原样内联进安装器：\n\
        第一，正文长度必须超过阈值，否则会触发「content 非空」这条断言；\n\
        第二，换行与缩进都要保留，安装界面的弹窗按 text 格式原样渲染；\n\
        第三，内联结果仍要是合法 JSON，能被 uninst / update 的 pack 配置读取。\n";
    std::fs::write(tmp.join("USER_AGREEMENT.txt"), agreement_src).expect("write agreement");
    let cfg_path = tmp.join("kachina.config.json");
    std::fs::write(
        &cfg_path,
        r#"{"agreementFile":"USER_AGREEMENT.txt","agreementFormat":"text","agreementTitle":"用户协议"}"#,
    )
    .expect("write config");
    let raw = std::fs::read_to_string(&cfg_path).expect("read config");
    let mut config: serde_json::Value = serde_json::from_str(&raw).expect("parse config");
    let before = config.get("agreement").is_some();
    resolve_agreement(&mut config, &cfg_path);
    let obj = config.as_object().unwrap();
    let ag = obj.get("agreement").and_then(|v| v.as_object());
    check("配置里原本没有内联 agreement", !before, "");
    check(
        "resolve_agreement 写出了 agreement 对象",
        ag.is_some(),
        "".to_string(),
    );
    if let Some(ag) = ag {
        let content = ag.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let format = ag.get("format").and_then(|v| v.as_str()).unwrap_or("");
        let title = ag.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let src = agreement_src.to_string();
        check("title == 用户协议", title == "用户协议", title);
        check("format == text", format == "text", format);
        check(
            "content 与 USER_AGREEMENT.txt 完全一致",
            content == src.replace("\r\n", "\n").trim_end(),
            format!("len={} vs {}", content.len(), src.trim_end().len()),
        );
        check("content 非空", content.len() > 100, content.len().to_string());
        // 内联后必须仍是合法 JSON（会被写进 uninst/inst 的 pack config）
        let dumped = serde_json::to_string(&config).expect("serialize");
        let reparsed: serde_json::Value = serde_json::from_str(&dumped).expect("reparse");
        check(
            "内联结果可重新序列化为合法 JSON",
            reparsed["agreement"]["content"] == serde_json::Value::String(content.to_string()),
            "",
        );
    }
    // 反例：agreementFile 指向不存在的文件时，不得写出 content。
    // resolve_agreement 内部会往 stderr 打一条 "Warning: failed to read agreementFile ..."，
    // 这正是我们要的行为，但光看日志会以为是故障 —— 所以先用 eprintln 把说明打到
    // 同一个流里，让它紧挨着那条 Warning（devcheck 是把 stdout / stderr 分别读完再拼的，
    // 用 println 打说明会跑到前半段去，跟 Warning 分家）。
    eprintln!("（预期告警 ↓ 反例用例：agreementFile 指向不存在的文件，应告警且不写出 content）");
    let mut bad: serde_json::Value = serde_json::from_str(
        r#"{"agreementFile":"../NO_SUCH_FILE.txt","agreementFormat":"text"}"#,
    )
    .unwrap();
    resolve_agreement(&mut bad, &cfg_path);
    let has_content = bad
        .get("agreement")
        .and_then(|v| v.get("content"))
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty());
    check("协议文件缺失时不写出 content（链接保持不可点）", !has_content, "");
    let _ = std::fs::remove_dir_all(&tmp);
}


fn delete_target_cases() {
    println!("[6] 用户数据 / 额外目录安全阀 is_safe_delete_target");
    let lad = std::env::temp_dir().join("kcheck-lad");
    std::env::set_var("LOCALAPPDATA", &lad);
    // 一律用平台相关的临时目录拼：写死 /tmp/... 在 Windows 上不是绝对路径
    // （没有盘符），会被「必须绝对路径」这条先拦掉，断言就测不到它想测的规则。
    let public = std::env::temp_dir().join("kcheck-public");
    std::env::set_var("PUBLIC", &public);
    // 受保护的 Shell 容器按「配置目录 + 相对组件」推出来，所以这里把三个变量
    // 都指到同一个假配置目录上（下面 [9]/[10] 还会再改 USERPROFILE，互不影响）
    let shell_profile = std::env::temp_dir().join("kcheck-shellprofile");
    std::env::set_var("USERPROFILE", &shell_profile);
    std::env::set_var("APPDATA", shell_profile.join("AppData").join("Roaming"));
    std::env::set_var("LOCALAPPDATA", &lad);
    let cases: Vec<(String, bool)> = vec![
        // 正常：产品自己的数据目录
        (p(&lad.join("GenshinFpsUnlocker")), true),
        (p(&lad.join("GenshinFpsUnlocker").join("logs")), true),
        // 受保护根本身
        (p(&lad), false),
        (p(&public), false),
        // 层级太浅 / 根
        ("/tmp".to_string(), false),
        ("/".to_string(), false),
        // 用户配置目录下面一层的 Shell 容器：产品目录一定在它们**下面**，
        // 配置里少写一段就会把桌面 / 文档 / 开始菜单整个端掉，必须拦
        (p(&shell_profile.join("Desktop")), false),
        (p(&shell_profile.join("Documents")), false),
        (p(&shell_profile.join("Downloads")), false),
        (p(&shell_profile.join("AppData")), false),
        (p(&shell_profile.join("AppData").join("Local")), false),
        (p(&shell_profile.join("AppData").join("Roaming")), false),
        (
            p(
                &shell_profile
                    .join("AppData")
                    .join("Roaming")
                    .join("Microsoft")
                    .join("Windows")
                    .join("Start Menu")
                    .join("Programs"),
            ),
            false,
        ),
        // 但容器下面一层的产品目录仍然放行（含 .lnk 文件）
        (p(&shell_profile.join("Documents").join("GenshinFpsUnlocker")), true),
        (
            p(&shell_profile.join("Desktop").join("GenshinFpsUnlocker.lnk")),
            true,
        ),
        // 形状不合法
        ("AppData/Local/X".to_string(), false),
        (
            p(&std::env::temp_dir().join("a").join("..").join("b")),
            false,
        ),
        // 系统目录（桩：以 /windows/ 开头）
        ("/windows/system32/drivers".to_string(), false),
        // 符号链接（桩：含 REPARSE）
        (p(&lad.join("REPARSE").join("x")), false),
    ];
    for (path, want) in cases {
        let got = is_safe_delete_target(Path::new(&path));
        check(&format!("{path} => {want}"), got == want, format!("got {got}"));
    }
}

fn path_eq_cases() {
    println!("[7] path_eq 归一化");
    let cases: Vec<(&str, &str, bool)> = vec![
        (r"C:\Users\Public", "C:/Users/Public/", true),
        (r"C:\Users\Public\", r"C:\users\PUBLIC", true),
        ("C:", r"C:\", true),
        (r"C:\a", r"C:\b", false),
        (r"C:\Program Files\App", r"C:\Program Files", false),
    ];
    for (a, b, want) in cases {
        let got = path_eq(Path::new(a), Path::new(b));
        check(&format!("{a} == {b} => {want}"), got == want, format!("got {got}"));
    }
}


fn expand_env_cases() {
    println!("[8] %VAR% 展开（配置里的用户数据路径）");
    // 同上：期望值必须是「本平台认得的绝对路径」，否则 is_safe_delete_target
    // 与断言里的 is_absolute() 在 Windows 上会假失败。
    let lad = std::env::temp_dir()
        .join("kcheck-expand")
        .join("AppData")
        .join("Local");
    let lad_str = p(&lad);
    std::env::set_var("KCHECK_LAD", &lad_str);
    let cases: Vec<(String, String)> = vec![
        (
            "%KCHECK_LAD%/GenshinFpsUnlocker".to_string(),
            format!("{lad_str}/GenshinFpsUnlocker"),
        ),
        // 未知变量原样保留：宁可少删，也不要拼出半个路径去删
        (
            "%KCHECK_NOPE%/GenshinFpsUnlocker".to_string(),
            "%KCHECK_NOPE%/GenshinFpsUnlocker".to_string(),
        ),
        ("C:\\a\\b".to_string(), "C:\\a\\b".to_string()),
        ("100% done".to_string(), "100% done".to_string()),
        ("a%%b".to_string(), "a%b".to_string()),
    ];
    for (input, want) in cases {
        let got = expand_env_vars(&input);
        check(&format!("{input:?} => {want:?}"), got == want, format!("got {got:?}"));
    }
    // 回归：不展开 %VAR% 的话，安全阀会因为「不是绝对路径」把整条跳过 ——
    // 这正是「勾了删除用户数据也一个字节没删」的根因。
    let expanded = expand_path_list(&[
        "%KCHECK_LAD%/GenshinFpsUnlocker".to_string(),
        "   ".to_string(),
    ]);
    check(
        "expand_path_list 展开变量并丢掉空项",
        expanded.len() == 1 && Path::new(&expanded[0]).is_absolute(),
        format!("{expanded:?}"),
    );
    let raw = is_safe_delete_target(Path::new("%KCHECK_LAD%/GenshinFpsUnlocker"));
    let done = is_safe_delete_target(Path::new(&expanded[0]));
    check("展开前被安全阀拦掉、展开后放行", !raw && done, format!("raw={raw} done={done}"));
}

fn profile_tail_cases() {
    println!("[9] 多用户清理的相对尾部 profile_relative_tail");
    let profile = std::env::temp_dir().join("kcheck-profile").join("alice");
    std::env::set_var("USERPROFILE", &profile);
    let cases: Vec<(PathBuf, bool)> = vec![
        (profile.join("AppData/Local/GenshinFpsUnlocker"), true),
        (profile.join("AppData/Roaming/GenshinFpsUnlocker"), true),
        (profile.join("Documents/GenshinFpsUnlocker"), true),
        (
            profile.join("AppData/Roaming/Microsoft/Windows/Start Menu/Programs/GenshinFpsUnlocker"),
            true,
        ),
        (profile.join("Desktop/原神帧率解锁.lnk"), true),
        // 桌面上的非 .lnk：别人桌面的文档一概不碰
        (profile.join("Desktop/notes.txt"), false),
        // 尾部只有一级（等于把整个容器目录端掉）：不碰
        (profile.join("AppData"), false),
        (profile.join("Documents"), false),
        (profile.join("Foo"), false),
        // 白名单外的容器：不碰
        (profile.join("Downloads/GenshinFpsUnlocker"), false),
        (profile.join("Videos/a/b"), false),
        // —— 误删除防线：尾巴本身是 Shell 容器时一律拒绝 ——
        // 重放是「把尾巴拼到每个用户目录上」，所以尾巴退化成容器就等于把所有人的
        // 开始菜单 / 文档 / 桌面端掉。现实中触发得到：前端拼的开始菜单文件夹是
        // `Programs\{appName}`，appName 为空就成了 `Programs` 本身。
        (
            profile.join("AppData/Roaming/Microsoft/Windows/Start Menu/Programs"),
            false,
        ),
        (profile.join("AppData/Roaming/Microsoft/Windows/Start Menu"), false),
        (profile.join("AppData/Roaming/Microsoft/Windows"), false),
        (profile.join("AppData/Roaming/Microsoft"), false),
        (profile.join("AppData/Roaming"), false),
        (profile.join("AppData/Local"), false),
        (profile.join("AppData/Local/Microsoft/Windows"), false),
        (profile.join("Documents/GenshinFpsUnlocker/logs"), true),
        // 叶子名大小写不敏感
        (profile.join("AppData/Local/PROGRAMS"), false),
        (profile.join("AppData/Local/Microsoft"), false),
        // AppData 下只有两级 = 直接挂在 Local/Roaming 那一层，只能是容器
        (profile.join("AppData/Whatever"), false),
        // 路径穿越
        (profile.join("AppData/../secret"), false),
        // 压根不在配置目录下（公共开始菜单、安装目录等）：与「哪个用户」无关
        (
            std::env::temp_dir().join("kcheck-elsewhere/AppData/Local/GenshinFpsUnlocker"),
            false,
        ),
    ];
    for (path, want) in cases {
        let got = profile_relative_tail(&path).is_some();
        check(&format!("{} => {want}", p(&path)), got == want, format!("got {got}"));
    }
}

async fn per_user_sweep_cases() {
    println!("[10] 多用户残留清理（ProfileList 扫描 + 尽力删除）");
    let base = std::env::temp_dir().join("kcheck-multiuser");
    let _ = std::fs::remove_dir_all(&base);
    let profiles = base.join("profiles");
    let alice = base.join("alice");
    std::env::set_var("USERPROFILE", &alice);
    std::env::set_var("KCHECK_MOCK_PROFILE_BASE", &profiles);
    std::fs::create_dir_all(&alice).unwrap();

    // mock 的 ProfileList 会给出这些 SID：一个真实用户 + 三个必须跳过的
    let roots = [
        ("user", profiles.join("S-1-5-21-111-222-333-1001")),
        ("system", profiles.join("S-1-5-18")),
        ("default", profiles.join(".DEFAULT")),
        ("classes", profiles.join("S-1-5-21-111-222-333-1001_Classes")),
    ];
    for (_, root) in &roots {
        let data = root.join("AppData/Local/GenshinFpsUnlocker");
        std::fs::create_dir_all(data.join("logs")).unwrap();
        std::fs::write(data.join("config.json"), b"{}").unwrap();
        std::fs::create_dir_all(root.join("Desktop")).unwrap();
        std::fs::write(root.join("Desktop/原神帧率解锁.lnk"), b"x").unwrap();
        std::fs::write(root.join("Desktop/keep.txt"), b"x").unwrap();
    }
    let other = profiles.join("S-1-5-21-111-222-333-1001");

    let configured = vec![
        p(&alice.join("AppData/Local/GenshinFpsUnlocker")),
        p(&alice.join("Desktop/原神帧率解锁.lnk")),
        p(&alice.join("Desktop/keep.txt")),
        p(&base.join("ProgramData/GenshinFpsUnlocker")),
    ];
    let targets = collect_all_users_cleanup_targets(&configured);
    let shown = format!("{targets:?}");
    check(
        "候选只落在真实用户的配置目录里",
        !targets.is_empty() && targets.iter().all(|t| t.starts_with(&other)),
        shown.clone(),
    );
    check(
        "跳过 S-1-5-18 / .DEFAULT / *_Classes",
        !targets.iter().any(|t| {
            t.starts_with(&profiles.join("S-1-5-18"))
                || t.starts_with(&profiles.join(".DEFAULT"))
                || t.starts_with(&profiles.join("S-1-5-21-111-222-333-1001_Classes"))
        }),
        shown.clone(),
    );
    check(
        "其它账户的数据目录进了候选",
        targets.iter().any(|t| path_eq(t, &other.join("AppData/Local/GenshinFpsUnlocker"))),
        shown.clone(),
    );
    check(
        "其它账户桌面上的产品 .lnk 进了候选",
        targets.iter().any(|t| path_eq(t, &other.join("Desktop/原神帧率解锁.lnk"))),
        shown.clone(),
    );
    check(
        "桌面上的非 .lnk 与配置目录外的路径都没进候选",
        !targets.iter().any(|t| t.to_string_lossy().contains("keep.txt")
            || t.to_string_lossy().contains("ProgramData")),
        shown,
    );

    clean_per_user_leftovers(&configured).await;
    check("其它账户的数据目录已删", !other.join("AppData/Local/GenshinFpsUnlocker").exists(), "");
    check("其它账户的桌面 .lnk 已删", !other.join("Desktop/原神帧率解锁.lnk").exists(), "");
    check("其它账户桌面上的别的文件原样保留", other.join("Desktop/keep.txt").exists(), "");
    for (tag, root) in &roots {
        if *tag == "user" {
            continue;
        }
        check(
            &format!("{tag} 配置目录原样保留"),
            root.join("AppData/Local/GenshinFpsUnlocker/config.json").exists(),
            p(root),
        );
    }

    // —— 误删除防线（端到端）：尾巴退化成 Shell 容器时，一个字节都不能删 ——
    // 每个用户目录下都建一份「开始菜单 Programs + 里面的别人家快捷方式」，
    // 只要重放逻辑失手，keep.lnk 就会消失，断言立刻抓到。
    let shell_tails = [
        "AppData/Roaming/Microsoft/Windows/Start Menu/Programs",
        "AppData/Roaming/Microsoft/Windows/Start Menu",
        "AppData/Roaming/Microsoft",
        "AppData/Local",
        "Documents",
        "Desktop",
    ];
    for (_, root) in &roots {
        for tail in &shell_tails {
            std::fs::create_dir_all(root.join(tail)).unwrap();
            std::fs::write(root.join(tail).join("keep.lnk"), b"x").unwrap();
        }
    }
    let shell_inputs: Vec<String> = shell_tails.iter().map(|t| p(&alice.join(t))).collect();
    let shell_targets = collect_all_users_cleanup_targets(&shell_inputs);
    check(
        "Shell 容器尾巴一个候选都不产生",
        shell_targets.is_empty(),
        format!("{shell_targets:?}"),
    );
    clean_per_user_leftovers(&shell_inputs).await;
    check(
        "所有用户的 Programs / Documents / Desktop 原样保留",
        roots.iter().all(|(_, r)| {
            shell_tails
                .iter()
                .all(|t| r.join(t).join("keep.lnk").exists())
        }),
        "",
    );

    // 正向对照：产品自己的开始菜单文件夹仍然要跨用户清掉（别把功能一起防没了）
    let product_sm = "AppData/Roaming/Microsoft/Windows/Start Menu/Programs/GenshinFpsUnlocker";
    for (_, root) in &roots {
        std::fs::create_dir_all(root.join(product_sm)).unwrap();
        std::fs::write(root.join(product_sm).join("卸载.lnk"), b"x").unwrap();
    }
    let sm_inputs = vec![p(&alice.join(product_sm))];
    check(
        "产品开始菜单文件夹进了候选",
        collect_all_users_cleanup_targets(&sm_inputs)
            .iter()
            .any(|t| path_eq(t, &other.join(product_sm))),
        "",
    );
    clean_per_user_leftovers(&sm_inputs).await;
    check(
        "其它账户开始菜单里的产品文件夹已删",
        !other.join(product_sm).exists(),
        p(&other.join(product_sm)),
    );
    check(
        "Programs 目录本身与别人的快捷方式仍在",
        other
            .join("AppData/Roaming/Microsoft/Windows/Start Menu/Programs/keep.lnk")
            .exists(),
        "",
    );
    let _ = std::fs::remove_dir_all(&base);
}

async fn temp_artifact_cases() {
    println!("[11] %TEMP% 里本安装器的残留 clean_installer_temp_files");
    let temp = std::env::temp_dir();
    let mine = [
        "KachinaInstaller.log",
        "Kachina.RuntimePackage.Microsoft.DotNet.DesktopRuntime.9.exe",
        "kachina.uninst.1700000000.exe",
        "kachina.MicrosoftEdgeWebview2Setup.exe",
    ];
    for name in mine {
        std::fs::write(temp.join(name), b"x").unwrap();
    }
    // 含 kachina 但不在白名单里的，必须原样保留
    let keep = ["kcheck-keep-kachina.txt", "Kachina.RuntimePackage.foo.txt", "kachina.exe"];
    for name in keep {
        std::fs::write(temp.join(name), b"x").unwrap();
    }
    check("认得安装器日志", is_installer_temp_artifact("KachinaInstaller.log"), "");
    check(
        "认得运行时安装包（安装失败时会留下几十 MB）",
        is_installer_temp_artifact("Kachina.RuntimePackage.Microsoft.VCRedist.2015+.x64.exe"),
        "",
    );
    check("认得卸载器自身副本", is_installer_temp_artifact("kachina.uninst.1700000000.exe"), "");
    check("认得 WebView2 引导器", is_installer_temp_artifact("kachina.MicrosoftEdgeWebview2Setup.exe"), "");
    check(
        "不按「名字里含 kachina 就删」",
        !keep.iter().any(|n| is_installer_temp_artifact(n)),
        format!("{keep:?}"),
    );

    // 正在运行的卸载器自身（skip 参数）不许删
    let self_path = temp.join("kachina.uninst.1700000000.exe");
    clean_installer_temp_files(Some(&p(&self_path))).await;
    check("白名单内的临时文件已删", !temp.join("KachinaInstaller.log").exists(), "");
    check(
        "运行时安装包已删",
        !temp.join("Kachina.RuntimePackage.Microsoft.DotNet.DesktopRuntime.9.exe").exists(),
        "",
    );
    check("正在运行的卸载器副本被跳过", self_path.exists(), "");
    check(
        "非白名单文件原样保留",
        keep.iter().all(|n| temp.join(n).exists()),
        format!("{keep:?}"),
    );
    for name in mine.iter().chain(keep.iter()) {
        let _ = std::fs::remove_file(temp.join(name));
    }
}

/// 「勾选了才删用户数据」这条契约由三处代码配合成立，任何一处被改掉都会变成
/// 「用户没同意也删」或者「用户同意了却没删」。这里直接对**真实仓库文件**做静态
/// 断言，把三处钉住（和 [5] 协议内联那组同一个套路）：
/// 1. 前端只在勾选时把 `userDataPath` 传下去，没勾传空数组；
/// 2. 配置里每条 `userDataPath` 都是「`%VAR%`/产品子目录」形状，不是容器本身；
/// 3. Rust 侧 `%VAR%` 展开发生在删除安全阀**之前**，且跨用户清理吃的就是同一份
///    `to_be_delete`（于是勾选语义自动跟随，不需要第二套开关）。
fn uninstall_consent_wiring_case() {
    println!("[12] 卸载勾选语义（真实仓库文件）");
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .expect("repo root")
        .to_path_buf();

    // —— 1) 前端 ——
    let app = std::fs::read_to_string(repo.join("src/App.vue")).expect("read App.vue");
    let gated = app
        .split("user_data_path:")
        .nth(1)
        .and_then(|rest| rest.split("extra_uninstall_path:").next())
        .unwrap_or("");
    check("App.vue 里有 user_data_path 这一段", !gated.is_empty(), "");
    check(
        "未勾选「同时删除用户数据」时传空数组（= 一个数据目录都不删）",
        gated.contains("deleteUserData.value") && gated.contains(": []"),
        gated.trim().to_string(),
    );

    // —— 2) Rust 侧顺序 ——
    // 「配置里每条 userDataPath 都是 %VAR%/产品子目录形状」是对**下游应用配置**的断言，
    // 那份配置在 HoYoEnhance 仓库，因此由那边的 devcheck 负责。
    let rs = std::fs::read_to_string(
        repo.join("src-tauri/src/installer/uninstall.rs"),
    )
    .expect("read uninstall.rs");
    let expand_at = rs.find("let to_be_delete = expand_path_list(");
    let guard_at = rs.find("if !is_safe_delete_target(path)");
    check("run_uninstall 里先展开 %VAR%", expand_at.is_some(), format!("{expand_at:?}"));
    check("再走删除安全阀", guard_at.is_some(), format!("{guard_at:?}"));
    check(
        "展开在安全阀之前（顺序反了就等于没修「勾了也不删」）",
        matches!((expand_at, guard_at), (Some(a), Some(b)) if a < b),
        format!("{expand_at:?} vs {guard_at:?}"),
    );
    check(
        "跨用户清理吃的是同一份 to_be_delete（勾选语义自动跟随）",
        rs.contains("clean_per_user_leftovers(&to_be_delete)"),
        "",
    );
    check(
        "跨用户重放带 Shell 容器黑名单（防止把所有人的开始菜单端掉）",
        rs.contains("PER_USER_DENY_LEAVES"),
        "",
    );
}

fn rm_list_cases() {
    println!("[15] 旧版本残留清单 rm_list 的攻击形状（网络元数据 → 提权删除）");
    // 前端把 latest_meta.deletes（在线安装时来自网络）的每一项拼成
    // `${source}${sep()}${entry}` 交给提权进程逐个 remove_file。
    // 反斜杠形状的 `..` 在 Linux 上 Path::components() 看不出来（整个是一个文件名），
    // 靠的是 is_safe_delete_target 里那条按 / 与 \ 切段的文本判定 —— 两个平台都要拦。
    let install_dir = std::env::temp_dir().join("kcheck-install-dir");
    let base = p(&install_dir);

    let cases: Vec<(String, bool)> = vec![
        // 正常：安装目录里的旧文件 / 子目录里的旧文件
        (p(&install_dir.join("GenshinFpsUnlocker.exe")), true),
        (p(&install_dir.join("plugins").join("old.dll")), true),
        // 越界：反斜杠 `..` 逃到系统目录（Windows 上真实形状）
        (format!("{base}\\..\\..\\Windows\\System32\\x.dll"), false),
        (format!("{base}\\..\\..\\..\\Windows\\System32\\drivers\\x.sys"), false),
        // 越界：正斜杠 `..`（两个平台 components 都认得）
        (format!("{base}/../../windows/system32/x.dll"), false),
        (
            p(&install_dir.join("..").join("..").join("Windows").join("x.dll")),
            false,
        ),
        // 相对路径：清单里混进一个不是绝对路径的条目
        ("plugins\\old.dll".to_string(), false),
        ("GenshinFpsUnlocker.exe".to_string(), false),
        // 符号链接（桩：路径里含 REPARSE）
        (p(&install_dir.join("REPARSE").join("old.dll")), false),
        // 绝对路径塞进清单其实**无害**：拼接后变成 `<安装目录>\C:\Windows\...`，
        // 只是个不存在的怪路径，删不掉任何东西。真正的洞是 `..`，上面已经拦住。
        (format!("{base}\\C:\\Windows\\System32\\x.dll"), true),
    ];
    for (path, want) in cases {
        let got = is_safe_delete_target(Path::new(&path));
        check(&format!("{path} => {want}"), got == want, format!("got {got}"));
    }
}

fn relative_member_cases() {
    println!("[13] 安装目录内文件清单的安全阀 is_safe_relative_member / path_starts_with");
    // files 来自注册表 InstallerMeta（非提权安装时写在 HKCU，同账户中等完整性进程可改），
    // 却由提权卸载器逐个 remove_file —— 上游直接 source.join(f)，绝对路径会丢掉 source、
    // `..` 能逃出安装目录。
    let base = std::env::temp_dir().join("kcheck-install-dir");

    // 正例：安装目录里的普通文件与子目录（正反斜杠都要放行）
    for e in [
        "GenshinFpsUnlocker.exe",
        "FpsUnlockerStub.dll",
        "ui/index.html",
        r"ui\index.html",
        r"ui\sub\a.txt",
    ] {
        check(
            &format!("放行安装目录内的 {e}"),
            is_safe_relative_member(&base, e),
            "被拦了",
        );
    }

    // 反例：绝对路径（join 会把 base 整个丢掉）—— 两种平台的形状都要拦
    let elsewhere = std::env::temp_dir().join("kcheck-elsewhere").join("secret.txt");
    check(
        "拦掉 Windows 绝对路径 C:\\Windows\\...\\hosts",
        !is_safe_relative_member(&base, r"C:\Windows\System32\drivers\etc\hosts"),
        "放行了",
    );
    check(
        "拦掉本机绝对路径",
        !is_safe_relative_member(&base, &elsewhere.to_string_lossy()),
        "放行了",
    );

    // 反例：.. 逃逸
    for e in [
        "../secret.txt",
        r"..\..\Windows\win.ini",
        "ui/../../x.txt",
        r"ui\..\..\y.txt",
    ] {
        check(
            &format!("拦掉 .. 逃逸 {e}"),
            !is_safe_relative_member(&base, e),
            "放行了",
        );
    }

    // 反例：根相对 / UNC / 盘符相对 / 当前目录 / 空 / NUL
    for e in [
        r"\Windows\win.ini",
        "/etc/passwd",
        "C:evil.exe",
        r"\\server\share\x.exe",
        "",
        ".",
        "./x.txt",
        "a\u{0}b.txt",
    ] {
        check(
            &format!("拦掉非法形状 {:?}", e),
            !is_safe_relative_member(&base, e),
            "放行了",
        );
    }

    // path_starts_with：大小写不敏感 + 按分隔符边界比（不是裸字符串前缀）
    check(
        "大小写不一致也算在里面（注册表 InstallLocation 常见）",
        path_starts_with(
            Path::new(r"C:\Program Files\App\GenshinFpsUnlocker.uninst.exe"),
            Path::new(r"c:\program files\app"),
        ),
        "没认出来",
    );
    check(
        "斜杠方向不同也算在里面",
        path_starts_with(Path::new("C:/App/x.exe"), Path::new(r"C:\App")),
        "没认出来",
    );
    check(
        "不是裸前缀：C:\\Foo 不在 C:\\F 里",
        !path_starts_with(Path::new(r"C:\Foo\x.exe"), Path::new(r"C:\F")),
        "误判成在里面",
    );
    check(
        "相等不算在里面",
        !path_starts_with(Path::new(r"C:\App"), Path::new(r"C:\App")),
        "相等被当成子路径",
    );
    check(
        "盘符根下面算在里面",
        path_starts_with(Path::new(r"C:\x.exe"), Path::new("C:")),
        "没认出来",
    );
    check(
        "空 parent 不放行",
        !path_starts_with(Path::new(r"C:\x.exe"), Path::new("")),
        "放行了",
    );
}

fn runtime_signature_cases() {
    println!("[14] 微软安装包验签判定 is_trusted_microsoft_signature");
    let ms = "CN=Microsoft Corporation, O=Microsoft Corporation, L=Redmond, S=Washington, C=US";
    check(
        "微软签名 + Valid 放行",
        is_trusted_microsoft_signature("Valid", ms),
        "被拦了",
    );
    check(
        "Status 大小写不敏感",
        is_trusted_microsoft_signature("valid", ms),
        "被拦了",
    );
    check(
        "Subject 段落顺序不同也认得",
        is_trusted_microsoft_signature("Valid", "O=Microsoft Corporation, CN=Microsoft Corporation"),
        "被拦了",
    );
    for (status, why) in [
        ("NotSigned", "未签名"),
        ("HashMismatch", "哈希不符"),
        ("UnknownError", "验签失败"),
        ("", "空状态"),
    ] {
        check(
            &format!("拦掉{why}（{status}）"),
            !is_trusted_microsoft_signature(status, ms),
            "放行了",
        );
    }
    check(
        "拦掉别人签的（Status=Valid 也不放行）",
        !is_trusted_microsoft_signature("Valid", "CN=Evil Corp, O=Evil Corp, L=X, C=CN"),
        "放行了",
    );
    check(
        "拦掉冒名写法（子串匹配会放过的那种）",
        !is_trusted_microsoft_signature("Valid", "CN=Not Microsoft Corporation Ltd"),
        "放行了",
    );
    check(
        "拦掉空签名者",
        !is_trusted_microsoft_signature("Valid", ""),
        "放行了",
    );
}

fn scheduled_task_name_cases() {
    println!("[17] 计划任务名安全阀 is_safe_task_name");
    let long_name = format!("GenshinFpsUnlocker.{}", "a".repeat(200));
    let cases: Vec<(&str, &str, bool)> = vec![
        // 正常：本产品的登录任务
        ("GenshinFpsUnlocker", "GenshinFpsUnlocker.AutoStart", true),
        ("GenshinFpsUnlocker", "genshinfpsunlocker.autostart", true),
        ("GenshinFpsUnlocker", "GenshinFpsUnlocker", true),
        ("GenshinFpsUnlocker", "  GenshinFpsUnlocker.AutoStart  ", true),
        // 通配符：等于清空整台机器的任务
        ("GenshinFpsUnlocker", "*", false),
        ("GenshinFpsUnlocker", "GenshinFpsUnlocker*", false),
        ("GenshinFpsUnlocker", "GenshinFpsUnlocker?", false),
        // 目录形态 / 别人的任务
        ("GenshinFpsUnlocker", r"GenshinFpsUnlocker\Evil", false),
        ("GenshinFpsUnlocker", r"\GenshinFpsUnlocker.AutoStart", false),
        ("GenshinFpsUnlocker", "Other.Product", false),
        // 空产品名 / 空任务名 / 超长
        ("", "GenshinFpsUnlocker.AutoStart", false),
        ("   ", "GenshinFpsUnlocker.AutoStart", false),
        ("GenshinFpsUnlocker", "", false),
        ("GenshinFpsUnlocker", "   ", false),
        ("GenshinFpsUnlocker", long_name.as_str(), false),
        // 命令行注入形态（虽然 schtasks 用参数数组传，不放行更省心）
        ("GenshinFpsUnlocker", "GenshinFpsUnlocker.AutoStart\" /F", false),
        ("GenshinFpsUnlocker", "GenshinFpsUnlocker.AutoStart&calc", false),
        ("GenshinFpsUnlocker", "GenshinFpsUnlocker.AutoStart;whoami", false),
    ];
    for (product, name, want) in cases {
        let got = is_safe_task_name(product, name);
        check(
            &format!("product={product:?} name={name:?} => {want}"),
            got == want,
            format!("got {got}"),
        );
    }
}

fn scheduled_task_wiring_case() {
    println!("[18] 卸载登录计划任务接线（本仓库文件）");
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .expect("repo root")
        .to_path_buf();

    // 「配置里登记的任务名 == 宿主 Autostart.cs 里的任务名」是对**下游应用配置**的断言，
    // 那份配置与宿主源码都在 HoYoEnhance 仓库，由那边的 devcheck 负责。
    let app = std::fs::read_to_string(repo.join("src/App.vue")).expect("read App.vue");
    check(
        "App.vue 把 extraUninstallScheduledTasks 传给卸载器",
        app.contains("extra_uninstall_scheduled_tasks")
            && app.contains("PROJECT_CONFIG.extraUninstallScheduledTasks"),
        "",
    );
    let ipc = std::fs::read_to_string(repo.join("src/api/ipc.ts")).expect("read ipc.ts");
    check(
        "ipc.ts 声明 extra_uninstall_scheduled_tasks",
        ipc.contains("extra_uninstall_scheduled_tasks"),
        "",
    );
    let types = std::fs::read_to_string(repo.join("src/types.ts")).expect("read types.ts");
    check(
        "types.ts 声明 extraUninstallScheduledTasks",
        types.contains("extraUninstallScheduledTasks"),
        "",
    );

    let rs = std::fs::read_to_string(
        repo.join("src-tauri/src/installer/uninstall.rs"),
    )
    .expect("read uninstall.rs");
    check(
        "uninstall.rs 用 reg_name 调 clean_extra_scheduled_tasks",
        rs.contains("clean_extra_scheduled_tasks(&reg_name"),
        "",
    );
}

fn zip_entry_name_cases() {
    println!("[19] zip 条目名解码（替代 zip fork 的强制 UTF-8）");
    // 打包工具写中文名却不置 UTF-8 标志位时，zip 自己会按 CP437 解出乱码
    // （「中文」→「Σ╕¡µûç」）；decode_entry_name 必须按原始字节还原。
    let utf8_raw = "中文/文件.txt".as_bytes();
    let got = decode_entry_name(utf8_raw);
    check(
        "未置位的中文名按 UTF-8 还原",
        got == "中文/文件.txt",
        format!("got {got}"),
    );
    // 非 UTF-8 字节：与 fork 一样走 lossy 解码，不 panic、也不丢条目。
    let lossy = decode_entry_name(&[0xff, 0xfe, b'a']);
    check(
        "非 UTF-8 字节走 lossy 解码",
        lossy == "\u{fffd}\u{fffd}a",
        format!("got {lossy:?}"),
    );
    // ASCII 名（changes.json / .metadata.json 这类）不受影响。
    let ascii = decode_entry_name(b"changes.json");
    check("ASCII 名不受影响", ascii == "changes.json", format!("got {ascii}"));
}

/// H3 证书固定用的样例证书：openssl 现生成的 P-256 自签证书（CN=h3check.example），
/// 三个常量都由 openssl 侧独立算出来，用来交叉验证我们自己写的 DER 解析。
const H3_CERT_DER_HEX: &str = concat!(
    "308201a53082014ba0030201020214483c51f5abd347944d1c8e31bec76ad5908db5c5300a06082a8648ce3d040302301a31",
    "18301606035504030c0f6833636865636b2e6578616d706c65301e170d3236303932303038343832375a170d333630393137",
    "3038343832375a301a3118301606035504030c0f6833636865636b2e6578616d706c653059301306072a8648ce3d02010608",
    "2a8648ce3d03010703420004d7ed138f10848d71c26e9d40f0e17f3663fa72dc2bdf7c064e4bafe7918e33c6e3e5ca94a092",
    "adc6125bc1a17e196045ae50c535b86440a94305ceec6bba9de2a36f306d301d0603551d0e0416041420af054940430aa165",
    "dff8a6b27b6b0b6a524152301f0603551d2304183016801420af054940430aa165dff8a6b27b6b0b6a524152300f0603551d",
    "130101ff040530030101ff301a0603551d1104133011820f6833636865636b2e6578616d706c65300a06082a8648ce3d0403",
    "02034800304502200a81dca0423688aeadeab80d039a5a4e31f5cf0840261de6471c0ddb0e20c455022100e426f3160b8d8a",
    "d32949224717db0297427011baee0e59eec8a7017dc5befa42",
);
const H3_CERT_SHA256: &str = "898f47029c53f83646567fa56237fcdc6fc111de5f5e2ae8ac84197be0ae98a6";
const H3_SPKI_SHA256: &str = "9af0e528ef99ca47a58d45101878168c38d283655d971eef7608f8d1bd7f469d";

fn h3_pin_cases() {
    println!("[20] H3 证书固定（pinning）与 SPKI 哈希");

    let parse = |frag: &str| {
        parse_pin_from_fragment(
            &url::Url::parse(&format!("http3://example.com/file#{frag}")).expect("url"),
        )
    };
    let spki = "aa".repeat(32);
    let cert = "bb".repeat(32);

    let cfg = parse(&format!("spki={spki}")).expect("spki");
    check(
        "固定值：只给 spki 时默认 force",
        cfg.mode == PinningMode::Force && cfg.target == PinTarget::Spki([0xaa; 32]),
        format!("{cfg:?}"),
    );

    let cfg = parse(&format!("spki={spki}&pinning_mode=add")).expect("spki+add");
    check(
        "固定值：pinning_mode=add 生效",
        cfg.mode == PinningMode::Add,
        format!("{cfg:?}"),
    );

    let cfg = parse(&format!("spki={spki}&pinning_mode=whatever")).expect("unknown mode");
    check(
        "固定值：未知 pinning_mode 退回 force（安全默认值）",
        cfg.mode == PinningMode::Force,
        format!("{cfg:?}"),
    );

    let cfg = parse(&format!("cert={cert}")).expect("cert");
    check(
        "固定值：cert 目标可解析",
        cfg.target == PinTarget::Cert([0xbb; 32]),
        format!("{cfg:?}"),
    );

    let cfg = parse(&format!("spki={spki}&cert={cert}&pinning_mode=add")).expect("both");
    check(
        "固定值：spki 与 cert 同时出现时 cert 优先",
        cfg.target == PinTarget::Cert([0xbb; 32]) && cfg.mode == PinningMode::Add,
        format!("{cfg:?}"),
    );

    check(
        "固定值：长度不足 64 被拒绝",
        parse("spki=abcd").is_none(),
        "长度 4 的 spki 不该通过".to_string(),
    );
    check(
        "固定值：非 hex 被拒绝",
        parse(&format!("spki={}", "zz".repeat(32))).is_none(),
        "非 hex 不该通过".to_string(),
    );
    check(
        "固定值：只写 pinning_mode 被拒绝",
        parse("pinning_mode=add").is_none(),
        "没有 spki/cert 时不该给出配置".to_string(),
    );
    check(
        "固定值：没有 fragment 被拒绝",
        parse_pin_from_fragment(&url::Url::parse("http3://example.com/file").expect("url"))
            .is_none(),
        "无 fragment 不该给出配置".to_string(),
    );

    // SPKI 定位必须与 openssl 完全一致，否则用户按文档算出来的固定值会全部失配。
    let der = hex::decode(H3_CERT_DER_HEX).expect("fixture hex");
    check(
        "证书哈希：整张证书的 SHA-256 与 openssl 一致",
        hex::encode(compute_cert_hash(&der)) == H3_CERT_SHA256,
        format!("got {}", hex::encode(compute_cert_hash(&der))),
    );
    check(
        "证书哈希：SPKI 的 SHA-256 与 openssl 一致",
        compute_spki_hash(&der).map(hex::encode).as_deref() == Some(H3_SPKI_SHA256),
        format!("got {:?}", compute_spki_hash(&der).map(hex::encode)),
    );
    check(
        "证书哈希：截断的 DER 被拒绝",
        extract_spki_der(&der[..der.len() / 2]).is_none(),
        "截断输入不该解析出 SPKI".to_string(),
    );
    check(
        "证书哈希：不定长编码被拒绝",
        extract_spki_der(&[0x30, 0x80, 0x00]).is_none(),
        "DER 不允许不定长".to_string(),
    );
}

/// 造一个最小 PE 映像：DOS 头 + `e_lfanew` 指向的 `PE\0\0`。
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

fn builder_pack_cases() {
    println!("[21] 打包器：包体 PE 识别 / 嵌入名规则 / 抽取路径安全阀");

    let pe = mini_pe(1);
    check(
        "PE：合法映像起点被识别",
        is_pe_at(&pe, 0),
        "e_lfanew=0x80 且带 PE 签名".to_string(),
    );
    let mut false_mz = vec![0x4D, 0x5A, 0x90, 0x00];
    false_mz.extend_from_slice(&[0u8; 60]);
    check(
        "PE：只有 MZ\\x90\\x00 不算映像",
        !is_pe_at(&false_mz, 0),
        "缺 PE 签名".to_string(),
    );
    check(
        "PE：截断输入不 panic",
        !is_pe_at(&[0x4D, 0x5A], 0),
        "长度不足 0x40".to_string(),
    );
    let mut far = pe.clone();
    far[0x3C..0x40].copy_from_slice(&0x2000u32.to_le_bytes());
    check(
        "PE：e_lfanew 超出窗口被拒",
        !is_pe_at(&far, 0),
        "0x2000 > 0x1000".to_string(),
    );
    let mut low = pe.clone();
    low[0x3C..0x40].copy_from_slice(&0x20u32.to_le_bytes());
    check(
        "PE：e_lfanew 过小被拒",
        !is_pe_at(&low, 0),
        "0x20 < 0x40".to_string(),
    );

    // 打包产物 = builder 字节 + installer 字节；安装器体内再埋一个 DOS 魔数。
    // 旧实现按 MZ\x90\x00 扫，会把埋在体内的那个当成映像起点，于是 rcedit 加载半截文件。
    let builder = mini_pe(1);
    let installer = mini_pe(2);
    let mut bundle = builder.clone();
    bundle.extend_from_slice(&installer);
    let planted = builder.len() + 0x40;
    bundle[planted..planted + 4].copy_from_slice(&[0x4D, 0x5A, 0x90, 0x00]);
    check(
        "包体：只认真正的 PE 起点（忽略体内埋的 MZ）",
        pe_image_starts(&bundle) == vec![0, builder.len()],
        format!("got {:?}", pe_image_starts(&bundle)),
    );

    for ok in ["\0CONFIG", "changes.json", "abc-DEF_1.2", "a"] {
        check(
            &format!("嵌入名：{ok:?} 放行"),
            is_embedded_name(ok),
            "应放行".to_string(),
        );
    }
    for bad in ["", "中文", "a b", "../x", "a/b", "\0NOPE"] {
        check(
            &format!("嵌入名：{bad:?} 拒绝"),
            !is_embedded_name(bad),
            "应拒绝".to_string(),
        );
    }

    let (md5, xxh) = (Some("m".to_string()), Some("x".to_string()));
    check(
        "哈希取值：md5 优先",
        preferred_file_hash(&md5, &xxh).map(String::as_str) == Some("m"),
        "两个都有时取 md5".to_string(),
    );
    check(
        "哈希取值：只有 xxh 时用它",
        preferred_file_hash(&None, &xxh).map(String::as_str) == Some("x"),
        "md5 缺失时取 xxh".to_string(),
    );
    check(
        "哈希取值：都没有则 None",
        preferred_file_hash(&None, &None).is_none(),
        "两个都没有时应为 None".to_string(),
    );

    // 抽取路径安全阀：包内路径来自归档 metadata，越界必须整体拒绝。
    // 只断言跨平台语义一致的部分；`..\x` / `C:\x` 这类反斜杠形式在 Linux 上
    // 是普通文件名，由 Windows 上的 cargo test 覆盖（见 utils/hash.rs 同批改动）。
    let root = Path::new("out");
    check(
        "抽取路径：单层文件名放行",
        relative_under_root(root, "app.exe").is_ok(),
        "app.exe".to_string(),
    );
    check(
        "抽取路径：子目录放行",
        relative_under_root(root, "User/settings.json").is_ok(),
        "User/settings.json".to_string(),
    );
    for evil in ["../outside.txt", "a/../../outside.txt", "/etc/passwd", ""] {
        check(
            &format!("抽取路径：{evil:?} 被拒"),
            relative_under_root(root, evil).is_err(),
            "应拒绝".to_string(),
        );
    }
}

fn hash_reader_cases() {
    println!("[22] 文件哈希：md5 / xxh / sha256 摘要与分块边界");

    let hello = hash_reader("md5", &b"hello"[..]).expect("md5 hello");
    check(
        "md5：已知摘要",
        hello == "5d41402abc4b2a76b9719d911017c592",
        format!("got {hello}"),
    );
    let empty = hash_reader("md5", &b""[..]).expect("md5 empty");
    check(
        "md5：空输入",
        empty == "d41d8cd98f00b204e9800998ecf8427e",
        format!("got {empty}"),
    );

    // 跨过 1 MB 读缓冲：分块读不能改变摘要（换缓冲大小、改成一次性读都要一致）
    let mut big = vec![0u8; 3 * 1024 * 1024 + 7];
    for (i, b) in big.iter_mut().enumerate() {
        *b = (i % 251) as u8;
    }
    let expected_md5 = chksum_md5::hash(&big).to_hex_lowercase();
    let got_md5 = hash_reader("md5", &big[..]).expect("md5 big");
    check(
        "md5：跨 1 MB 分块与一次性哈希一致",
        got_md5 == expected_md5,
        format!("got {got_md5} want {expected_md5}"),
    );

    let mut hasher = twox_hash::XxHash3_128::new();
    // XxHash3_128 有自己的 `write(&[u8])`，不走 std::hash::Hasher（后者只到 64 位）
    hasher.write(&big);
    let expected_xxh = format!("{:x}", hasher.finish_128());
    let got_xxh = hash_reader("xxh", &big[..]).expect("xxh big");
    check(
        "xxh：与直接哈希一致",
        got_xxh == expected_xxh,
        format!("got {got_xxh} want {expected_xxh}"),
    );

    // Mirror酱 归档校验用的就是这条分支：摘要算错等于校验形同虚设。
    let got_sha_hello = hash_reader("sha256", &b"hello"[..]).expect("sha256 hello");
    check(
        "sha256：已知摘要",
        got_sha_hello
            == "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
        format!("got {got_sha_hello}"),
    );
    use sha2::Digest as _;
    let mut sha_big = sha2::Sha256::new();
    sha_big.update(&big);
    let expected_sha = format!("{:x}", sha_big.finalize());
    let got_sha_big = hash_reader("sha256", &big[..]).expect("sha256 big");
    check(
        "sha256：跨 1 MB 分块与一次性哈希一致",
        got_sha_big == expected_sha,
        format!("got {got_sha_big} want {expected_sha}"),
    );

    check(
        "未知算法报错",
        hash_reader("sha1", &b"x"[..]).is_err(),
        "NO_HASH_ALGO_ERR".to_string(),
    );
}

/// [24] 自更新回滚：更新失败时旧安装器必须原地回来。
///
/// 这条保证的另一半（「成功才登记退出自删」）只能靠 `DELETE_SELF_ON_EXIT_PATH` 的静态
/// 写入点唯一来保证，见 `LOCAL_PATCHES.md` 第 19a 节与同名的 note。
fn rollback_self_update_cases() {
    println!("[24] 自更新回滚：失败路径还原旧安装器");

    let dir = std::env::temp_dir().join(format!(
        "kirara-rollback-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");

    // 1) 直写模式失败：目标位置留下半截新文件，备份还在
    let target = dir.join("updater.exe");
    let backup = dir.join("updater.instbak");
    std::fs::write(&target, b"NEW-PARTIAL").expect("write partial target");
    std::fs::write(&backup, b"OLD").expect("write backup");
    let res = rollback_self_update_backup_sync(&target, &backup);
    check(
        "直写模式失败：备份改回原名",
        res.is_ok() && std::fs::read(&target).unwrap() == b"OLD",
        format!("{res:?}"),
    );
    check(
        "直写模式失败：半截新文件被丢掉、.instbak 不再存在",
        !backup.exists(),
        format!("backup exists = {}", backup.exists()),
    );

    // 2) patch 模式失败：目标根本不存在（失败在 .patching 上），备份同样要回来
    let target2 = dir.join("app.dll");
    let backup2 = dir.join("app.dll.instbak");
    std::fs::write(&backup2, b"OLD-DLL").expect("write backup2");
    let res2 = rollback_self_update_backup_sync(&target2, &backup2);
    check(
        "patch 模式失败：目标缺失也照样还原",
        res2.is_ok() && std::fs::read(&target2).unwrap() == b"OLD-DLL",
        format!("{res2:?}"),
    );

    // 3) 备份不存在：如实报错，调用方据此把备份路径写进日志
    let res3 = rollback_self_update_backup_sync(
        &dir.join("gone.exe"),
        &dir.join("gone.exe.instbak"),
    );
    check(
        "备份不存在时报错（不静默假装还原成功）",
        res3.is_err(),
        format!("{res3:?}"),
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// [25] 无人值守运行不得弹模态框：否则 CI / 静默安装在失败路径上永久等待。
fn unattended_dialog_cases() {
    println!("[25] 无人值守运行不弹模态框");

    check(
        "交互运行允许弹框",
        should_show_dialog(false, false),
        "应返回 true".to_string(),
    );
    check(
        "静默运行禁止弹框",
        !should_show_dialog(true, false),
        "应返回 false".to_string(),
    );
    check(
        "非交互运行禁止弹框",
        !should_show_dialog(false, true),
        "应返回 false".to_string(),
    );
    check(
        "静默 + 非交互仍禁止弹框",
        !should_show_dialog(true, true),
        "应返回 false".to_string(),
    );
}

/// [26] 提权 helper 退出自删接线：C9 自更新把旧安装器留在暂存目录的 `old\`，
/// helper 没有 UI 窗口，不主动执行的话这份旧镜像只能等下次 `OpenStaging` 清掉。
fn uac_cleanup_wiring_case() {
    println!("[26] 提权 helper 退出自删接线（本仓库文件）");
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .expect("repo root")
        .to_path_buf();

    let manager = std::fs::read_to_string(repo.join("src-tauri/src/ipc_v2/manager.rs"))
        .expect("read ipc_v2/manager.rs");
    let uac_body = manager
        .split("pub async fn uac_ipc_main")
        .nth(1)
        .expect("uac_ipc_main exists");
    check(
        "uac_ipc_main 退出时调用 delete_self_on_exit",
        uac_body.contains("delete_self_on_exit()"),
        "",
    );

    let host_window = std::fs::read_to_string(repo.join("src-tauri/src/host/window.rs"))
        .expect("read host/window.rs");
    check(
        "原生宿主 WM_CLOSE 仍调用 delete_self_on_exit",
        host_window.contains("WM_CLOSE") && host_window.contains("delete_self_on_exit();"),
        "",
    );
}

#[tokio::main]
async fn main() {
    reg_target_cases();
    clean_registry_cases();
    shortcut_cases();
    rm_best_effort_cases().await;
    agreement_wiring_case();
    delete_target_cases();
    path_eq_cases();
    expand_env_cases();
    profile_tail_cases();
    per_user_sweep_cases().await;
    temp_artifact_cases().await;
    uninstall_consent_wiring_case();
    relative_member_cases();
    runtime_signature_cases();
    rm_list_cases();
    scheduled_task_name_cases();
    scheduled_task_wiring_case();
    zip_entry_name_cases();
    h3_pin_cases();
    builder_pack_cases();
    hash_reader_cases();
    rollback_self_update_cases();
    unattended_dialog_cases();
    uac_cleanup_wiring_case();
    let (pass, fail) = (PASS.load(Ordering::Relaxed), FAIL.load(Ordering::Relaxed));
    println!("\n==== PASS {pass} / FAIL {fail} ====");
    if fail > 0 {
        std::process::exit(1);
    }
}
