# devcheck 自检（被 devcheck.ps1 dot-source）
#
# 检查工具最大的风险是「跑通了但其实什么都没查」。这里先正常生成一次，然后逐个
# 注入错误，确认对应的层真的会失败；任何一层「注入了错误却没报错」= 自检失败。
#
# 注入分两种：只改 tools/devcheck 下的生成物 / 临时目录，和临时改写仓库内的真实
# 文件（清单见 lib/Common.ps1 的 $script:SelfTestRepoFiles）。后者由仓库改动锁
# 互斥，并在磁盘上留备份，进程被杀后下次运行会先恢复。

function Invoke-SelfTest {
    $tmpDir = Join-Path $DevCheckRoot '_selftest'
    $cases = [System.Collections.Generic.List[object]]::new()

    function Add-Case {
        param([string]$Name, [scriptblock]$Mutate, [scriptblock]$Run, [scriptblock]$Cleanup = {})
        $cases.Add([pscustomobject]@{ Name = $Name; Mutate = $Mutate; Run = $Run; Cleanup = $Cleanup })
    }

    # ---- vendor：源码不许变成 submodule ----
    $gitmodules = Join-Path $RepoRoot '.gitmodules'
    Add-Case 'vendor 层能抓到源码变成 submodule' `
        -Mutate {
            Set-Content -Path $gitmodules -Encoding utf8 -Value @'
[submodule "."]
	path = .
	url = https://example.invalid/upstream.git
'@
        } `
        -Run { Test-VendoredSource } `
        -Cleanup { if (Test-Path -LiteralPath $gitmodules) { Remove-Item -LiteralPath $gitmodules -Force } }

    # ---- vendor：工作流里出现从外部拉取的动作 ----
    $badWorkflow = Join-Path $RepoRoot '.github/workflows/zz-devcheck-selftest.yml'
    Add-Case 'vendor 层能抓到工作流从外部拉取' `
        -Mutate {
            Set-Content -Path $badWorkflow -Encoding utf8 -Value @'
name: selftest
on: workflow_dispatch
jobs:
  x:
    runs-on: ubuntu-latest
    steps:
      - run: Invoke-WebRequest https://example.invalid/kirara-builder.exe -OutFile kirara-builder.exe
'@
        } `
        -Run { Test-VendoredSource } `
        -Cleanup { if (Test-Path -LiteralPath $badWorkflow) { Remove-Item -LiteralPath $badWorkflow -Force } }

    # ---- vendor：ARP 静默卸载用了 CLI 里不存在的选项 ----
    #      真踩过：把 -U/-S/-I 写成 --uninstall --silent --non-interactive，CLI 报
    #      「未知选项」直接退出，卸载一步都不跑 —— 而 ARP 里那个值看起来是「存在」的。
    $registryFile = Join-Path $RepoRoot 'native/installer/registry.rs'
    Add-Case 'vendor 层能抓到 ARP 静默卸载用了不存在的选项' `
        -Mutate {
            $original = [System.IO.File]::ReadAllText($registryFile)
            $mutated = $original.Replace('"\"{}\" -U -S -I"',
                '"\"{}\" --uninstall --silent --non-interactive"')
            if ($mutated -eq $original) { throw 'selftest 注入点没匹配上（registry.rs 的 QuietUninstallString 写法变了？）' }
            [System.IO.File]::WriteAllText($registryFile, $mutated)
        } `
        -Run { Test-VendoredSource } `
        -Cleanup { Restore-RepoFile -Backup (Get-RepoBackupPath -Path $registryFile) -Path $registryFile }

    # ---- vendor：rescle.cc 里再出现 locale::empty() ----
    #      这是 vendored 副本里唯一的「MSVC 版本敏感」代码，用注释形式注入不算
    #      （检查会剥掉 // 注释），所以注入一行真代码。
    $rescleFile = Join-Path $RepoRoot 'vendor/rcedit-rs/rcedit-sys/src/rescle.cc'
    Add-Case 'vendor 层能抓到 rescle.cc 用回 locale::empty()' `
        -Mutate {
            Add-Content -Path $rescleFile -Encoding utf8 `
                -Value "`nstatic void _devcheck_selftest() { std::locale l(std::locale::empty()); (void)l; }"
        } `
        -Run { Test-VendoredSource } `
        -Cleanup { Restore-RepoFile -Backup (Get-RepoBackupPath -Path $rescleFile) -Path $rescleFile }

    # ---- vendor：遥测调用被加回源码 ----
    #      注入一行**真代码**：注释形式不算（检查会剥掉 // 注释）。
    $telemetryFile = Join-Path $RepoRoot 'web/devcheck-selftest-telemetry.ts'
    Add-Case 'vendor 层能抓到 Sentry 上报被加回来' `
        -Mutate {
            Set-Content -Path $telemetryFile -Encoding utf8 `
                -Value "export const _devcheckSelfTest = sentry::init({ dsn: 'https://example.invalid/1' });"
        } `
        -Run { Test-VendoredSource } `
        -Cleanup { if (Test-Path -LiteralPath $telemetryFile) { Remove-Item -LiteralPath $telemetryFile -Force } }

    # ---- vendor：遥测依赖被加回 Cargo.toml（连 lock 一起回归的信号）----
    $cargoToml = Join-Path $RepoRoot 'Cargo.toml'
    Add-Case 'vendor 层能抓到 Sentry 依赖被加回 Cargo.toml' `
        -Mutate {
            Add-Content -Path $cargoToml -Encoding utf8 `
                -Value "`nsentry = { version = `"0.37`", features = [`"backtrace`"] }"
        } `
        -Run { Test-VendoredSource } `
        -Cleanup { Restore-RepoFile -Backup (Get-RepoBackupPath -Path $cargoToml) -Path $cargoToml }

    # ---- vendor：上报域名回到树里（DSN / 统计端点兜底）----
    #      新建一个临时文件而不是改现有文件：这条断言扫的是「全树文本文件里有没有域名」。
    Add-Case 'vendor 层能抓到上报域名回到树里' `
        -Mutate {
            Set-Content -Path $telemetryFile -Encoding utf8 `
                -Value "export const dsn = 'http://000000000000000000000000000000ff@steambird.cocogoat.cn/insight/x/0';"
        } `
        -Run { Test-VendoredSource } `
        -Cleanup { if (Test-Path -LiteralPath $telemetryFile) { Remove-Item -LiteralPath $telemetryFile -Force } }

    # ---- vendor：停更依赖被写回 Cargo.toml ----
    #      第 14～16 节把 mslnk / nt_version 换成了系统 API（IShellLinkW / ntdll），
    #      写回去就等于把没人维护的依赖请回来。
    Add-Case 'vendor 层能抓到停更依赖被写回 Cargo.toml' `
        -Mutate {
            Add-Content -Path $cargoToml -Encoding utf8 -Value "`nmslnk = `"=0.1`""
        } `
        -Run { Test-VendoredSource } `
        -Cleanup { Restore-RepoFile -Backup (Get-RepoBackupPath -Path $cargoToml) -Path $cargoToml }

    # ---- ps1：临时放一个语法错误的 .ps1 进仓库 ----
    Add-Case 'ps1 层能抓到 PowerShell 语法错误' `
        -Mutate {
            New-Item -ItemType Directory -Path $tmpDir -Force | Out-Null
            Set-Content -Path (Join-Path $tmpDir 'broken.ps1') -Value 'if ($x { Write-Host "unclosed" ' -Encoding utf8
        } `
        -Run { Test-Ps1Syntax }

    # ---- gen：抽取清单里的 item 在源码里消失（上游改名 / 删除）----
    #      「找不到就抛错」是这套抽取的地基：静默少抽一个安全阀等于没测。
    $packFile = Join-Path $RepoRoot 'native/builder/pack.rs'
    Add-Case 'gen 层能抓到清单里的 item 在源码里消失' `
        -Mutate {
            $original = [System.IO.File]::ReadAllText($packFile)
            $mutated = $original.Replace('fn unknown_config_keys(', 'fn unknown_config_keys_renamed(')
            if ($mutated -eq $original) { throw '注入失败：没找到 pack.rs 里的 unknown_config_keys' }
            [System.IO.File]::WriteAllText($packFile, $mutated)
        } `
        -Run { New-GenSources } `
        -Cleanup { Restore-RepoFile -Backup (Get-RepoBackupPath -Path $packFile) -Path $packFile }

    # ---- logic：把删目录安全阀的深度要求从 2 段放宽到 1 段 ----
    #      用 1 而不是 0：usize >= 0 恒真，编译器会多打一条无用的 comparison 告警。
    Add-Case 'logic 层能抓到安全阀被放宽' `
        -Mutate {
            $f = Join-Path $DevCheckRoot 'rust/logic/src/gen/extracted.rs'
            $t = [System.IO.File]::ReadAllText($f)
            $t2 = $t.Replace('depth >= 2 && !is_protected_root(path)', 'depth >= 1 && !is_protected_root(path)')
            if ($t2 -eq $t) { throw '注入失败：没找到 is_safe_delete_root 的深度判断（上游改了？）' }
            Write-GeneratedFile -Path $f -Content $t2
        } `
        -Run { Test-RustLogic }

    # ---- rust：往 CLI 里塞一个类型错误 ----
    #      这一层只在 Windows 上跑（native 依赖的 C++ 要 MSVC），其它平台记为跳过。
    $cliFile = Join-Path $RepoRoot 'native/cli/mod.rs'
    Add-Case 'rust 层能抓到 Rust 类型错误' `
        -Mutate {
            Add-Content -Path $cliFile -Encoding utf8 `
                -Value "`nfn _devcheck_selftest() { let _x: u32 = `"不是数字`"; }"
        } `
        -Run { Test-RustTypecheck } `
        -Cleanup { Restore-RepoFile -Backup (Get-RepoBackupPath -Path $cliFile) -Path $cliFile }

    # ---- front：往协议渲染文件里塞一个类型错误 ----
    $agreementFile = Join-Path $RepoRoot 'web/agreement.ts'
    Add-Case 'front 层能抓到 TypeScript 类型错误' `
        -Mutate {
            Add-Content -Path $agreementFile -Encoding utf8 `
                -Value "`nexport const _devcheckSelfTest: number = 'not a number';"
        } `
        -Run { Test-Frontend } `
        -Cleanup { Restore-RepoFile -Backup (Get-RepoBackupPath -Path $agreementFile) -Path $agreementFile }

    # ---- front：类型仍然合法、但把协议净化改坏（只有 vitest 抓得到）----
    #      证明单测不是摆设：tsc 只看得懂类型，`<script>` 会不会被剥掉只有用例知道。
    Add-Case 'front 层能抓到协议净化被改坏' `
        -Mutate {
            $original = [System.IO.File]::ReadAllText($agreementFile)
            $mutated = $original.Replace("'script',", "'script2',")
            if ($mutated -eq $original) { throw '注入失败：没找到 FORBID_TAGS 里的 script 项' }
            [System.IO.File]::WriteAllText($agreementFile, $mutated)
        } `
        -Run { Test-Frontend } `
        -Cleanup { Restore-RepoFile -Backup (Get-RepoBackupPath -Path $agreementFile) -Path $agreementFile }

    # ---- ci：工作流里的 action 版本低于登记的下限 ----
    $devcheckYml = Join-Path $RepoRoot '.github/workflows/devcheck.yml'
    Add-Case 'ci 层能抓到 action 版本低于下限' `
        -Mutate {
            $original = [System.IO.File]::ReadAllText($devcheckYml)
            $mutated = $original.Replace('actions/checkout@v5', 'actions/checkout@v1')
            if ($mutated -eq $original) { throw '注入失败：devcheck.yml 里没有 actions/checkout@v5' }
            [System.IO.File]::WriteAllText($devcheckYml, $mutated)
        } `
        -Run { Test-CiScripts } `
        -Cleanup { Restore-RepoFile -Backup (Get-RepoBackupPath -Path $devcheckYml) -Path $devcheckYml }

    # ---- ci：工作流不再裸跑 devcheck（all 集合的层没在 CI 上执行）----
    Add-Case 'ci 层能抓到工作流不再跑 all 集合' `
        -Mutate {
            $original = [System.IO.File]::ReadAllText($devcheckYml)
            $mutated = [regex]::Replace($original,
                '(?m)^[ \t]*run:[ \t]*pwsh -NoProfile -File \S*devcheck\.ps1[ \t]*\r?\n',
                "# 裸调用被自检删掉`n")
            if ($mutated -eq $original) { throw '注入失败：devcheck.yml 里没有裸跑 devcheck.ps1 的那一步' }
            [System.IO.File]::WriteAllText($devcheckYml, $mutated)
        } `
        -Run { Test-CiScripts } `
        -Cleanup { Restore-RepoFile -Backup (Get-RepoBackupPath -Path $devcheckYml) -Path $devcheckYml }

    # ---- ci：package.json 的 build 脚本丢掉拼接一步 ----
    #      少了它，交付物就是 cargo 的裸 builder（installer 没拼进去）。
    $packageJson = Join-Path $RepoRoot 'package.json'
    Add-Case 'ci 层能抓到 build 脚本丢掉拼接一步' `
        -Mutate {
            $original = [System.IO.File]::ReadAllText($packageJson)
            $mutated = $original.Replace(' && node scripts/merge-release-bundle.mjs', '')
            if ($mutated -eq $original) { throw '注入失败：package.json 的 build 脚本里没有 merge-release-bundle 这一步' }
            [System.IO.File]::WriteAllText($packageJson, $mutated)
        } `
        -Run { Test-CiScripts } `
        -Cleanup { Restore-RepoFile -Backup (Get-RepoBackupPath -Path $packageJson) -Path $packageJson }

    # 先按磁盘备份清掉上一次被中断的自检留下的注入，再上锁独占。
    Clear-RepoMutations
    $caught = 0; $missed = 0; $skipped = 0

    Enter-RepoLock
    try {
        Write-Step 'selftest 注入错误自检'
        $idx = 0
        foreach ($c in $cases) {
            $idx++
            try {
                # 每次都从干净的生成物开始
                New-GenSources | Out-Null
                # Mutate 之前先把原始内容落到磁盘：进程被杀也留得下恢复依据。
                # 只在备份不存在时写：否则某个用例没清干净时，备份会被「已污染」的
                # 内容覆盖，之后再也回不到原始版本。
                foreach ($rel in $script:SelfTestRepoFiles) {
                    $path = Join-Path $RepoRoot $rel
                    if (-not (Test-Path -LiteralPath (Get-RepoBackupPath -Path $path))) {
                        Backup-RepoFile -Path $path | Out-Null
                    }
                }
                & $c.Mutate
            }
            catch {
                $missed++
                Write-Bad "  ✗ $($c.Name) —— 注入失败: $($_.Exception.Message)"
                continue
            }
            Write-Host "  ── 注入 $idx/$($cases.Count)：$($c.Name)" -ForegroundColor DarkYellow
            Write-Host '     ↓ 接下来这段报错是故意注入的，看到它才说明这层没被架空' -ForegroundColor DarkGray
            $outcome = 'CAUGHT'
            try {
                & $c.Run | Out-Null
                $outcome = 'MISSED'
            }
            catch [LayerSkipped] { $outcome = 'SKIPPED' }
            catch { $outcome = 'CAUGHT' }
            try { & $c.Cleanup } catch { }

            switch ($outcome) {
                'CAUGHT'  { $caught++;  Write-Ok "  ✓ $($c.Name)" }
                'SKIPPED' { $skipped++; Write-Info "  - $($c.Name)（缺工具链，跳过）" }
                'MISSED'  { $missed++;  Write-Bad "  ✗ $($c.Name) —— 注入了错误却没报错，这层是空壳！" }
            }
        }

        # 收尾：删掉临时目录并恢复干净的生成物
        if (Test-Path -LiteralPath $tmpDir) { Remove-Item -LiteralPath $tmpDir -Recurse -Force }
        New-GenSources | Out-Null
    }
    finally {
        # 无论正常结束还是抛异常，都要把仓库内的注入还原并放锁。
        Clear-RepoMutations -Quiet
        # Backup-RepoFile 会顺手建目录，这里再兜底删一次，避免留下空目录。
        Remove-Item -LiteralPath (Get-SelfTestBackupDir) -Recurse -Force -ErrorAction SilentlyContinue
        Exit-RepoLock
    }

    Write-Host ''
    if ($missed) {
        Write-Host "✗ 自检失败：$missed 个注入错误没被抓到" -ForegroundColor Red
        return $false
    }
    Write-Host "✓ 自检通过：$caught 个注入错误全部被抓到$(if ($skipped) { "，$skipped 个因缺工具链跳过" } else { '' })" -ForegroundColor Green
    return $true
}
