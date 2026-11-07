#!/usr/bin/env bash
# White Album 2 · native engine — macOS (Apple Silicon) launcher.
#
#   ./run-mac.sh            标题画面
#   ./run-mac.sh 2003       直接进入某个场景（调试用）
#   ./run-mac.sh --check    只检查依赖和素材，不启动
#   ./run-mac.sh --data     只准备素材
#
# 与 Linux 的 run.sh 的差别：没有 AppImage、没有系统包管理器自动安装，
# 依赖来自 Homebrew；素材用 launcher/fetch_data_mac.py 并行下载后逐卷解压，
# 峰值磁盘占用远低于“先全下完再解压”。
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"

BIN=./target/release/wa2-native
DATA="${WA2_DATA:-$PWD/data}"
export WA2_DATA="$DATA"

# 4x is the only art tree a macOS install keeps (the 8x masters are 22 GB and a
# Retina drawable never asks for more than 4x). `auto` decode follows the
# drawable instead of forcing the maximum scale, which is what keeps windowed
# play light: a 1280x720-point window is a 2560x1440 canvas, so 2x is already
# pixel-exact and 4x would only be supersampled away.
export WA2_ART_SCALE="${WA2_ART_SCALE:-4}"
export WA2_RENDER_SCALE="${WA2_RENDER_SCALE:-auto}"
# 超长 scroll CG（v200000 1280x3362、v201900 1280x2160）解码一次要 1~4 秒，
# 默认的磁盘缓存上限 2x 会让它们每次冷启动都重新解码，屏幕停在黑幕上。
# 把磁盘缓存上限提到 4x，让这类大图解码一次后就一直走缓存，消除黑屏。
export WA2_ART_DISK_SCALE="${WA2_ART_DISK_SCALE:-4}"

mode=play
case "${1:-}" in
  --check) mode=check ;;
  --data)  mode=data ;;
  --help|-h)
    sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
    exit 0 ;;
esac

missing=()
# Probe the libraries themselves rather than asking Homebrew: this script also
# runs from a Finder double-click, where `brew` is not on PATH and `brew list`
# would report every package missing on a machine that has all of them.
lib_present() {
  local name="$1" prefix
  for prefix in /opt/homebrew /usr/local /opt/local; do
    [ -e "$prefix/lib/$name" ] && return 0
  done
  return 1
}
lib_present libSDL2-2.0.0.dylib || missing+=("sdl2")
lib_present libfreetype.6.dylib || missing+=("freetype")
command -v ffmpeg >/dev/null 2>&1 || [ -x /opt/homebrew/bin/ffmpeg ] || [ -x /usr/local/bin/ffmpeg ] || missing+=("ffmpeg")
if [ ${#missing[@]} -gt 0 ]; then
  echo "缺少 Homebrew 依赖：${missing[*]}"
  echo "请先运行：brew install ${missing[*]}"
  exit 1
fi

if [ ! -d "$DATA/script" ]; then
  if [ "$mode" = check ]; then
    echo "素材尚未就绪：${DATA}（正式启动会自动下载）"
    exit 1
  fi
  echo "素材尚未就绪（没有 $DATA/script），开始下载。约 15 GB（只取 4x 素材树），可中断续传。"
  python3 -u launcher/fetch_data_mac.py
fi

[ "$mode" = data ] && exit 0
[ -f "$DATA/script" ] || [ -d "$DATA/script" ] || { echo "素材目录不完整：$DATA"; exit 1; }

if [ ! -x "$BIN" ]; then
  echo "编译引擎……"
  cargo build --release
fi

[ "$mode" = check ] && { echo "依赖与素材就绪：$DATA"; exit 0; }

# 黑屏排查：把引擎的 stderr 日志（每帧 scene/bg 状态）同时落盘，
# 这样从 Finder 双击 .app 或正常游玩时，黑屏那一瞬间的原因也会被记下来。
LOG="${WA2_LOG:-$PWD/wa2.log}"
if [ "${WA2_NOLOG:-0}" != "1" ]; then
  : > "$LOG"
  echo "引擎日志写入：$LOG（设 WA2_NOLOG=1 可关闭）" >&2
fi

run() {
  if [ "${WA2_NOLOG:-0}" != "1" ]; then
    "$BIN" "$@" 2>&1 | tee -a "$LOG"
  else
    "$BIN" "$@"
  fi
}

if [ $# -eq 0 ] || [ "$1" = title ]; then
  run title
  exit $?
fi
if [[ "$1" =~ ^[0-9]+$ ]]; then
  run run "$1"
  exit $?
fi
run "$@"
exit $?
