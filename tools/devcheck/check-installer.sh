#!/usr/bin/env bash
# WSL/Linux 上对**整包**（`kachina-installer`，含 fs.rs / ipc/ / thirdparty/）做一次类型检查。
#
# 背景：devcheck 的 rust 层只把 `installer/uninstall.rs` + 几个 utils 抽进最小 crate，
# 上面那些文件的类型错误在 Linux 上抓不到，只能等 Windows runner 的 Build job 报错——
# 一轮十几分钟。这个脚本用一组 stub 编译器（`cl` / `lib` / `link` / `nasm` / `llvm-rc`）
# 让 `cargo check --target x86_64-pc-windows-msvc` 跑到底：`check` 不链接，所以 C 侧
# 产物的内容无关紧要，只要能生成出文件。
#
# 用法：
#     bash tools/devcheck/check-installer.sh            # 整包
#     bash tools/devcheck/check-installer.sh -p <crate> # 其余参数透传给 cargo
#
# 注意：只做类型检查，**不产生可用二进制**；真正的构建仍然只在 Windows 上跑。
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
stub_dir="$(mktemp -d "${TMPDIR:-/tmp}/kachina-msvc-stub.XXXXXX")"
trap 'rm -rf "$stub_dir"' EXIT

# 编译/归档/汇编 stub：解析出各自的输出参数，建一个空文件就算成功。
cat > "$stub_dir/cl.exe" <<'STUB'
#!/usr/bin/env bash
out=""
args=("$@")
i=0
while [ $i -lt ${#args[@]} ]; do
  a="${args[$i]}"
  case "$a" in
    -Fo*|/Fo*) out="${a#-Fo}"; out="${out#/Fo}" ;;
    -o) i=$((i+1)); out="${args[$i]}" ;;
  esac
  i=$((i+1))
done
if [ -z "$out" ]; then
  for a in "${args[@]}"; do
    case "$a" in
      *.c|*.cc|*.cpp|*.S|*.s) out="${a%.*}.obj"; break ;;
    esac
  done
fi
[ -n "$out" ] || out="$PWD/stub-out.obj"
mkdir -p "$(dirname "$out")" 2>/dev/null || true
: > "$out"
STUB

cat > "$stub_dir/lib.exe" <<'STUB'
#!/usr/bin/env bash
out=""
for a in "$@"; do
  case "$a" in
    -out:*|/OUT:*) out="${a#-out:}"; out="${out#/OUT:}" ;;
  esac
done
[ -n "$out" ] || out="$PWD/stub-out.lib"
mkdir -p "$(dirname "$out")" 2>/dev/null || true
: > "$out"
STUB

cat > "$stub_dir/link.exe" <<'STUB'
#!/usr/bin/env bash
out=""
for a in "$@"; do
  case "$a" in
    -out:*|/OUT:*) out="${a#-out:}"; out="${out#/OUT:}" ;;
  esac
done
[ -n "$out" ] || out="$PWD/stub-out.exe"
mkdir -p "$(dirname "$out")" 2>/dev/null || true
: > "$out"
STUB

cat > "$stub_dir/nasm" <<'STUB'
#!/usr/bin/env bash
out=""
args=("$@")
i=0
while [ $i -lt ${#args[@]} ]; do
  case "${args[$i]}" in
    -o) i=$((i+1)); out="${args[$i]}" ;;
  esac
  i=$((i+1))
done
[ -n "$out" ] || out="$PWD/stub-out.obj"
mkdir -p "$(dirname "$out")" 2>/dev/null || true
: > "$out"
STUB

# embed-resource 先跑 `-V /?` 认版本（要求 stdout 以 OVERVIEW 开头），再 `/fo <lib> …`
cat > "$stub_dir/llvm-rc" <<'STUB'
#!/usr/bin/env bash
for a in "$@"; do
  case "$a" in
    "/?"|-V)
      echo "OVERVIEW: Resource Converter (stub) no-preprocess"
      exit 0 ;;
  esac
done
out=""
args=("$@")
i=0
while [ $i -lt ${#args[@]} ]; do
  case "${args[$i]}" in
    /fo|-fo) i=$((i+1)); out="${args[$i]}" ;;
  esac
  i=$((i+1))
done
[ -n "$out" ] || out="$PWD/stub-out.lib"
mkdir -p "$(dirname "$out")" 2>/dev/null || true
: > "$out"
STUB

chmod +x "$stub_dir"/*

echo "== 用 stub 工具链对 x86_64-pc-windows-msvc 做类型检查（不链接、不出产物）"
cd "$repo_root/src-tauri"
PATH="$stub_dir:$PATH" \
CC_x86_64_pc_windows_msvc="$stub_dir/cl.exe" \
CXX_x86_64_pc_windows_msvc="$stub_dir/cl.exe" \
AR_x86_64_pc_windows_msvc="$stub_dir/lib.exe" \
RC_x86_64_pc_windows_msvc="$stub_dir/llvm-rc" \
    cargo check --target x86_64-pc-windows-msvc --locked "$@"
echo "== 类型检查通过（仅类型；可用二进制只在 Windows 上构建）"
