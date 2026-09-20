<#
.SYNOPSIS
    从本仓库根目录的源码（上游 kachina-installer 快照 + 本地补丁）构建 kirara-builder.exe。

.DESCRIPTION
    产物：tools\kirara-builder.exe
    该文件是「打包器 CLI + 安装器 GUI 模板」的二进制拼接体，由上游 package.json
    的 build 脚本生成（上游产物名为 kachina-builder.exe），本脚本负责准备工具链、
    构建、再以本项目的名字拷到 tools\ 下。

    这是本仓库唯一的构建入口；产物供下游项目（HoYoEnhance）打包安装包时使用。

    仅在 Windows 上可运行（上游依赖 MSVC、windows crate、Tauri/WebView2）。

.PARAMETER Force
    即使 tools\kirara-builder.exe 已存在也重新构建。

.PARAMETER Toolchain
    Rust 工具链，默认 nightly（上游 src-tauri\rust-toolchain.toml 指定）。

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

$RepoRoot     = Split-Path -Parent $MyInvocation.MyCommand.Path
$ToolsDir     = Join-Path $RepoRoot "tools"
$BuilderOut   = Join-Path $ToolsDir "kirara-builder.exe"

# 上游 build 脚本的输出位置与文件名（target 三元组固定，产物名保持上游的 kachina-builder.exe）
$TargetTriple  = "x86_64-win7-windows-msvc"
$ReleaseDir    = Join-Path $RepoRoot "src-tauri\target\$TargetTriple\release"
$BuiltBuilder  = Join-Path $ReleaseDir "kachina-builder.exe"

function Step([string]$msg) { Write-Host "==> $msg" -ForegroundColor Cyan }
function Ok([string]$msg)   { Write-Host "    $msg" -ForegroundColor Green }

# 找出比 $reference（现有 builder）更新的源码文件；没有则返回 $null。
# 用来避免「改了源码，却仍在用旧的 kirara-builder.exe」。
# 只扫参与构建的目录与文件：tools\ 下是本项目自己的脚本，改它们不该触发重建。
function Get-NewerSource([string]$reference) {
    if (-not (Test-Path $reference)) { return $null }
    $refTime = (Get-Item $reference).LastWriteTimeUtc
    # 分隔符两种都认：脚本只在 Windows 上跑，但这样便于在别处单测
    $skip = '[\\/](node_modules|dist|target|gen|\.cache)([\\/]|$)'
    $roots = @('src', 'src-tauri', 'vendor', 'public', 'tests') |
        ForEach-Object { Join-Path $RepoRoot $_ } |
        Where-Object { Test-Path -LiteralPath $_ }
    $files = @(Get-ChildItem -Path $roots -Recurse -File -Force -ErrorAction SilentlyContinue)
    $files += @(Get-ChildItem -Path $RepoRoot -File -Force |
        Where-Object { $_.Extension -in @('.json', '.ts', '.mjs', '.yaml', '.lock') })
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
    throw "kirara-builder 只能在 Windows 上构建（Tauri + MSVC + windows crate）。"
}

if (-not (Test-Path (Join-Path $RepoRoot "package.json"))) {
    throw "未找到 $RepoRoot\package.json —— 上游源码快照缺失。"
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

Step "安装 Rust 工具链 $Toolchain + rust-src（build-std 需要）"
& rustup toolchain install $Toolchain --profile minimal
if ($LASTEXITCODE -ne 0) { throw "rustup toolchain install $Toolchain 失败" }
& rustup component add rust-src --toolchain $Toolchain
if ($LASTEXITCODE -ne 0) { throw "rustup component add rust-src 失败" }

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
if (-not $pnpm) { throw "pnpm 不可用，无法构建 kachina-installer" }
Ok("pnpm: $($pnpm.Source)")

# ---------------------------------------------------------------- 构建
Push-Location $RepoRoot
try {
    Step "pnpm install（frozen-lockfile）"
    & pnpm install --frozen-lockfile
    if ($LASTEXITCODE -ne 0) { throw "pnpm install 失败" }

    Step "pnpm build（tauri build → $TargetTriple，-Z build-std，LTO；首次冷构建较慢）"
    # 上游 build 脚本内部用 cmd 内建 ren/del/copy /b 拼接 builder + installer，
    # 必须经由 pnpm 走 cmd.exe 执行，这里不要自己重排命令。
    & pnpm build
    if ($LASTEXITCODE -ne 0) { throw "pnpm build 失败" }
} finally {
    Pop-Location
}

if (-not (Test-Path $BuiltBuilder)) {
    # 兜底：个别环境下 tauri 会落到不带三元组的 target\release
    $alt = Join-Path $RepoRoot "src-tauri\target\release\kachina-builder.exe"
    if (Test-Path $alt) { $BuiltBuilder = $alt }
}
if (-not (Test-Path $BuiltBuilder)) {
    throw "构建结束但未找到 kachina-builder.exe（预期 $ReleaseDir）"
}

Copy-Item $BuiltBuilder $BuilderOut -Force
$sizeMb = [math]::Round((Get-Item $BuilderOut).Length / 1MB, 2)
Ok("kirara-builder → $BuilderOut ($sizeMb MB)")

# 自检：能打印帮助即认为可用
& $BuilderOut --help *> $null
if ($LASTEXITCODE -ne 0) { Write-Warning "kirara-builder --help 返回 $LASTEXITCODE（可能是拼接产物的正常行为）" }

return $BuilderOut
