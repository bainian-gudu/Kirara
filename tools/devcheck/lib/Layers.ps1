# devcheck 各层实现（被 devcheck.ps1 dot-source）
#
# 每个 Test-* 函数对应一个层：返回字符串 = 通过的说明，抛异常 = 失败。
# 依赖 devcheck.ps1 的 $RepoRoot / $DevCheckRoot / $SkipInstall 与 lib/Common.ps1 的
# 助手函数。本仓库的根目录就是安装器源码（上游快照 + 本地补丁），所以源码路径不带前缀。

# 取某个顶层 job 的正文。工作流里「哪一步在哪个 job 里」是要断言的（dump 用例必须
# 用 debug 产物、必须在 unit-test 里跑），整文件 -match 分不出来。
function Get-WorkflowJobText {
    param([Parameter(Mandatory)][string]$Workflow, [Parameter(Mandatory)][string]$Job)
    # 顶层 job 名缩进两格；正文到下一个两格缩进的键为止。
    $m = [regex]::Match($Workflow,
        "(?ms)^  $([regex]::Escape($Job)):[ \t]*\r?\n(?<body>.*?)(?=^  [A-Za-z_]|\z)")
    if (-not $m.Success) { throw "工作流里找不到 job $Job" }
    return $m.Groups['body'].Value
}

function Test-VendoredSource {
    # 源码必须是「仓库内的快照」：CI 与脚本都不许从上游拉源码或下二进制，只能从本
    # 仓库构建。同时把「已经移除的东西不许回来」钉住（遥测、停更依赖、git 形式的
    # rcedit），以及写进注册表的卸载命令行必须真的能被自己的 CLI 解析。
    $notes = [System.Collections.Generic.List[string]]::new()
    $ka = $RepoRoot

    # 1) 不是 submodule
    if (Test-Path -LiteralPath (Join-Path $ka '.gitmodules')) {
        throw '存在 .gitmodules —— 源码必须是仓库内的快照，不是 submodule'
    }

    # 2) 快照完整（缺一个就说明 vendored 源码被误删，CI 会退化成「去别处找」）
    $required = @(
        'Cargo.toml',
        'Cargo.lock',
        'package.json',
        'pnpm-lock.yaml',
        'build.ps1',
        'scripts/merge-release-bundle.mjs',
        'native/main.rs',
        'native/builder/pack.rs',
        'native/installer/uninstall.rs',
        'native/session/plan.rs',
        'native/utils/config_keys.rs'
    )
    foreach ($r in $required) {
        if (-not (Test-Path -LiteralPath (Join-Path $ka $r))) { throw "源码快照不完整，缺 $r" }
    }
    $notes.Add("快照完整($($required.Count) 个关键文件)")

    # 3) 工作流与脚本里不许出现「从外部拉源码/下二进制」的动作。
    #    只扫会参与构建 / 发布的脚本：tests/ 下的用例本来就要下载安装包，不在此列。
    $scan = @(Get-ChildItem -LiteralPath (Join-Path $ka '.github/workflows') -Filter '*.yml' -File)
    $scan += @(Get-ChildItem -LiteralPath (Join-Path $ka 'scripts') -Filter '*.mjs' -File -ErrorAction SilentlyContinue)
    $rootBuild = Join-Path $ka 'build.ps1'
    if (Test-Path -LiteralPath $rootBuild) { $scan += @(Get-Item -LiteralPath $rootBuild) }

    $badPatterns = @(
        'YuehaiTeam', 'kachina-installer\.git', 'releases/download', 'release-downloader',
        'git\s+clone', 'git\s+submodule', 'Invoke-WebRequest', 'Invoke-RestMethod',
        'DownloadFile', 'curl\s', 'wget\s'
    )
    # 唯一放行的「外部地址」：p6-e2e.yml 的 workflow_dispatch 输入默认值 —— 那是操作者
    # 手动触发时喂给 tests/p6-e2e.mjs 的冻结旧安装包地址（测试夹具输入），真正的下载
    # 发生在测试脚本里，不是构建期 / 发布期的动作。按内容精确放行，不做目录级豁免。
    $allowances = @{
        '^default:\s*https://github\.com/[\w.\-]+/[\w.\-]+/releases/download/\S+$' =
            'p6-e2e.yml 的旧安装包地址输入（测试夹具，见 tests/p6-e2e.mjs）'
    }
    $usedAllowance = @{}
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
            $allowed = $false
            foreach ($key in $allowances.Keys) {
                if ($t -match $key) { $usedAllowance[$key] = $true; $allowed = $true; break }
            }
            if ($allowed) { continue }
            foreach ($pat in $badPatterns) {
                if ($t -match $pat) {
                    throw "$($f.Name):$lineNo 出现从外部拉取的语句 [$pat]: $t"
                }
            }
        }
    }
    # 放行规则匹配不上就报错：否则上游改了默认值的写法后，这条豁免会静默变成
    # 「什么都没放行」，而下一次真出现外部拉取时又会被别的规则漏掉。
    foreach ($key in $allowances.Keys) {
        if (-not $usedAllowance.ContainsKey($key)) {
            throw "vendor 层的放行规则没有匹配到任何一行，已失效，请同步检查：$($allowances[$key])"
        }
    }
    $notes.Add("$($scan.Count) 个工作流/脚本无外部拉取")

    # 4) build.yml 必须真的走「源码构建 → 拼接 → 交付」这条路
    $buildYml = [System.IO.File]::ReadAllText((Join-Path $ka '.github/workflows/build.yml'))
    if ($buildYml -notmatch 'pnpm build') {
        throw 'build.yml 没有调用 pnpm build —— 必须从仓库内源码构建'
    }
    if ($buildYml -notmatch 'is not larger than cargo builder') {
        throw 'build.yml 没有断言「拼接体大于 cargo 裸 builder」—— installer 没拼进去也能过'
    }
    if ($buildYml -notmatch [regex]::Escape('kirara-builder.exe')) {
        throw 'build.yml 没有交付 kirara-builder.exe —— 交付名与 build.ps1 / 下游固定引用不一致'
    }
    if ($buildYml -notmatch 'uses:\s*actions/upload-artifact@') {
        throw 'build.yml 没有上传构建产物'
    }
    # 会话 dump 的编译在 cfg(debug_assertions) 后面，只有 debug 产物写得出来；混进跑
    # release 产物的 test 矩阵会静默跳过「dump 与 planner 对比」。
    $unitTest = Get-WorkflowJobText -Workflow $buildYml -Job 'unit-test'
    foreach ($needle in @('cargo build', 'test:dump-offline-install', 'compare_offline_install_dump_if_present')) {
        if ($unitTest -notmatch [regex]::Escape($needle)) {
            throw "build.yml 的 unit-test job 里缺少「$needle」—— 离线安装计划 dump 与对比会静默失效"
        }
    }
    $notes.Add('CI 从源码构建、断言拼接体并交付 kirara-builder.exe')

    # 5) cargo 的 git 依赖必须在 Cargo.lock 里锁到 commit
    #    注释会写出「原为 git = "..."」这类说明，不剥掉就会被当成真依赖。
    $cargoTomlCode = (([System.IO.File]::ReadAllLines((Join-Path $ka 'Cargo.toml'))) |
        Where-Object { -not $_.Trim().StartsWith('#') }) -join "`n"
    $cargoLock = [System.IO.File]::ReadAllText((Join-Path $ka 'Cargo.lock'))
    $gitDeps = @([regex]::Matches($cargoTomlCode, 'git\s*=\s*"([^"]+)"') |
        ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique)
    if ($gitDeps.Count -eq 0) {
        throw 'Cargo.toml 里一个 git 依赖都没解析到 —— 本组断言的解析规则失效了'
    }
    foreach ($g in $gitDeps) {
        if ($g -match 'YuehaiTeam|kachina-installer') { throw "cargo 依赖指向上游仓库: $g" }
        $esc = [regex]::Escape($g)
        if ($cargoLock -notmatch "source = `"git\+$esc[^`"]*#[0-9a-f]{40}") {
            throw "git 依赖没有在 Cargo.lock 里锁定 commit（CI 可能拉到漂移的分支）: $g"
        }
    }
    $notes.Add("$($gitDeps.Count) 个 git 依赖已锁 commit")

    # 6) npm 依赖里不许有 git/http/file/link 形式（只能是 registry 版本）
    $pkg = Get-Content -LiteralPath (Join-Path $ka 'package.json') -Raw | ConvertFrom-Json
    foreach ($section in @('dependencies', 'devDependencies')) {
        $sec = $pkg.PSObject.Properties[$section]
        if (-not $sec) { continue }
        foreach ($prop in $sec.Value.PSObject.Properties) {
            if ($prop.Value -match '^(git|git\+|https?|github|file|link|workspace):' -or
                $prop.Value -match 'YuehaiTeam') {
                throw "npm 依赖 $($prop.Name) 不是 registry 版本: $($prop.Value)"
            }
        }
    }
    $notes.Add('npm 依赖全部来自 registry')

    # 7) rcedit-rs 的 vendored 副本：唯一需要 C++ 编译器的依赖。为什么不用 git 依赖、
    #    与上游差在哪、怎么升级：副本目录里的 LOCAL_PATCHES.md。
    $rc = 'vendor/rcedit-rs'
    $rcRequired = @(
        "$rc/Cargo.toml", "$rc/LICENSE", "$rc/LICENSE.rcedit",
        "$rc/LOCAL_PATCHES.md", "$rc/README.md", "$rc/src/lib.rs",
        "$rc/rcedit-sys/Cargo.toml", "$rc/rcedit-sys/build.rs", "$rc/rcedit-sys/src/lib.rs",
        "$rc/rcedit-sys/src/rescle.cc", "$rc/rcedit-sys/src/rescle.h",
        "$rc/rcedit-sys/src/librcedit.cpp"
    )
    foreach ($r in $rcRequired) {
        if (-not (Test-Path -LiteralPath (Join-Path $ka $r))) { throw "rcedit-rs vendored 副本不完整，缺 $r" }
    }
    # 只看可执行代码：rescle.cc 里解释这处修改的注释本身就会写出 locale::empty()。
    $rescle = (([System.IO.File]::ReadAllLines((Join-Path $ka "$rc/rcedit-sys/src/rescle.cc"))) |
        ForEach-Object { ($_ -replace '//.*$', '') }) -join "`n"
    if ($rescle -match 'locale::empty\s*\(') {
        throw 'rescle.cc 用回了 std::locale::empty() —— MSVC 14.51(VS 2026) 起已移除，windows-latest 上必然 error C2039'
    }
    if ($cargoTomlCode -match '(?m)^\s*rcedit\s*=\s*\{[^\n]*\bgit\s*=') {
        throw 'rcedit 依赖又指回 git 上游了 —— 必须用 vendor/rcedit-rs 的副本'
    }
    if ($cargoTomlCode -notmatch '(?m)^\s*rcedit\s*=\s*\{[^\n]*path\s*=') {
        throw 'rcedit 依赖不是 path 形式（应指向 vendor/rcedit-rs）'
    }
    if ($cargoLock -match 'source = "git\+https://github\.com/Devolutions/rcedit-rs') {
        throw 'Cargo.lock 里 rcedit 仍然是 git 来源 —— 应随 path 依赖一起更新'
    }
    $notes.Add('rcedit-rs 已 vendored(含 MSVC 14.51 修复)')

    # 8) 遥测已物理移除，不许回归。
    #    上游有两条外发通道：Rust 侧把 panic / anyhow 错误连环境信息一起上报到 Sentry，
    #    前端把安装 / 升级 / 卸载 / 启动事件 POST 到统计端点。本项目是个人自用构建，
    #    两条通道连依赖一起拔掉了。扫描口径与上面一致：只扫可执行内容（行注释剥掉），
    #    .md 完全不扫 —— 台账与 devcheck 的 README 需要能把这件事写清楚。

    # 8a) Sentry 的 Rust 入口文件不许回来
    if (Test-Path -LiteralPath (Join-Path $ka 'native/utils/sentry.rs')) {
        throw '上游的 native/utils/sentry.rs 又出现了 —— 本项目已物理移除 Sentry 上报'
    }

    # 8b) 依赖清单：Cargo.toml / Cargo.lock / package.json / 两个 pnpm 清单
    # 注意：whoami 现在只是 dev-dependency（session::state 的 icacls 测试要当前
    # 用户名），不再是上报链路的一环，所以只盯 sentry 系本身。
    if ($cargoTomlCode -match '(?m)^\s*(sentry|sentry-tracing|sentry-anyhow)\s*=') {
        throw 'Cargo.toml 又声明了 Sentry 依赖（遥测已移除）'
    }
    if ($cargoLock -match '(?m)^name = "(sentry[^"]*|hostname|os_info|debugid)"') {
        throw 'Cargo.lock 里还锁着 Sentry 相关 crate —— 改完依赖要重新生成 Cargo.lock（cargo metadata）'
    }
    foreach ($section in @('dependencies', 'devDependencies')) {
        $sec = $pkg.PSObject.Properties[$section]
        if (-not $sec) { continue }
        foreach ($prop in $sec.Value.PSObject.Properties) {
            if ($prop.Name -eq 'sentry' -or $prop.Name.StartsWith('@sentry/')) {
                throw "package.json 又声明了遥测依赖 $($prop.Name)"
            }
        }
    }
    foreach ($lf in @('pnpm-lock.yaml', 'pnpm-workspace.yaml')) {
        $txt = [System.IO.File]::ReadAllText((Join-Path $ka $lf))
        if ($txt -match '@sentry/') {
            throw "$lf 里还有 @sentry/ 条目 —— package.json 改完要重新生成 lock（pnpm install --lockfile-only）"
        }
    }

    # 8c) 源码里不许有活的遥测调用（Rust + 前端一起扫，行注释剥掉）。
    #     跳过的目录同 8d：那里是检查工具与代理配置，不是随产物发布的代码。
    $skipCode = @('devcheck', '.github', '.agents')
    $codeFiles = @(Get-RepoFiles -Include '.rs' -SkipDir $skipCode)
    $codeFiles += @(Get-RepoFiles -Include @('.ts', '.tsx', '.js') -SkipDir $skipCode)
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
                throw "$($f.FullName.Substring($RepoRoot.Length + 1)) 里出现了遥测调用 [$t] —— 本项目不外发任何统计/错误上报"
            }
        }
    }

    # 8d) 兜底：整棵树的文本文件里不许再出现上报域名（DSN 与事件端点都带它）。
    #     tools/devcheck、.github、.agents 里写的是「哪些东西已被移除」或代理配置，
    #     不是随产物发布的代码，故跳过。
    $textExt = @('.rs', '.ts', '.tsx', '.js', '.mjs', '.cjs', '.json', '.toml',
                 '.yaml', '.yml', '.html', '.css', '.lock', '.txt', '.ps1')
    $sweep = @(Get-RepoFiles -Include $textExt -SkipDir @('devcheck', '.github', '.agents'))
    $hit = @($sweep | Select-String -Pattern 'cocogoat' -SimpleMatch -List)
    if ($hit.Count) {
        $where = ($hit | ForEach-Object { "$($_.Path.Substring($RepoRoot.Length + 1)):$($_.LineNumber)" }) -join '、'
        throw "$where 出现上报域名 cocogoat —— Sentry DSN 与统计端点都已移除"
    }
    $notes.Add("遥测已物理移除($($codeFiles.Count) 个源文件 + $($sweep.Count) 个文本文件无 Sentry/cocogoat)")

    # 9) ARP 的卸载命令行只能用 native/cli/mod.rs 的 OPTS 里**真的声明过**的选项。
    #    传一个不存在的选项 = CLI 直接报错退出，卸载一步都不跑 —— 而 ARP 里那个值
    #    看起来是「存在」的，比没写更难查。
    $cliRs = [System.IO.File]::ReadAllText((Join-Path $ka 'native/cli/mod.rs'))
    $regRs = [System.IO.File]::ReadAllText((Join-Path $ka 'native/installer/registry.rs'))
    $declaredShort = @([regex]::Matches($cliRs, "short:\s*Some\('([A-Za-z])'\)") |
        ForEach-Object { $_.Groups[1].Value })
    $declaredLong = @([regex]::Matches($cliRs, 'long:\s*Some\("([a-z0-9\-]+)"\)') |
        ForEach-Object { $_.Groups[1].Value })
    if ($declaredShort.Count -eq 0 -or $declaredLong.Count -eq 0) {
        throw 'native/cli/mod.rs 的 OPTS 里没解析出短 / 长选项 —— 本组断言的解析规则失效了'
    }

    # 9a) UninstallString 必须是「整条命令被引号包住的路径」（默认装在 Program Files
    #     下，不加引号时「应用和功能」会按第一个空格把命令截断成 C:\Program）
    $plain = [regex]::Match($regRs,
        '(?<!Quiet)UninstallString"\s*,\s*&?format!\(\s*"((?:[^"\\]|\\.)*)"')
    if (-not $plain.Success) { throw 'registry.rs 里找不到 UninstallString 的写入' }
    if ($plain.Groups[1].Value -notmatch '^\\".*\\"\s*$') {
        throw "ARP 的 UninstallString 没有整体加引号（实际写法：$($plain.Groups[1].Value)）—— 路径含空格时会被截断"
    }

    # 9b) QuietUninstallString 的每个选项都要在 OPTS 里存在，且至少要有一个选项 ——
    #     否则它跟 UninstallString 没区别，静默卸载会弹界面。
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
                throw "QuietUninstallString 用了 $u，但 native/cli/mod.rs 的 OPTS 没声明这个长选项"
            }
        } elseif ($declaredShort -notcontains $u.Substring(1)) {
            throw "QuietUninstallString 用了 $u，但 native/cli/mod.rs 的 OPTS 没声明这个短选项"
        }
    }
    $notes.Add("ARP 卸载命令行与 cli 的 OPTS 一致(UninstallString 已加引号；Quiet=$($used -join ' '))")

    # 10) 停更 / 无保障的依赖不许回归：mslnk / nt_version 换成了系统 API，
    #     msquic 系换成 quinn + rustls，zip 换回 crates.io 的 zip + 自己的条目名解码。
    #     换回去等于把这些风险请回来。
    foreach ($stale in @('mslnk', 'nt_version')) {
        if ($cargoTomlCode -match ('(?m)^\s*' + $stale + '\s*=')) {
            throw "Cargo.toml 又声明了 $stale —— 已换成系统 API，见 MIGRATION_LEDGER.md"
        }
        if ($cargoLock -match ('(?m)^name = "' + $stale + '"')) {
            throw "Cargo.lock 里还锁着 $stale —— 改完依赖要重新生成 Cargo.lock（cargo metadata）"
        }
    }
    if ($cargoTomlCode -match '(?m)^\s*(h3-)?msquic-async\s*=') {
        throw '又用回了 msquic 系（上游 fork 分支依赖）—— H3 传输层应保持 quinn + rustls'
    }
    if ($cargoLock -match '(?m)^name = "[^"]*msquic') {
        throw 'Cargo.lock 里还有 msquic 系 crate —— 改完依赖要重新生成 Cargo.lock（cargo metadata）'
    }
    if ($cargoTomlCode -match 'xytoki/zip2' -or $cargoLock -match 'xytoki/zip2') {
        throw 'zip 又指回 xytoki/zip2 fork —— 强制 UTF-8 的条目名解码已由 native/thirdparty/mirrorc.rs 复刻'
    }
    $notes.Add('停更依赖未回归(mslnk/nt_version/msquic 系/zip2 fork)')

    # 10b) vendored 的 HDiffPatch 源码（libs/hdiff-sys、libs/hpatch-sys）是 MIT，
    #      许可证必须随源码一起分发；出处与本地差异记在 libs/THIRDPARTY.md。
    foreach ($r in @('libs/THIRDPARTY.md', 'libs/hdiff-sys/LICENSE', 'libs/hpatch-sys/LICENSE')) {
        if (-not (Test-Path -LiteralPath (Join-Path $ka $r))) {
            throw "libs 下的 vendored 第三方源码缺 $r —— MIT 许可证必须随源码分发"
        }
    }
    $notes.Add('libs 下 vendored 源码带齐 LICENSE 与出处说明')

    return ($notes -join '；')
}

function Test-Ps1Syntax {
    $files = @(Get-RepoFiles -Include '.ps1')
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
    $a = @(New-LogicGen -RepoRoot $RepoRoot -DevCheckRoot $DevCheckRoot)
    return "生成 $($a.Count) 个文件"
}

function Test-RustTypecheck {
    # 整棵根 crate 在 x86_64-pc-windows-msvc 上编一遍（--all-targets 连单测一起）。
    # native 依赖（libs/hdiff-sys、libs/hpatch-sys、vendor/rcedit-rs）带 C++，只能靠
    # MSVC 的 cl.exe 为 msvc 目标编，所以这一层只在 Windows 上跑。
    if (-not $script:IsWin) {
        Skip-Layer 'native 依赖的 C++ 构建脚本需要 MSVC 的 cl.exe，无法在非 Windows 上为 windows-msvc 目标编译'
    }
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
        -Arguments @('check', '--target', $target, '--all-targets', '--message-format', 'short') `
        -WorkingDirectory $RepoRoot -Tail 60 -TimeoutSec 1800
    if ($r.ExitCode -ne 0) { throw 'cargo check --all-targets（x86_64-pc-windows-msvc）失败，见上方输出' }
    $warn = ([regex]::Matches($r.Output, 'warning:')).Count
    $what = if ($warn) { "（$warn 条 warning）" } else { '，0 warning' }
    return "根 crate 的 lib/bin/测试目标在 $target 上类型检查通过$what"
}

function Test-RustLogic {
    $cargo = Get-Tool 'cargo'
    if (-not $cargo) { Skip-Layer 'cargo 不在 PATH（https://rustup.rs）' }
    $r = Invoke-Native -FilePath $cargo -Arguments @('run', '--quiet') `
        -WorkingDirectory (Join-Path $DevCheckRoot 'rust/logic') -Tail 90
    if ($r.ExitCode -ne 0) { throw '行为断言失败，见上方输出' }
    $summary = ($r.Output -split "`r?`n" | Where-Object { $_ -match '条断言' } | Select-Object -Last 1)
    if (-not $summary) { throw 'logic 没有打印断言条数 —— 抽取出来的 item 一条都没跑到？' }
    # 断言条数也是断言：抽取清单被清空、或者生成物被换成空文件时，退出码仍是 0。
    if ($summary -notmatch '(\d+) 条断言，(\d+) 条失败') { throw "看不懂 logic 的汇总行：$summary" }
    if ([int]$Matches[1] -lt 100) { throw "logic 只跑了 $($Matches[1]) 条断言，抽取清单被架空？" }
    return $summary.Trim()
}

function Test-Frontend {
    # 直接跑仓库自己的前端检查：tsc --noEmit 覆盖 web/ 全量（strict），vitest 覆盖
    # web/__tests__。这里不另建一套最小 TS 工程 —— 那会与根 tsconfig 漂移，而根
    # tsconfig 才是构建真正用的那份。
    $pnpm = Get-Tool 'pnpm'
    if (-not $pnpm) { Skip-Layer 'pnpm 不在 PATH（corepack enable，或 npm i -g pnpm）' }
    if (-not (Test-Path -LiteralPath (Join-Path $RepoRoot 'node_modules'))) {
        if ($SkipInstall) { Skip-Layer 'node_modules 不存在（去掉 -SkipInstall 可自动 pnpm install --frozen-lockfile）' }
        Write-Info '首次运行：pnpm install --frozen-lockfile …'
        $r = Invoke-Native -FilePath $pnpm -Arguments @('install', '--frozen-lockfile') `
            -WorkingDirectory $RepoRoot -Tail 15 -TimeoutSec 900
        if ($r.ExitCode -ne 0) { throw 'pnpm install 失败' }
    }
    $checks = @(
        @{ What = 'tsc --noEmit（web/ 全量 strict）'; Args = @('exec', 'tsc', '--noEmit') }
        @{ What = 'vitest run'; Args = @('test') }
        # 只查我们自己维护的协议渲染文件（web/ 其余文件沿用上游格式，全量 prettier
        # 会把上游代码的既有风格当成回归）。--end-of-line auto：换行交给 .gitattributes。
        @{ What = 'prettier --check'; Args = @('exec', 'prettier', '--check', '--end-of-line', 'auto', 'web/agreement.ts') }
    )
    foreach ($c in $checks) {
        $r = Invoke-Native -FilePath $pnpm -Arguments $c.Args -WorkingDirectory $RepoRoot -Tail 30
        if ($r.ExitCode -ne 0) { throw "$($c.What) 失败" }
    }
    return 'tsc --strict / vitest / prettier(协议渲染) 均通过'
}
