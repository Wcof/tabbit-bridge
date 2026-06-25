#!/bin/sh
# tb.sh —— tabbit-bridge 一键安装（macOS / Linux）
# 用法: curl -fsSL https://your.host/tb.sh | sh
set -eu

REPO="${REPO:-tabbit/tabbit-bridge}"
VERSION="${VERSION:-latest}"
PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"

C_GREEN='\033[32m'; C_YELLOW='\033[33m'; C_RED='\033[31m'; C_CYAN='\033[36m'; C_END='\033[0m'
info() { printf "${C_GREEN}[install]${C_END} %s\n" "$*"; }
err()  { printf "${C_RED}[install]${C_END} %s\n" "$*" >&2; }

# 1. 平台检测
detect_target() {
    os="$(uname -s)"; arch="$(uname -m)"
    case "$os/$arch" in
        Darwin/arm64)   echo "aarch64-apple-darwin" ;;
        Darwin/x86_64)  echo "x86_64-apple-darwin" ;;
        Linux/x86_64)   echo "x86_64-unknown-linux-musl" ;;
        Linux/aarch64)  echo "aarch64-unknown-linux-musl" ;;
        *) err "不支持的平台: $os/$arch"; exit 1 ;;
    esac
}
TARGET="$(detect_target)"
info "目标平台: $TARGET"

# 2. 下载二进制与 tb 控制脚本
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
URL_BIN="https://github.com/$REPO/releases/download/$VERSION/tabbit-bridge-$TARGET.tar.gz"
URL_TB="https://github.com/$REPO/releases/download/$VERSION/tb"

info "下载二进制: $URL_BIN"
curl -fsSL "$URL_BIN" -o "$TMP/bridge.tar.gz" || { err "下载二进制失败"; exit 1; }
info "下载 tb 控制器: $URL_TB"
curl -fsSL "$URL_TB" -o "$TMP/tb" || { err "下载 tb 失败"; exit 1; }

# 3. 安装到 ~/.local/bin
mkdir -p "$BIN_DIR"
tar -xzf "$TMP/bridge.tar.gz" -C "$TMP"
install -m 0755 "$TMP/tabbit-bridge" "$BIN_DIR/tabbit-bridge"
install -m 0755 "$TMP/tb"            "$BIN_DIR/tb"
info "二进制已安装至: $BIN_DIR/tabbit-bridge"
info "控制器已安装至: $BIN_DIR/tb"

# 4. PATH 提示
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) printf "${C_YELLOW}[install]${C_END} 请把 %s 加入 PATH:\n  echo 'export PATH=\"%s:\$PATH\"' >> ~/.zshrc\n" "$BIN_DIR" "$BIN_DIR" ;;
esac

# 5. 首次自举 + 注册守护 + 启动
TOKEN="$("$BIN_DIR/tabbit-bridge" --print-token)" || { err "配置自举失败"; exit 1; }
"$BIN_DIR/tb" start >/dev/null

# 6. 友好输出
if [ "$(uname -s)" = "Darwin" ]; then
    CFG="$HOME/Library/Application Support/tabbit-bridge/config.toml"
else
    CFG="$HOME/.config/tabbit-bridge/config.toml"
fi
PORT="$(awk -F'=' '/^port/ {gsub(/[ "]/,"",$2); print $2}' "$CFG" 2>/dev/null)"
printf "\n${C_YELLOW}================ tabbit-bridge 已就绪 ================${C_END}\n"
printf "  ✅ 监听: ${C_CYAN}http://127.0.0.1:%s${C_END}\n" "$PORT"
printf "  ✅ TOKEN（填入 Tabbit 妙招）:\n     ${C_CYAN}%s${C_END}\n" "$TOKEN"
printf "  ✅ 控制: ${C_CYAN}tb start | tb stop | tb status | tb token | tb logs | tb uninstall${C_END}\n"
printf "${C_YELLOW}=====================================================${C_END}\n\n"
