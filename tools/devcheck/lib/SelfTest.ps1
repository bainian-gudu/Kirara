# devcheck 自检（被 devcheck.ps1 dot-source）
#
# 证明这套检查不是空壳：先正常生成一次，然后往**生成物**里注入错误（仓库源码一个字都不改），
# 逐个确认对应层会失败。任何一层「注入了错误却没报错」= 自检失败。

function Invoke-SelfTest {
    $front = Join-Path $DevCheckRoot 'front'
    $tmpDir = Join-Path $DevCheckRoot '_selftest'
    $cases = [System.Collections.Generic.List[object]]::new()

    function Add-Case {
        param([string]$Name, [scriptblock]$Mutate, [scriptblock]$Run, [scriptblock]$Cleanup = {})
        $cases.Add([pscustomobject]@{ Name = $Name; Mutate = $Mutate; Run = $Run; Cleanup = $Cleanup })
    }

    # --- 0a) vendor：出现 .gitmodules 就必须报错（kachina 不能是 submodule）---
    $gitmodules = Join-Path $RepoRoot '.gitmodules'
    Add-Case 'vendor 层能抓到 kachina 变成 submodule' `
        -Mutate {
            Set-Content -Path $gitmodules -Encoding utf8 -Value @'
[submodule "."]
	path = .
	url = https://example.invalid/upstream.git
'@
        } `
        -Run { Test-VendoredSource } `
        -Cleanup { if (Test-Path -LiteralPath $gitmodules) { Remove-Item -LiteralPath $gitmodules -Force } }

    # --- 0b) vendor：工作流里出现从外部拉取的动作就必须报错 ---
    $badWorkflow = Join-Path $RepoRoot '.github/workflows/zz-devcheck-selftest.yml'
    Add-Case 'vendor 层能抓到工作流从上游拉取' `
        -Mutate {
            Set-Content -Path $badWorkflow -Encoding utf8 -Value @'
name: selftest
on: workflow_dispatch
jobs:
  x:
    runs-on: ubuntu-latest
    steps:
      - run: Invoke-WebRequest https://example.invalid/kirara-builder.exe -OutFile tools/kirara-builder.exe
'@
        } `
        -Run { Test-VendoredSource } `
        -Cleanup { if (Test-Path -LiteralPath $badWorkflow) { Remove-Item -LiteralPath $badWorkflow -Force } }

    # --- 0b2) vendor：QuietUninstallString 用了 cli/arg.rs 里不存在的选项就必须报错 ---
    #      真踩过：第一版写的是 --uninstall --silent --non-interactive，而 arg.rs 里
    #      这几个 flag 只声明了 short（-U/-S/-I），clap 不认长名，退出码 2、卸载不跑。
    $registryFile = Join-Path $RepoRoot 'src-tauri/src/installer/registry.rs'
    Add-Case 'vendor 层能抓到 ARP 静默卸载用了不存在的选项' `
        -Mutate {
            $original = [System.IO.File]::ReadAllText($registryFile)
            $mutated = $original -replace [regex]::Escape('"\"{uninstaller}\" -U -S -I"'), `
                '"\"{uninstaller}\" --uninstall --silent --non-interactive"'
            if ($mutated -eq $original) { throw 'selftest 注入点没匹配上（registry.rs 的 QuietUninstallString 写法变了？）' }
            [System.IO.File]::WriteAllText($registryFile, $mutated)
        } `
        -Run { Test-VendoredSource } `
        -Cleanup { Restore-RepoFile -Backup (Get-RepoBackupPath -Path $registryFile) -Path $registryFile }

    # --- 0c) vendor：rescle.cc 里再出现 locale::empty() 就必须报错 ---
    #     这是 vendored 副本里唯一的「MSVC 版本敏感」代码，用注释形式注入不算
    #     （检查会剥掉 // 注释），所以注入一行真代码。
    $rescleFile = Join-Path $RepoRoot 'vendor/rcedit-rs/rcedit-sys/src/rescle.cc'
    Add-Case 'vendor 层能抓到 rescle.cc 用回 locale::empty()' `
        -Mutate {
            Add-Content -Path $rescleFile -Encoding utf8 `
                -Value "`nstatic void _devcheck_selftest() { std::locale l(std::locale::empty()); (void)l; }"
        } `
        -Run { Test-VendoredSource } `
        -Cleanup { Restore-RepoFile -Backup (Get-RepoBackupPath -Path $rescleFile) -Path $rescleFile }

    # --- 0d) vendor：Sentry 上报被加回来就必须报错（本项目已物理移除遥测）---
    #     注入一行**真代码**：注释形式不算（检查会剥掉 // 注释），跟 0c 同一个道理。
    $utilsMod = Join-Path $RepoRoot 'src-tauri/src/utils/mod.rs'
    Add-Case 'vendor 层能抓到 Sentry 上报被加回来' `
        -Mutate {
            Add-Content -Path $utilsMod -Encoding utf8 `
                -Value "`nfn _devcheck_selftest_telemetry() { let _g = sentry::init(sentry::ClientOptions::default()); }"
        } `
        -Run { Test-VendoredSource } `
        -Cleanup { Restore-RepoFile -Backup (Get-RepoBackupPath -Path $utilsMod) -Path $utilsMod }

    # --- 0e) vendor：遥测依赖被加回 Cargo.toml 就必须报错（连 lock 一起回归的信号）---
    $kaCargoToml = Join-Path $RepoRoot 'src-tauri/Cargo.toml'
    Add-Case 'vendor 层能抓到 Sentry 依赖被加回 Cargo.toml' `
        -Mutate {
            Add-Content -Path $kaCargoToml -Encoding utf8 `
                -Value "`nsentry = { version = `"0.37`", features = [`"backtrace`"] }"
        } `
        -Run { Test-VendoredSource } `
        -Cleanup { Restore-RepoFile -Backup (Get-RepoBackupPath -Path $kaCargoToml) -Path $kaCargoToml }

    # --- 0f) vendor：上报域名回到 vendored 树里就必须报错（DSN / 统计端点兜底）---
    #     新建一个临时文件而不是改现有文件：这条断言扫的是「全树文本文件里有没有域名」。
    $dsnFile = Join-Path $RepoRoot 'src/devcheck-selftest-telemetry.ts'
    Add-Case 'vendor 层能抓到上报域名回到 vendored 树' `
        -Mutate {
            Set-Content -Path $dsnFile -Encoding utf8 `
                -Value "export const dsn = 'http://000000000000000000000000000000ff@steambird.cocogoat.cn/insight/x/0';"
        } `
        -Run { Test-VendoredSource } `
        -Cleanup { if (Test-Path -LiteralPath $dsnFile) { Remove-Item -LiteralPath $dsnFile -Force } }

    # --- 0g) vendor：停更依赖被写回 Cargo.toml 就必须报错 ---
    #     第 16 节把 mslnk / nt_version 换成了系统 API（IShellLinkW / ntdll），
    #     写回去就等于把没人维护的依赖请回来，这条断言必须拦住。
    Add-Case 'vendor 层能抓到停更依赖被写回 Cargo.toml' `
        -Mutate {
            Add-Content -Path $kaCargoToml -Encoding utf8 -Value "`nmslnk = `"=0.1`""
        } `
        -Run { Test-VendoredSource } `
        -Cleanup { Restore-RepoFile -Backup (Get-RepoBackupPath -Path $kaCargoToml) -Path $kaCargoToml }

    # --- 0h) vendor：C10 后 Tauri 依赖被加回来就必须报错 ---
    Add-Case 'vendor 层能抓到 Tauri 依赖回归' `
        -Mutate {
            Add-Content -Path $kaCargoToml -Encoding utf8 -Value "`ntauri = `"2`""
        } `
        -Run { Test-VendoredSource } `
        -Cleanup { Restore-RepoFile -Backup (Get-RepoBackupPath -Path $kaCargoToml) -Path $kaCargoToml }

    # --- 1) ps1：临时放一个语法错误的 .ps1 进仓库 ---
    Add-Case 'ps1 层能抓到 PowerShell 语法错误' `
        -Mutate {
            New-Item -ItemType Directory -Path $tmpDir -Force | Out-Null
            Set-Content -Path (Join-Path $tmpDir 'broken.ps1') -Value 'if ($x { Write-Host "unclosed" ' -Encoding utf8
        } `
        -Run { Test-Ps1Syntax }

    # --- 2) rust：往生成的 uninstall.rs 里塞一个类型错误 ---
    Add-Case 'rust 层能抓到 Rust 类型错误' `
        -Mutate {
            $f = Join-Path $DevCheckRoot 'rust/typecheck/src/gen/uninstall.rs'
            Add-Content -Path $f -Value "`nfn _devcheck_selftest() { let _x: u32 = `"不是数字`"; }" -Encoding utf8
        } `
        -Run { Test-RustTypecheck }

    # --- 2b) rust：lnk.rs 的 COM 调用写错（或整份文件没挂进 crate）就必须报错 ---
    #     这一段是「用系统 API 换掉 mslnk」的落点，只在 Windows 上跑，
    #     本地唯一能守的就是「它在 msvc target 上编得过」。
    Add-Case 'rust 层能抓到 lnk.rs 的 COM 调用写错' `
        -Mutate {
            $f = Join-Path $DevCheckRoot 'rust/typecheck/src/gen/lnk.rs'
            Add-Content -Path $f -Encoding utf8 -Value @'

fn _devcheck_selftest(l: &windows::Win32::UI::Shell::IShellLinkW) {
    let _ = unsafe { l.SetPathTypo(&windows::core::HSTRING::from("x")) };
}
'@
        } `
        -Run { Test-RustTypecheck }

    # --- 3) logic：把注册表安全阀的深度要求从 2 段放宽到 1 段 ---
    # 用 1 而不是 0：usize >= 0 恒真，编译器会多打一条无用的 comparison 告警（噪音），
    # 放宽的效果一样。详见 README.md「日志里哪些 Warning / error 是正常的」。
    Add-Case 'logic 层能抓到安全阀被放宽' `
        -Mutate {
            $f = Join-Path $DevCheckRoot 'rust/logic/src/gen/extracted.rs'
            $t = [System.IO.File]::ReadAllText($f)
            $t2 = $t.Replace('Some(_) => segments.len() >= 2,', 'Some(_) => segments.len() >= 1,')
            if ($t2 -eq $t) { throw '注入失败：没找到 is_safe_registry_target 的深度判断（上游改了？）' }
            Write-GeneratedFile -Path $f -Content $t2
        } `
        -Run { Test-RustLogic }

    # --- 4) front/ts：往生成的 agreement.ts 里塞一个类型错误 ---
    Add-Case 'front 层能抓到 TypeScript 类型错误' `
        -Mutate {
            $f = Join-Path $front 'gen/src/utils/agreement.ts'
            Add-Content -Path $f -Value "`nexport const _devcheckSelfTest: number = 'not a number';" -Encoding utf8
        } `
        -Run {
            Test-Frontend
            throw 'front 层没有报错，这层是空壳'
        }

    # --- 5) front/sfc：一个 template 不闭合的 .vue ---
    Add-Case 'front 层能抓到 .vue 模板错误' `
        -Mutate {
            $d = Join-Path $front '_selftest'
            New-Item -ItemType Directory -Path $d -Force | Out-Null
            Set-Content -Path (Join-Path $d 'Broken.vue') -Encoding utf8 -Value @'
<template>
  <div class="x">
    <span>未闭合
</template>
<script setup lang="ts">
const a: number = 1;
</script>
'@
        } `
        -Run {
            $node = Get-Tool 'node'
            if (-not $node) { Skip-Layer 'node 不在 PATH' }
            $r = Invoke-Native -FilePath $node -Arguments @('sfccheck.mjs', '_selftest') -WorkingDirectory $front -Tail 10
            if ($r.ExitCode -ne 0) { throw 'SFC 编译报错（符合预期）' }
        }

    # --- 6) ci：把 vcvars 输出会解析出 0 个变量的注册
    # 这层守的是 build-kachina 的 MSVC 注入：真退化了要等 9 分钟冷构建才炸。
    $ciScript = Join-Path $RepoRoot 'tools/ci/Import-DevCmd.ps1'
    Add-Case 'ci 层能抓到 MSVC 环境解析失效' `
        -Mutate {
            $text = [System.IO.File]::ReadAllText($ciScript)
            $broken = $text.Replace("if (`$line -match '^([^=]+)=(.*)`$')", 'if ($line -match "^THIS_WILL_NEVER_MATCH=(.*)$")')
            if ($broken -eq $text) { throw '注入失败：没找到解析环境变量的正则' }
            [System.IO.File]::WriteAllText($ciScript, $broken)
        } `
        -Run { Test-CiScripts } `
        -Cleanup { Restore-RepoFile -Backup (Get-RepoBackupPath -Path $ciScript) -Path $ciScript }

    # 先按磁盘备份清掉上一次被中断的自检留下的注入，再上锁独占。
    Clear-RepoMutations
    $caught = 0; $missed = 0; $skipped = 0

    Enter-RepoLock
    try {
        Write-Step 'selftest 注入错误自检'
        $idx = 0
        foreach ($c in $cases) {
            $idx++
            # 每次都从干净的生成物开始
            New-GenSources | Out-Null
            try {
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
        foreach ($d in @($tmpDir, (Join-Path $front '_selftest'))) {
            if (Test-Path -LiteralPath $d) { Remove-Item -LiteralPath $d -Recurse -Force }
        }
        New-GenSources | Out-Null
    }
    finally {
        # 无论正常结束还是抛异常，都要把仓库内的注入还原并放锁。
        Clear-RepoMutations -Quiet
        # Backup-RepoFile 会顺手建目录，这里再兜底删一次，避免留下空目录。
        Remove-Item -LiteralPath (Get-SelfTestBackupDir) -Recurse -Force -ErrorAction SilentlyContinue
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
