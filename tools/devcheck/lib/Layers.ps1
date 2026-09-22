# devcheck 各层实现（被 devcheck.ps1 dot-source）
#
# 每个 Test-* 函数对应一个层：返回字符串 = 通过的说明，抛异常 = 失败。
# 依赖 devcheck.ps1 的 $RepoRoot / $DevCheckRoot / $KachinaSrc 与 lib/Common.ps1 的助手函数。
# 本仓库的根目录就是安装器源码（上游快照 + 本地补丁），所以源码路径不带前缀。

function Test-VendoredSource {
    # 源码必须是「仓库内的快照」：CI 与脚本都不许从上游
    # YuehaiTeam/kachina-installer 拉源码或下二进制，只能从本仓库构建。
    $notes = [System.Collections.Generic.List[string]]::new()

    # 1) 不是 submodule
    if (Test-Path -LiteralPath (Join-Path $RepoRoot '.gitmodules')) {
        throw '存在 .gitmodules —— 源码必须是仓库内的快照，不是 submodule'
    }

    # 2) 快照完整（缺一个就说明 vendored 源码被误删，CI 会退化成「去别处找」）
    $required = @(
        'package.json',
        'pnpm-lock.yaml',
        'src-tauri/Cargo.toml',
        'src-tauri/Cargo.lock',
        'src-tauri/src/installer/uninstall.rs',
        'src-tauri/src/builder/pack.rs',
        'src/App.vue',
        'build.ps1'
    )
    foreach ($r in $required) {
        if (-not (Test-Path -LiteralPath (Join-Path $RepoRoot $r))) { throw "源码快照不完整，缺 $r" }
    }
    $notes.Add("快照完整($($required.Count) 个关键文件)")

    # 3) 工作流与脚本里不许出现「从外部拉源码/下二进制」的动作
    #    只扫可执行内容：注释行（# 开头）跳过，避免误伤说明性文字
    $scan = @(Get-ChildItem -LiteralPath (Join-Path $RepoRoot '.github/workflows') -Filter '*.yml' -File)
    # tools/devcheck 自己持有这张「禁止出现的模式」清单，扫它等于自己报自己，
    # 所以只扫真正会参与构建 / 发布的脚本。
    $scan += @(Get-ChildItem -LiteralPath (Join-Path $RepoRoot 'tools') -Recurse -Filter '*.ps1' -File |
        Where-Object { $_.FullName -notmatch '[\\/](node_modules|target|gen|_selftest|devcheck)[\\/]' })
    $rootBuild = Join-Path $RepoRoot 'build.ps1'
    if (Test-Path -LiteralPath $rootBuild) { $scan += @(Get-Item -LiteralPath $rootBuild) }

    $badPatterns = @(
        'YuehaiTeam', 'kachina-installer\.git', 'releases/download', 'release-downloader',
        'git\s+clone', 'git\s+submodule', 'Invoke-WebRequest', 'Invoke-RestMethod',
        'DownloadFile', 'curl\s', 'wget\s'
    )
    $inBlockComment = $false
    foreach ($f in $scan) {
        $lineNo = 0
        foreach ($line in [System.IO.File]::ReadLines($f.FullName)) {
            $lineNo++
            $t = $line.Trim()
            if ($t -match '^<#' -or $inBlockComment) {
                $inBlockComment = -not ($t -match '#>')
                continue
            }
            if ($t.StartsWith('#')) { continue }
            foreach ($pat in $badPatterns) {
                if ($t -match $pat) {
                    throw "$($f.Name):$lineNo 出现从外部拉取的语句 [$pat]: $t"
                }
            }
        }
    }
    $notes.Add("$($scan.Count) 个工作流/脚本无外部拉取")

    # 4) build.yml 必须真的走「源码构建」这条路，并把产物作为本仓库的产物上传
    $buildYml = [System.IO.File]::ReadAllText((Join-Path $RepoRoot '.github/workflows/build.yml'))
    if ($buildYml -notmatch 'build\.ps1') {
        throw 'build.yml 没有调用 build.ps1 —— 必须从仓库内源码构建'
    }
    if ($buildYml -notmatch [regex]::Escape('tools/kirara-builder.exe')) {
        throw 'build.yml 没有上传 tools/kirara-builder.exe —— 产物路径与 build.ps1 不一致'
    }
    $notes.Add('CI 从源码构建并上传产物')

    # 5) kachina 的 git 依赖必须在 Cargo.lock 里锁到 commit，且不得指向上游
    $cargoToml = [System.IO.File]::ReadAllText((Join-Path $RepoRoot 'src-tauri/Cargo.toml'))
    $cargoLock = [System.IO.File]::ReadAllText((Join-Path $RepoRoot 'src-tauri/Cargo.lock'))
    # 剥掉整行注释再扫：Cargo.toml 里的注释会写「原为 git = ".../rcedit-rs.git"」这类
    # 说明文字，不剥掉就会被当成真依赖，然后因为在 Cargo.lock 里找不到而误报。
    $cargoTomlCode = (([System.IO.File]::ReadAllLines((Join-Path $RepoRoot 'src-tauri/Cargo.toml'))) |
        Where-Object { -not $_.Trim().StartsWith('#') }) -join "`n"
    $gitDeps = @([regex]::Matches($cargoTomlCode, 'git\s*=\s*"([^"]+)"') |
        ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique)
    foreach ($g in $gitDeps) {
        if ($g -match 'YuehaiTeam') { throw "kachina 的 cargo 依赖指向上游仓库: $g" }
        $esc = [regex]::Escape($g)
        if ($cargoLock -notmatch "source = `"git\+$esc[^`"]*#[0-9a-f]{40}") {
            throw "git 依赖没有在 Cargo.lock 里锁定 commit（CI 可能拉到漂移的分支）: $g"
        }
    }
    $notes.Add("$($gitDeps.Count) 个 git 依赖已锁 commit")

    # 6) npm 依赖里不许有 git/http/file/link 形式（只能是 registry 版本）
    $pkg = Get-Content -LiteralPath (Join-Path $RepoRoot 'package.json') -Raw | ConvertFrom-Json
    foreach ($section in @('dependencies', 'devDependencies')) {
        $node = $pkg.$section
        if (-not $node) { continue }
        foreach ($prop in $node.PSObject.Properties) {
            if ($prop.Value -match '^(git|git\+|https?|github|file|link|workspace):' -or
                $prop.Value -match 'YuehaiTeam') {
                throw "kachina 的 npm 依赖 $($prop.Name) 不是 registry 版本: $($prop.Value)"
            }
        }
    }
    $notes.Add('npm 依赖全部来自 registry')

    # 7) rcedit-rs 的 vendored 副本：kachina 唯一需要 C++ 编译器的依赖。
    #    为什么不用 git 依赖、与上游差在哪、怎么升级：副本目录里的 LOCAL_PATCHES.md。
    $rc = 'vendor/rcedit-rs'
    $rcRequired = @(
        "$rc/Cargo.toml", "$rc/LICENSE", "$rc/LICENSE.rcedit", "$rc/src/lib.rs",
        "$rc/rcedit-sys/Cargo.toml", "$rc/rcedit-sys/build.rs", "$rc/rcedit-sys/src/lib.rs",
        "$rc/rcedit-sys/src/rescle.cc", "$rc/rcedit-sys/src/rescle.h",
        "$rc/rcedit-sys/src/librcedit.cpp"
    )
    foreach ($r in $rcRequired) {
        if (-not (Test-Path -LiteralPath (Join-Path $RepoRoot $r))) { throw "rcedit-rs vendored 副本不完整，缺 $r" }
    }
    # 只看可执行代码：rescle.cc 里解释这处修改的注释本身就会写出 locale::empty()，
    # 不剥掉 // 注释会自己误报自己（跟上面扫工作流时跳过 # 注释是同一个道理）。
    $rescle = (([System.IO.File]::ReadAllLines((Join-Path $RepoRoot "$rc/rcedit-sys/src/rescle.cc"))) |
        ForEach-Object { ($_ -replace '//.*$', '') }) -join "`n"
    if ($rescle -match 'locale::empty\s*\(') {
        throw 'rescle.cc 用回了 std::locale::empty() —— MSVC 14.51(VS 2026) 起已移除，windows-latest 上必然 error C2039'
    }
    if ($cargoTomlCode -match '(?m)^\s*rcedit\s*=\s*\{[^\n]*\bgit\s*=') {
        throw 'kachina 的 rcedit 依赖又指回 git 上游了 —— 必须用 vendor/rcedit-rs 的副本'
    }
    if ($cargoTomlCode -notmatch '(?m)^\s*rcedit\s*=\s*\{[^\n]*path\s*=') {
        throw 'kachina 的 rcedit 依赖不是 path 形式（应指向 ../vendor/rcedit-rs）'
    }
    if ($cargoLock -match 'source = "git\+https://github\.com/Devolutions/rcedit-rs') {
        throw 'Cargo.lock 里 rcedit 仍然是 git 来源 —— 应随 path 依赖一起更新'
    }
    $notes.Add('rcedit-rs 已 vendored(含 MSVC 14.51 修复)')

    # 8) 遥测已物理移除，不许回归。
    #    上游安装器有两条外发通道：Rust 侧把 panic / anyhow 错误连环境信息一起上报到
    #    Sentry（DSN 主机 steambird.cocogoat.cn），前端 sendInsight() 往 77.cocogoat.cn
    #    POST 安装/升级/卸载/启动事件（带屏幕分辨率、语言、来源 id）。本项目是个人自用
    #    构建，两条通道连依赖一起拔掉了（LOCAL_PATCHES.md 第 7 节）。
    #    扫描口径与上面几处一致：只扫可执行内容（行注释剥掉），.md 完全不扫 ——
    #    LOCAL_PATCHES.md 与 devcheck/README.md 需要能把这件事写清楚。
    $ka = $RepoRoot

    # 8a) Sentry 的 Rust 入口文件不许回来
    if (Test-Path -LiteralPath (Join-Path $ka 'src-tauri/src/utils/sentry.rs')) {
        throw '上游的 src-tauri/src/utils/sentry.rs 又出现了 —— 本项目已物理移除 Sentry 上报'
    }

    # 8b) 依赖清单三处：Cargo.toml / package.json / pnpm-workspace.yaml + 两个 lock
    if ($cargoTomlCode -match '(?m)^\s*(sentry|sentry-tracing|sentry-anyhow|whoami)\s*=') {
        throw 'kachina 的 Cargo.toml 又声明了 Sentry / whoami 依赖（遥测已移除）'
    }
    if ($cargoLock -match '(?m)^name = "(sentry[^"]*|whoami|hostname|os_info|debugid)"') {
        throw 'Cargo.lock 里还锁着 Sentry 相关 crate —— 改完依赖要重新生成 Cargo.lock（cargo metadata）'
    }
    foreach ($section in @('dependencies', 'devDependencies')) {
        $node = $pkg.$section
        if (-not $node) { continue }
        foreach ($prop in $node.PSObject.Properties) {
            if ($prop.Name -eq 'sentry' -or $prop.Name.StartsWith('@sentry/')) {
                throw "kachina 的 package.json 又声明了遥测依赖 $($prop.Name)"
            }
        }
    }
    foreach ($lf in @('pnpm-lock.yaml', 'pnpm-workspace.yaml')) {
        $txt = [System.IO.File]::ReadAllText((Join-Path $ka $lf))
        if ($txt -match '@sentry/') {
            throw "$lf 里还有 @sentry/ 条目 —— package.json 改完要重新生成 lock（pnpm install --lockfile-only）"
        }
    }

    # 8c) 源码里不许有活的遥测调用（Rust + 前端一起扫，行注释剥掉）
    $codeFiles = @(Get-ChildItem -LiteralPath (Join-Path $ka 'src-tauri/src') -Recurse -Filter '*.rs' -File)
    $codeFiles += @(Get-ChildItem -LiteralPath (Join-Path $ka 'src') -Recurse -File |
        Where-Object { @('.ts', '.vue', '.js') -contains $_.Extension })
    $telemetryTokens = @(
        'sentry::', 'sentry_tracing', 'capture_anyhow', 'add_breadcrumb',
        'start_transaction', 'configure_scope', 'sendInsight', 'getInsightBase',
        'evCache'
    )
    foreach ($f in $codeFiles) {
        $stripped = (([System.IO.File]::ReadAllLines($f.FullName)) |
            ForEach-Object { ($_ -replace '//.*$', '') }) -join "`n"
        foreach ($t in $telemetryTokens) {
            if ($stripped.Contains($t)) {
                $rel = $f.FullName.Substring($RepoRoot.Length + 1)
                throw "$rel 里出现了遥测调用 [$t] —— 本项目不外发任何统计/错误上报"
            }
        }
    }

    # 8d) 兜底：整棵 vendored 树的文本文件里不许再出现上报域名（DSN 与事件端点都带它）
    $textExt = @('.rs', '.ts', '.vue', '.js', '.json', '.toml', '.yaml', '.yml',
                 '.html', '.css', '.lock', '.txt', '.ps1', '.mjs', '.cjs')
    $sweep = @(Get-ChildItem -LiteralPath $ka -Recurse -File |
        Where-Object {
            # tools/devcheck 与 .github 里写的是「哪些东西已被移除」，不是活代码
            $_.FullName -notmatch '[\\/](node_modules|target|dist|\.git|devcheck|\.github)[\\/]' -and
            @($textExt) -contains $_.Extension -and $_.Length -lt 4MB
        })
    $hit = @($sweep | Select-String -Pattern 'cocogoat' -SimpleMatch -List)
    if ($hit.Count) {
        $where = ($hit | ForEach-Object { "$($_.Path.Substring($RepoRoot.Length + 1)):$($_.LineNumber)" }) -join '、'
        throw "$where 出现上报域名 cocogoat —— Sentry DSN 与统计端点都已移除"
    }
    $notes.Add("遥测已物理移除($($codeFiles.Count) 个源文件 + $($sweep.Count) 个文本文件无 Sentry/cocogoat)")

    # 9) ARP 的卸载命令行只能用 cli/arg.rs 里**真的声明过**的选项
    #    kachina 本体不在 devcheck 的编译范围内，clap 的长/短名写错没有任何一层会发现：
    #    传一个不存在的 `--uninstall`，clap 直接以退出码 2 报「unexpected argument」，
    #    静默卸载一步都不跑 —— 而 ARP 里那个值看起来是「存在」的，比没写更难查。
    $argRs = [System.IO.File]::ReadAllText((Join-Path $ka 'src-tauri/src/cli/arg.rs'))
    $regRs = [System.IO.File]::ReadAllText((Join-Path $ka 'src-tauri/src/installer/registry.rs'))
    $declaredShort = @([regex]::Matches($argRs, "short\s*=\s*'([A-Za-z])'") |
        ForEach-Object { $_.Groups[1].Value })
    # 显式写了名字的长选项
    $declaredLong = @([regex]::Matches($argRs, 'long\s*=\s*"([a-z0-9\-]+)"') |
        ForEach-Object { $_.Groups[1].Value })
    # 只写了 `#[clap(long, …)]` 的：clap 拿字段名当长名（下划线转连字符）
    $declaredLong += @([regex]::Matches($argRs,
        '(?m)^\s*#\s*\[\s*clap\s*\([^)]*\blong\b[^)]*\)\s*\]\s*\r?\n\s*pub\s+([a-z0-9_]+)\s*:') |
        ForEach-Object { $_.Groups[1].Value -replace '_', '-' })
    if ($declaredShort.Count -eq 0) {
        throw 'cli/arg.rs 里一个 short 选项都没解析到 —— 本组断言的解析规则失效了'
    }

    # 9a) UninstallString 必须是「整条命令被引号包住的路径」（默认装在 Program Files 下，
    #     不加引号时「应用和功能」会按第一个空格把命令截断成 C:\Program）
    $plain = [regex]::Match($regRs,
        '(?<!Quiet)UninstallString"\s*,\s*&?format!\(\s*"((?:[^"\\]|\\.)*)"')
    if (-not $plain.Success) { throw 'registry.rs 里找不到 UninstallString 的写入' }
    if ($plain.Groups[1].Value -notmatch '^\\".*\\"\s*$') {
        throw "ARP 的 UninstallString 没有整体加引号（实际写法：$($plain.Groups[1].Value)）—— 路径含空格时会被截断"
    }

    # 9b) QuietUninstallString 的每个选项都要在 arg.rs 里存在
    $quiet = [regex]::Match($regRs,
        'QuietUninstallString"\s*,\s*\r?\n?\s*&format!\(\s*"((?:[^"\\]|\\.)*)"')
    if (-not $quiet.Success) { throw 'registry.rs 里找不到 QuietUninstallString 的写入' }
    $cmdline = $quiet.Groups[1].Value
    $used = @([regex]::Matches($cmdline, '(?<![\w-])(--?[A-Za-z][\w-]*)') |
        ForEach-Object { $_.Groups[1].Value })
    if ($used.Count -eq 0) {
        throw 'QuietUninstallString 一个选项都没有 —— 那它跟 UninstallString 没区别，静默卸载会弹界面'
    }
    foreach ($u in $used) {
        if ($u.StartsWith('--')) {
            if ($declaredLong -notcontains $u.Substring(2)) {
                throw "QuietUninstallString 用了 $u，但 cli/arg.rs 没声明这个长选项（clap 会以退出码 2 报错，静默卸载不会跑）"
            }
        } elseif ($declaredShort -notcontains $u.Substring(1)) {
            throw "QuietUninstallString 用了 $u，但 cli/arg.rs 没声明这个短选项（clap 会以退出码 2 报错，静默卸载不会跑）"
        }
    }
    $notes.Add("ARP 卸载命令行与 cli/arg.rs 一致(UninstallString 已加引号；Quiet=$($used -join ' '))")

    # 10) 停更 / 无保障的依赖不许回归。
    #     第 14～16 节的重构把四个「没人维护」的依赖换成了系统 API 或标准 crates：
    #     mslnk 0.1（2022 年后停更，自带约 1300 行手写 .lnk 序列化）→ IShellLinkW +
    #     IPersistFile；nt_version 0.1（2020 年后停更）→ ntdll 的
    #     RtlGetNtVersionNumbers；h3-msquic-async + xytoki/msquic-async-rs fork
    #     （下载量千级、构建要编上千个 C 文件）→ quinn + rustls；xytoki/zip2 fork
    #     → crates.io 的 zip + 调用侧的 decode_entry_name。换回去等于把这些风险请回来。
    foreach ($stale in @('mslnk', 'nt_version')) {
        if ($cargoTomlCode -match ('(?m)^\s*' + $stale + '\s*=')) {
            throw "kachina 的 Cargo.toml 又声明了 $stale —— 已换成系统 API，见 LOCAL_PATCHES.md 第 16 节"
        }
        if ($cargoLock -match ('(?m)^name = "' + $stale + '"')) {
            throw "Cargo.lock 里还锁着 $stale —— 改完依赖要重新生成 Cargo.lock（cargo metadata）"
        }
    }
    if ($cargoTomlCode -match '(?m)^\s*(h3-)?msquic-async\s*=') {
        throw 'kachina 又用回了 msquic 系（上游 fork 分支依赖）—— H3 传输层应保持 quinn + rustls'
    }
    if ($cargoLock -match '(?m)^name = "[^"]*msquic') {
        throw 'Cargo.lock 里还有 msquic 系 crate —— 改完依赖要重新生成 Cargo.lock（cargo metadata）'
    }
    if ($cargoTomlCode -match 'xytoki/zip2' -or $cargoLock -match 'xytoki/zip2') {
        throw 'zip 又指回 xytoki/zip2 fork —— 强制 UTF-8 的条目名解码已由 thirdparty/mirrorc.rs 复刻'
    }
    $notes.Add('停更依赖未回归(mslnk/nt_version/msquic 系/zip2 fork)')

    # 10b) vendored 的 HDiffPatch 源码（libs/hdiff-sys、libs/hpatch-sys）是 MIT，
    #      许可证必须随源码一起分发；出处与本地差异记在 libs/THIRDPARTY.md。
    $libDocs = @(
        'src-tauri/libs/THIRDPARTY.md',
        'src-tauri/libs/hdiff-sys/LICENSE',
        'src-tauri/libs/hpatch-sys/LICENSE'
    )
    foreach ($r in $libDocs) {
        if (-not (Test-Path -LiteralPath (Join-Path $RepoRoot $r))) {
            throw "libs 下的 vendored 第三方源码缺 $r —— MIT 许可证必须随源码分发"
        }
    }
    $notes.Add('libs 下 vendored 源码带齐 LICENSE 与出处说明')

    # 11) C10：Tauri 宿主替换后不许回归。宿主由 Win32 窗口 + WebView2 controller
    #     直接实现，前端资源由 build.rs 压缩进 exe；把 Tauri 依赖或旧桥接文件加回来
    #     会让体积、IPC 形状和窗口生命周期同时回到旧架构。
    if ($cargoTomlCode -match '(?m)^\s*(tauri|tauri-build|tauri-utils)\s*=') {
        throw 'kachina 的 Cargo.toml 又声明了 Tauri 依赖 —— C10 已换成原生 Win32 + WebView2 宿主'
    }
    if ($cargoLock -match '(?m)^name = "(tauri|tauri-build|tauri-utils|wry)"') {
        throw 'Cargo.lock 里又出现 Tauri / wry —— 改完依赖要重新生成 Cargo.lock（cargo metadata）'
    }
    foreach ($section in @('dependencies', 'devDependencies')) {
        $node = $pkg.$section
        if (-not $node) { continue }
        foreach ($prop in $node.PSObject.Properties) {
            if ($prop.Name -eq '@tauri-apps/api' -or $prop.Name -eq '@tauri-apps/cli') {
                throw "package.json 又声明了 $($prop.Name) —— 原生宿主不依赖 Tauri JS API"
            }
        }
    }
    $pnpmLockText = [System.IO.File]::ReadAllText((Join-Path $RepoRoot 'pnpm-lock.yaml'))
    if ($pnpmLockText -match '@tauri-apps/') {
        throw 'pnpm-lock.yaml 里还有 @tauri-apps/ 条目 —— package.json 改完要重新生成 lock'
    }
    foreach ($r in @(
            'src/host.ts',
            'src-tauri/src/host/mod.rs',
            'src-tauri/src/host/assets.rs',
            'src-tauri/src/host/bridge.rs',
            'src-tauri/src/host/webview.rs',
            'src-tauri/src/host/window.rs'
        )) {
        if (-not (Test-Path -LiteralPath (Join-Path $RepoRoot $r))) {
            throw "原生宿主文件缺失：$r"
        }
    }
    if (Test-Path -LiteralPath (Join-Path $RepoRoot 'src/tauri.ts')) {
        throw 'src/tauri.ts 又出现了 —— 前端 IPC 应走 src/host.ts 的 WebView2 桥'
    }
    if (Test-Path -LiteralPath (Join-Path $RepoRoot 'src-tauri/tauri.conf.json')) {
        throw 'src-tauri/tauri.conf.json 又出现了 —— C10 已删除 Tauri 配置'
    }
    $rsbuild = [System.IO.File]::ReadAllText((Join-Path $RepoRoot 'rsbuild.config.ts'))
    foreach ($token in @('inlineScripts: true', 'inlineStyles: true', "strategy: 'all-in-one'")) {
        if ($rsbuild -notmatch [regex]::Escape($token)) {
            throw "rsbuild.config.ts 缺少「$token」—— 前端必须保持单文件内联产物"
        }
    }
    $notes.Add('Tauri 宿主未回归(原生 host / 单文件前端 / 无 @tauri-apps)')

    return ($notes -join '；')
}

function Test-Ps1Syntax {
    $files = @(Get-ChildItem -Path $RepoRoot -Recurse -Filter '*.ps1' -File |
        Where-Object { $_.FullName -notmatch '[\\/](node_modules|target|dist|bin|obj|gen|\.git)[\\/]' })
    $bad = 0
    foreach ($f in $files) {
        $tokens = $null; $errs = $null
        [System.Management.Automation.Language.Parser]::ParseFile($f.FullName, [ref]$tokens, [ref]$errs) | Out-Null
        if ($errs -and $errs.Count) {
            $bad++
            Write-Bad $f.FullName.Substring($RepoRoot.Length + 1)
            foreach ($e in $errs) { Write-Bad "   line $($e.Extent.StartLineNumber): $($e.Message)" }
        }
    }
    if ($bad) { throw "$bad / $($files.Count) 个 .ps1 有语法错误" }
    return "$($files.Count) 个 .ps1 语法通过"
}

function New-GenSources {
    $a = @(New-TypecheckGen -RepoRoot $RepoRoot -DevCheckRoot $DevCheckRoot)
    $b = @(New-LogicGen -RepoRoot $RepoRoot -DevCheckRoot $DevCheckRoot)

    # front：把要类型检查的 TS 原样搬到 front/gen/src（保持相对 import 结构）
    $frontGen = Join-Path $DevCheckRoot 'front/gen/src'
    New-Item -ItemType Directory -Path (Join-Path $frontGen 'utils') -Force | Out-Null
    Copy-Item (Join-Path $RepoRoot 'src/types.ts') (Join-Path $frontGen 'types.ts') -Force
    Copy-Item (Join-Path $RepoRoot 'src/utils/agreement.ts') (Join-Path $frontGen 'utils/agreement.ts') -Force
    # prettier 要用与 kachina 相同的配置，否则会按默认风格误报
    $rc = Join-Path $RepoRoot '.prettierrc'
    if (Test-Path -LiteralPath $rc) { Copy-Item $rc (Join-Path $DevCheckRoot 'front/.prettierrc') -Force }

    return "Rust $($a.Count + $b.Count) 个 + TS 2 个"
}

function Test-RustTypecheck {
    $cargo = Get-Tool 'cargo'
    if (-not $cargo) { Skip-Layer 'cargo 不在 PATH（https://rustup.rs）' }
    $target = 'x86_64-pc-windows-msvc'

    $rustup = Get-Tool 'rustup'
    if ($rustup) {
        $installed = ((& $rustup target list --installed) -join "`n")
        if ($installed -notmatch [regex]::Escape($target)) {
            if ($SkipInstall) { Skip-Layer "缺少 rustup target $target（去掉 -SkipInstall 可自动装）" }
            Write-Info "安装 rustup target $target …"
            & $rustup target add $target | Out-Null
        }
    }

    $r = Invoke-Native -FilePath $cargo `
        -Arguments @('check', '--target', $target, '--message-format', 'short') `
        -WorkingDirectory (Join-Path $DevCheckRoot 'rust/typecheck') -Tail 60
    if ($r.ExitCode -ne 0) { throw 'cargo check（x86_64-pc-windows-msvc）失败，见上方输出' }
    $warn = ([regex]::Matches($r.Output, 'warning:')).Count
    $what = if ($warn) { "（$warn 条 warning）" } else { '，0 warning' }
    return "uninstall.rs + lnk.rs + utils/{error,dir,os_version}.rs 在 $target 上类型检查通过$what"
}

function Test-RustLogic {
    $cargo = Get-Tool 'cargo'
    if (-not $cargo) { Skip-Layer 'cargo 不在 PATH（https://rustup.rs）' }
    $r = Invoke-Native -FilePath $cargo -Arguments @('run', '--quiet') `
        -WorkingDirectory (Join-Path $DevCheckRoot 'rust/logic') -Tail 90
    if ($r.ExitCode -ne 0) { throw '行为断言失败，见上方输出' }
    $summary = ($r.Output -split "`r?`n" | Where-Object { $_ -match '====' } | Select-Object -Last 1)
    if (-not $summary) { $summary = '断言全部通过' }
    return $summary.Trim()
}

function Test-NativeDeps {
    # vendored 的 rcedit-sys 带 C++（rescle.cc / librcedit.cpp），只能靠 MSVC 编。
    # 这一层在有 cl.exe 的机器上真编一遍，让工具链/C++ 侧的变化在自动跑的 Devcheck
    # 里就暴露，不必等手动触发 Build。踩过的具体那一次：vendor 目录的 LOCAL_PATCHES.md。
    $crate = Join-Path $RepoRoot 'vendor/rcedit-rs/rcedit-sys'
    if (-not (Test-Path -LiteralPath (Join-Path $crate 'Cargo.toml'))) {
        throw "找不到 $crate（vendored 副本被删了？）"
    }
    $cargo = Get-Tool 'cargo'
    if (-not $cargo) { Skip-Layer 'cargo 不在 PATH' }
    if (-not (Get-Tool 'cl')) { Skip-Layer 'cl.exe 不在 PATH（需要 Windows + MSVC 开发环境）' }

    # 独立 target 目录：既不污染 kachina 自己的构建产物，也不打乱 CI 的 cargo 缓存
    $prev = $env:CARGO_TARGET_DIR
    $env:CARGO_TARGET_DIR = (Join-Path $DevCheckRoot 'rust/native/target')
    try {
        $r = Invoke-Native -FilePath $cargo `
            -Arguments @('build', '--manifest-path', (Join-Path $crate 'Cargo.toml')) `
            -WorkingDirectory $crate -Tail 25
    }
    finally {
        if ($null -eq $prev) { Remove-Item Env:\CARGO_TARGET_DIR -ErrorAction SilentlyContinue }
        else { $env:CARGO_TARGET_DIR = $prev }
    }
    if ($r.ExitCode -ne 0) { throw 'rcedit-sys 编译失败（C++ 或 Rust 侧，详见上面的 cl.exe / cargo 输出）' }
    return 'vendored rcedit-sys 的 rescle.cc + librcedit.cpp 用 MSVC 编译通过'
}

function Test-Frontend {
    $node = Get-Tool 'node'
    if (-not $node) { Skip-Layer 'node 不在 PATH' }
    $npm = Get-Tool 'npm'
    if (-not $npm) { Skip-Layer 'npm 不在 PATH' }

    $front = Join-Path $DevCheckRoot 'front'
    if (-not (Test-Path (Join-Path $front 'node_modules'))) {
        if ($SkipInstall) { Skip-Layer 'front/node_modules 不存在（去掉 -SkipInstall 可自动 npm install）' }
        Write-Info '首次运行：安装前端最小依赖（typescript/dompurify/vue/@vue/compiler-sfc/prettier）…'
        $r = Invoke-Native -FilePath $npm -Arguments @('install', '--no-audit', '--no-fund', '--prefer-offline') `
            -WorkingDirectory $front -Tail 10
        if ($r.ExitCode -ne 0) { throw 'npm install 失败' }
    }

    $npx = Get-Tool 'npx'
    if (-not $npx) {
        $ext = if ($script:IsWin) { '.cmd' } else { '' }
        $npx = Join-Path (Split-Path -Parent $npm) "npx$ext"
    }

    $checks = @(
        @{ What = 'tsc --noEmit（agreement.ts / types.ts）'; Exe = $npx; Args = @('tsc', '--noEmit', '-p', 'tsconfig.json') }
        @{ What = '.vue 单文件组件编译'; Exe = $node; Args = @('sfccheck.mjs') }
        # 只查我们自己写的文件（types.ts / App.vue 是上游代码，本身不符合仓库的 prettier 风格）。
        # --end-of-line auto：换行统一由 .gitattributes 管，别让旧工作区的 CRLF 淹掉真问题，
        # 详见 README.md「跨平台的坑」。
        @{ What = 'prettier --check'; Exe = $npx; Args = @('prettier', '--check', '--end-of-line', 'auto', 'gen/src/utils/agreement.ts') }
    )
    foreach ($c in $checks) {
        $r = Invoke-Native -FilePath $c.Exe -Arguments $c.Args -WorkingDirectory $front -Tail 30
        if ($r.ExitCode -ne 0) { throw "$($c.What) 失败" }
    }
    return 'tsc --strict / 全部 .vue SFC / prettier 均通过'
}
