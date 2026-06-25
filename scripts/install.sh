#!/bin/sh
# tabbit-bridge 一键安装脚本（macOS / Linux）
# 用法: curl -fsSL https://<release>/install.sh | sh
set -eu

PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"
APP_NAME="tabbit-bridge"
REPO="${REPO:-tabbit/tabbit-bridge}"  # 替换为实际 release 仓库

err() { printf '\033[31m[install]\033[0m %s\n' "$*" >&2; }
info() { printf '\033[32m[install]\033[0m %s\n' "$*"; }

# 1. 检测平台
detect_target() {
    os="$(uname -s)"
    arch="$(uname -m)"
    case "$os/$arch" in
        Darwin/arm64)  echo "aarch64-apple-darwin" ;;
        Darwin/x86_64) echo "x86_64-apple-darwin" ;;
        Linux/x86_64)  echo "x86_64-unknown-linux-musl" ;;
        Linux/aarch64) echo "aarch64-unknown-linux-musl" ;;
        *) err "不支持的平台: $os/$arch"; exit 1 ;;
    esac
}

TARGET="$(detect_target)"
info "目标平台: $TARGET"

# 2. 下载二进制
VERSION="${VERSION:-latest}"
URL="https://github.com/${REPO}/releases/download/${VERSION}/tabbit-bridge-${TARGET}.tar.gz"
info "下载: $URL"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
curl -fsSL "$URL" -o "$TMP/bridge.tar.gz" || { err "下载失败"; exit 1; }
mkdir -p "$BIN_DIR"
tar -xzf "$TMP/bridge.tar.gz" -C "$TMP"
install -m 0755 "$TMP/tabbit-bridge" "$BIN_DIR/$APP_NAME" || { err "安装二进制失败"; exit 1; }
info "二进制已安装至: $BIN_DIR/$APP_NAME"

# 3. 首次自举配置（二进制内部会自动生成 config.toml + 0600 token）
# 先跑一次 --print-token 触发配置生成并取回 token
TOKEN="$("$BIN_DIR/$APP_NAME" --print-token)" || { err "配置自举失败"; exit 1; }

# 4. 注册守护并立即启动
info "注册后台守护..."
"$BIN_DIR/$APP_NAME" --install || { err "守护注册失败（可手动 --install 重试）"; exit 1; }

# 5. 打印 token（不进任何日志，仅终端一次性输出）
printf '\n\033[33m================ tabbit-bridge 安装完成 ================\n'
printf '监听地址: 127.0.0.1（端口见 config.toml）\n'
printf '配置路径: '
case "$(uname -s)" in
    Darwin) printf '~/Library/Application Support/tabbit-bridge/config.toml\n' ;;
    Linux)  printf '~/.config/tabbit-bridge/config.toml\n' ;;
esac
printf 'TOKEN（填入妙招脚本，请勿泄露）:\n'
printf '\033[36m%s\033[0m\n' "$TOKEN"
printf '========================================================\n\n'
info "如需卸载: $BIN_DIR/$APP_NAME --uninstall"
