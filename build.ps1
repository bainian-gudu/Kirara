<#
.SYNOPSIS
    从本仓库根目录的源码（上游 kachina-installer + 本地补丁）构建 kirara-builder.exe。

.DESCRIPTION
    产物：tools\kirara-builder.exe
    该文件是「打包器 CLI + 安装器 GUI 模板」的二进制拼接体，由 package.json 的
    build 脚本生成（cargo 产物名为 kachina-builder.exe），本脚本负责准备工具链、
    构建，再以本项目的名字拷到 tools\ 下。

    这是本仓库唯一的构建入口；产物供下游项目（HoYoEnhance）打包安装包时使用。

    仅在 Windows 上可运行（上游依赖 MSVC、windows crate、WebView2）。

.PARAMETER Force
    即使 tools\kirara-builder.exe 已存在也重新构建。

.PARAMETER Toolchain
    Rust 工具链，默认 nightly（根目录 rust-toolchain.toml 指定；`trim-paths` 与
    `profile.rustflags` 目前仍是 nightly 专属特性，stable 编不过）。

.EXAMPLE
    pwsh build.ps1
    pwsh build.ps1 -Force
#>
[CmdletBinding()]
param(
    [switch]$Force,
    [string]$Toolchain = "nightly"
)

$ErrorActionPreference = "Stop"

$RepoRoot    = Split-Path -Parent $MyInvocation.MyCommand.Path
$ToolsDir    = Join-Path $RepoRoot "tools"
$BuilderOut  = Join-Path $ToolsDir "kirara-builder.exe"

# 目标三元组：本项目只服务 HoYoEnhance，兼容范围与宿主一致（64 位 Windows 10
# 1607+ / Windows 11），因此用标准 tier-1 目标，不再需要上游的 win7 自定义目标
# （tier-3，rustup 没有预编译标准库，才要 nightly + rust-src + -Z build-std）。
$TargetTriple = "x86_64-pc-windows-msvc"
$ReleaseDir   = Join-Path $RepoRoot "target\$TargetTriple\release"
# pnpm build 的产物：cargo 编出原始 builder，再由 merge-release-bundle.mjs 拼成
# kachina-builder-bundle.exe（原始 builder 不被覆盖）。对外交付的是这个拼接体。
$BuiltBundle  = Join-Path $ReleaseDir "kachina-builder-bundle.exe"
$RawBuilder   = Join-Path $ReleaseDir "kachina-builder.exe"

function Step([string]$msg) { Write-Host "==> $msg" -ForegroundColor Cyan }
function Ok([string]$msg)   { Write-Host "    $msg" -ForegroundColor Green }

# 找出比 $reference（现有 builder）更新的源码文件；没有则返回 $null。
# 用来避免「改了源码，却仍在用旧的 kirara-builder.exe」。
function Get-NewerSource([string]$reference) {
    if (-not (Test-Path $reference)) { return $null }
    $refTime = (Get-Item $reference).LastWriteTimeUtc
    # 分隔符两种都认：脚本只在 Windows 上跑，但这样便于在别处单测
    $skip = '[\\/](node_modules|dist|target|gen|\.cache)([\\/]|$)'
    $roots = @('native', 'web', 'locales', 'resources', 'libs', 'scripts', 'tests') |
        ForEach-Object { Join-Path $RepoRoot $_ } |
        Where-Object { Test-Path -LiteralPath $_ }
    $files = @(Get-ChildItem -Path $roots -Recurse -File -Force -ErrorAction SilentlyContinue)
    $files += @(Get-ChildItem -Path $RepoRoot -File -Force |
        Where-Object { $_.Extension -in @('.json', '.ts', '.tsx', '.mjs', '.toml', '.lock') })
    return $files |
        Where-Object { $_.FullName -notmatch $skip -and $_.LastWriteTimeUtc -gt $refTime } |
        Sort-Object LastWriteTimeUtc -Descending |
        Select-Object -First 1 -ExpandProperty FullName
}

function Require([string]$name, [string]$hint) {
    $cmd = Get-Command $name -ErrorAction SilentlyContinue
    if (-not $cmd) { throw "缺少 $name。$hint" }
    return $cmd.Source
}

if ($env:OS -ne "Windows_NT") {
    throw "kirara-builder 只能在 Windows 上构建（MSVC + windows crate + WebView2）。"
}

if (-not (Test-Path (Join-Path $RepoRoot "Cargo.toml"))) {
    throw "未找到 $RepoRoot\Cargo.toml —— 源码缺失。"
}

if ((Test-Path $BuilderOut) -and -not $Force) {
    $newer = Get-NewerSource $BuilderOut
    if (-not $newer) {
        Ok("已存在且比源码新：$BuilderOut（用 -Force 强制重建）")
        return $BuilderOut
    }
    Step "源码比现有 builder 新，需要重建"
    Write-Host "    触发文件：$newer" -ForegroundColor DarkGray
}

New-Item -ItemType Directory -Force -Path $ToolsDir | Out-Null

# ---------------------------------------------------------------- 工具链检查
Step "检查工具链"
$null = Require "rustup" "请安装 Rust：https://rustup.rs （需 MSVC 生成工具）"
$null = Require "cargo"  "rustup 安装后重开终端"
$null = Require "node"   "请安装 Node.js 20+"

Step "安装 Rust 工具链 $Toolchain"
& rustup toolchain install $Toolchain --profile minimal
if ($LASTEXITCODE -ne 0) { throw "rustup toolchain install $Toolchain 失败" }

# pnpm：优先 corepack（Node 自带），退回 npm 全局安装
$pnpm = Get-Command pnpm -ErrorAction SilentlyContinue
if (-not $pnpm) {
    Step "启用 pnpm（corepack）"
    $corepack = Get-Command corepack -ErrorAction SilentlyContinue
    if ($corepack) {
        & corepack enable
        & corepack prepare pnpm@10.17.0 --activate
    }
    $pnpm = Get-Command pnpm -ErrorAction SilentlyContinue
}
if (-not $pnpm) {
    Step "corepack 不可用，改用 npm 安装 pnpm"
    & npm install -g pnpm@10.17.0 --no-fund --no-audit
    if ($LASTEXITCODE -ne 0) { throw "npm install -g pnpm 失败" }
    $pnpm = Get-Command pnpm -ErrorAction SilentlyContinue
}
if (-not $pnpm) { throw "pnpm 不可用，无法构建" }
Ok("pnpm: $($pnpm.Source)")

# ---------------------------------------------------------------- 构建
Push-Location $RepoRoot
try {
    Step "pnpm install（frozen-lockfile）"
    & pnpm install --frozen-lockfile
    if ($LASTEXITCODE -ne 0) { throw "pnpm install 失败" }

    # 依赖里仍有 crate 用 cmake 编 C 源码（msquic 等），按本机核数给 cmake 一个
    # 明确的并行度，省得在 Windows 上退回串行编译。
    if (-not $env:CMAKE_BUILD_PARALLEL_LEVEL) {
        $env:CMAKE_BUILD_PARALLEL_LEVEL = [string][Environment]::ProcessorCount
    }

    Step "pnpm build（原生宿主 + $TargetTriple，LTO；首次冷构建较慢）"
    & pnpm build
    if ($LASTEXITCODE -ne 0) { throw "pnpm build 失败" }
} finally {
    Pop-Location
}

if (-not (Test-Path $BuiltBundle)) {
    throw "构建结束但未找到 kachina-builder-bundle.exe（预期 $ReleaseDir；merge-release-bundle 未运行？）"
}
if ((Test-Path $RawBuilder) -and (Get-Item $BuiltBundle).Length -le (Get-Item $RawBuilder).Length) {
    throw "拼接体 $BuiltBundle 不大于 cargo builder $RawBuilder，installer 未拼进去"
}

Copy-Item $BuiltBundle $BuilderOut -Force
$sizeMb = [math]::Round((Get-Item $BuilderOut).Length / 1MB, 2)
Ok("kirara-builder → $BuilderOut ($sizeMb MB)")

# 自检：能打印帮助即认为可用
& $BuilderOut --help *> $null
if ($LASTEXITCODE -ne 0) { Write-Warning "kirara-builder --help 返回 $LASTEXITCODE（可能是拼接产物的正常行为）" }

return $BuilderOut
