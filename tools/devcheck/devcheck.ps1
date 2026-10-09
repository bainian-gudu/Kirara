#!/usr/bin/env pwsh
<#
.SYNOPSIS
    本地快速体检：不跑完整构建，就把「编译期会炸 / 会静默降级」的问题提前抓出来。

.DESCRIPTION
    分层检查，越靠前越快、依赖越少：

      vendor 源码只在仓库内：不是 submodule、快照完整、工作流与打包脚本里没有任何从
             上游拉源码或下二进制的动作、git 依赖在 Cargo.lock 里锁到 commit、
             npm 依赖全部来自 registry、遥测（Sentry 错误上报 + 使用统计）没有被加
             回来、写进 ARP 的卸载命令行能被自己的 CLI 解析、停更依赖没有回归
      ps1    所有 .ps1 的语法解析（PowerShell Parser，秒级，无依赖）
      gen    从 native/ 的源码按名字抽出 logic 层要断言的 item（秒级，无依赖）
      rust   根 crate 在 x86_64-pc-windows-msvc 上的类型检查（cargo check
             --all-targets）。native 依赖带 C++，只在有 MSVC 的 Windows 上跑，
             其它平台 SKIP
      logic  被抽出来的那批安全阀与纯逻辑的**行为断言**（任意平台可跑）
      front  web/ 的 tsc --noEmit（strict）+ vitest + 协议渲染文件的 prettier
      ci     工作流里 action 的版本下限、每一层有没有真的接进 devcheck.yml、
             构建入口与交付名是否一致

    任何一层失败 → 退出码 1。缺工具链的层标记 SKIP 并给出提示（不算失败）。

.EXAMPLE
    pwsh tools/devcheck/devcheck.ps1
.EXAMPLE
    pwsh tools/devcheck/devcheck.ps1 -Layer vendor,logic
.EXAMPLE
    pwsh tools/devcheck/devcheck.ps1 -SelfTest    # 注入 17 个错误，确认每层都会报错

.NOTES
    首次运行会下载：rustup target x86_64-pc-windows-msvc、logic crate 的 cargo 依赖、
    根 node_modules。-SkipInstall 禁止一切自动安装（缺什么就 SKIP）。
    下游应用（HoYoEnhance）的构建、宿主断言与安装包打包检查在那边仓库的
    tools/devcheck 里，本仓库只负责安装器自身。
#>
[CmdletBinding()]
param(
    # 逗号或空格分隔的层名。故意用 [string] 而不是 [string[]]：
    # `pwsh -File devcheck.ps1 -Layer vendor,logic` 用数组类型会把 "vendor,logic" 当成一个值。
    [string]$Layer = 'all',

    # 不自动安装任何东西（rustup target / pnpm install）
    [switch]$SkipInstall,

    # 自检：故意注入错误，确认每一层真的会报错。一部分只改 tools/devcheck 下的生成物
    # 与 _selftest 临时目录，另一部分会临时改写仓库内的文件（.gitmodules、假工作流、
    # registry.rs、rescle.cc、pack.rs、Cargo.toml、package.json、web/agreement.ts、
    # native/cli/mod.rs）。自检持有仓库改动锁，并在磁盘上留备份：中断后下次运行会先
    # 恢复再开工。
    [switch]$SelfTest
)

# npm / npx / node 自身会打 DEP0040（punycode）、DEP0169（url.parse）这类弃用告警，
# 跟本仓库无关，只会把真正需要看的输出淹掉。子进程继承这个变量。
$env:NODE_NO_WARNINGS = '1'

if ($PSVersionTable.PSVersion.Major -lt 7) {
    throw "devcheck 需要 PowerShell 7+（当前 $($PSVersionTable.PSVersion)）。Windows PowerShell 5.1 请用: pwsh -File tools/devcheck/devcheck.ps1"
}

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# 层内部主动跳过用（类必须在使用前定义）
class LayerSkipped : System.Exception {
    LayerSkipped([string]$message) : base($message) {}
}

$script:IsWin = ($env:OS -eq 'Windows_NT')
$DevCheckRoot = $PSScriptRoot
$RepoRoot = (Resolve-Path (Join-Path $DevCheckRoot '..\..')).Path

. (Join-Path $DevCheckRoot 'lib/RustSource.ps1')
. (Join-Path $DevCheckRoot 'lib/Generate.ps1')
. (Join-Path $DevCheckRoot 'lib/Common.ps1')
. (Join-Path $DevCheckRoot 'lib/Layers.ps1')
. (Join-Path $DevCheckRoot 'lib/CiScripts.ps1')
. (Join-Path $DevCheckRoot 'lib/SelfTest.ps1')

# ---------------------------------------------------------------------------
# 执行
# ---------------------------------------------------------------------------
if ($SelfTest) {
    Write-Host ''
    Write-Host "devcheck 自检 — 仓库根 $RepoRoot" -ForegroundColor White
    Write-Host ''
    $ok = Invoke-SelfTest
    if (-not $ok) { exit 1 }
    exit 0
}

$validLayers = @('all', 'vendor', 'ps1', 'gen', 'rust', 'logic', 'front', 'ci')
# all 的展开顺序：快 → 慢、依赖少 → 依赖多。ci 层会断言这份清单里的每一层都真的
# 在 .github/workflows/devcheck.yml 上跑过。
$allLayers = @('vendor', 'ps1', 'gen', 'rust', 'logic', 'front', 'ci')
$requested = @($Layer -split '[,\s]+' | Where-Object { $_ })
if (-not $requested.Count) { $requested = @('all') }
foreach ($r in $requested) {
    if ($validLayers -notcontains $r) { throw "未知的层 '$r'，可选: $($validLayers -join ', ')" }
}
$wanted = if ($requested -contains 'all') { $allLayers } else { $requested }
# gen 是 logic 的前置
if (($wanted -contains 'logic') -and ($wanted -notcontains 'gen')) {
    $wanted = @('gen') + $wanted
}

Write-Host ''
Write-Host "devcheck — 仓库根 $RepoRoot" -ForegroundColor White
Write-Host "层      $($wanted -join ', ')" -ForegroundColor White
Write-Host ''

foreach ($l in $wanted) {
    switch ($l) {
        'vendor' { Invoke-Layer 'vendor 源码只用仓库内快照'   { Test-VendoredSource } }
        'ps1'    { Invoke-Layer 'ps1   PowerShell 脚本语法'    { Test-Ps1Syntax } }
        'gen'    { Invoke-Layer 'gen   生成检查用源码'         { New-GenSources } }
        'rust'   { Invoke-Layer 'rust  根 crate 类型检查 msvc' { Test-RustTypecheck } }
        'logic'  { Invoke-Layer 'logic 安全阀与纯逻辑行为断言' { Test-RustLogic } }
        'front'  { Invoke-Layer 'front TS 类型 / 单测 / 格式'  { Test-Frontend } }
        'ci'     { Invoke-Layer 'ci    CI 不变量'              { Test-CiScripts } }
    }
}

Write-Host ''
Write-Host '════ 汇总 ════' -ForegroundColor White
$script:Results | Format-Table -AutoSize | Out-String -Width 200 | Write-Host

$failed = @($script:Results | Where-Object { $_.Status -eq 'FAIL' })
if ($failed.Count) {
    Write-Host "✗ $($failed.Count) 层失败: $(($failed | ForEach-Object Layer) -join ', ')" -ForegroundColor Red
    exit 1
}
if ($script:SkipCount) {
    Write-Host "✓ 通过（$($script:SkipCount) 层因缺工具链跳过）" -ForegroundColor Yellow
    exit 0
}
Write-Host '✓ 全部通过' -ForegroundColor Green
exit 0
