# tabbit-bridge

一个运行在用户本机的微型本地服务，作为 **Tabbit 浏览器**（基于 Chromium）页面脚本与本地 CLI 工具之间的**安全桥梁**。首要服务对象是 `rtk`（Rust CLI 代理，输出 token 节省统计）与 `ccusage`（统计各 AI CLI 用量与成本）。

核心目标优先级：**安全 > 静默 > 低资源 > 易扩展**。

---

## 一键安装

一行命令完成下载、配置自举、守护注册、立即启动、打印 token：

```bash
curl -fsSL https://github.com/tabbit/tabbit-bridge/releases/latest/download/tb.sh | sh
```

安装完成后终端会打印监听端口与 TOKEN。把 `~/.local/bin` 加入 PATH 即可直接使用 `tb` 控制器：

```bash
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc
```

> 自定义仓库或版本：`curl -fsSL .../tb.sh | REPO=你的/tabbit-bridge VERSION=v1.1.0 sh`

---

## `tb` 命令使用

`tb` 是随安装一并下放到 `~/.local/bin/tb` 的轻量 shell 控制器，封装了对 launchd / systemd 的日常操作：

| 命令 | 作用 |
| :--- | :--- |
| `tb start` | 启动服务（首次会自动注册守护进程） |
| `tb stop` | 停止服务 |
| `tb restart` | 重启服务 |
| `tb status` | 查看运行状态（PID / �端口 / 内存） |
| `tb token` | 打印当前 TOKEN（填入妙招脚本） |
| `tb logs` | �跟随 out/err 日志 |
| `tb uninstall` | 彻底卸载（守护进程 / 配置 / 二进制 / tb 软链全清） |
| `tb help` | 显示帮助 |

示例：

```bash
tb status        # [tb] 运行中 · PID=12345 · 端口=47113 · 内存=2.4 MB
tb token         # a1b2c3...（64 字符 hex）
tb logs          # �跟随日志，Ctrl+C 退出
```

---

## 信任模型与边界说明（务必先读）

⚠️ **本方案的安全前提是个人本机使用，不适合把含 token 的妙招公开分发。**

- token 需嵌入妙招代码运行在页面上下文，因此任何能看到该妙招源码的人都能取走 token。
- 若未来要做面向他人分发的产品能力，需另起基于**浏览器扩展 + Native Messaging** 的方案，本仓库不涵盖。
- 本服务以**普通用户身份**运行，绝不 root/管理员；仅监听 `127.0.0.1`，绝不监听 `0.0.0.0`。

---

## 安全设计（纵深防御四层）

1. **网络隔离** — 仅 `bind 127.0.0.1`，高位端口（40000~60000）首次启动随机生成，降低被扫描命中概率。
2. **密钥鉴权** — 256-bit 随机 token 写入权限 `0600` 的 `config.toml`；受保护请求须带 `Authorization: Bearer <token>`，服务端用 `subtle` **常量时间比较**校验，失败返回 `401`。
3. **防 DNS 重绑定 + CORS/PNA** — 校验 `Host` 头必须为 `127.0.0.1:<port>` / `localhost:<port>` / `[::1]:<port>`，否则 `403`；正确处理 `OPTIONS` 预检并返回 `Access-Control-Allow-Private-Network: true`，否则高版本 Chromium 会拦截 HTTPS 页面对本地接口的请求。
4. **白名单 + 无 shell 执行** — 客户端只传 `action` 标识符；执行时用 `Command::new(prog).args(固定参数数组)`，**绝不使用 `sh -c` 或任何字符串拼接**，从根本上消灭命令注入。未命中白名单返回 `404`。

工程兜底：每条命令施加超时（默认 5s）与输出上限（默认 2MB），超出截断并置 `truncated=true`；全程禁止 `unsafe`；token 与命令输出绝不写入日志。

---

## 安装

### macOS / Linux

```bash
curl -fsSL https://github.com/<your-release>/install.sh | sh
```

脚本会：检测平台下载二进制 → 自举生成随机端口与 token（`0600`）→ 注册后台守护并立即启动 → 在终端打印 token 供填入妙招。

### Windows（管理员 PowerShell）

```powershell
iwr -useb https://github.com/<your-release>/install.ps1 | iex
```

---

## 配置文件

| 平台 | 路径 |
|------|------|
| macOS | `~/Library/Application Support/tabbit-bridge/config.toml` |
| Linux | `~/.config/tabbit-bridge/config.toml` |
| Windows | `%APPDATA%\tabbit-bridge\config.toml` |

```toml
[server]
bind = "127.0.0.1"
port = 47113                 # 安装时随机生成
token = "..."                # 256-bit hex，权限 0600

[limits]
timeout_ms = 5000
max_output_bytes = 2000000
rate_per_min = 60
```

可用 `--config-dir <dir>` 覆盖配置目录（也支持环境变量 `TABBIT_BRIDGE_CONFIG_DIR`）。

---

## HTTP API

### `GET /healthz`（免鉴权）

```json
{ "status": "ok", "version": "1.0.0" }
```

### `POST /v1/exec`（需 `Authorization: Bearer <token>`）

请求体：
```json
{ "action": "rtk_gain" }
```

统一响应：
```json
{
  "ok": true,
  "action": "rtk_gain",
  "exit_code": 0,
  "data": { },
  "raw": null,
  "stderr": "",
  "duration_ms": 8,
  "truncated": false
}
```

错误码：`401` 鉴权失败 · `403` Host 校验失败 · `404` 非法 action · `408` 命令超时 · `500` 执行错误。

---

## 白名单

编译期静态映射，新增指令必须改代码重编译：

| action | 程序 | 固定参数 | JSON 解析 |
|--------|------|---------|----------|
| `rtk_gain` | `rtk` | `gain --all --format json` | ✓ |
| `rtk_discover` | `rtk` | `discover --all --since 7` | ✗ |
| `cc_daily` | `ccusage` | `daily --json` | ✓ |
| `cc_monthly` | `ccusage` | `monthly --json` | ✓ |
| `cc_daily_at` | `ccusage` | `daily --json --since <date>` | ✓（受控参数，须匹配 `^\d{4}-\d{2}-\d{2}$`） |

受控参数作为**独立 argv 元素**传入，格式不符直接拒绝，绝不拼接 shell 字符串。

---

## 守护进程

| 平台 | 机制 | 文件 |
|------|------|------|
| macOS | launchd | `~/Library/LaunchAgents/com.tabbit.bridge.plist` |
| Linux | systemd-user | `~/.config/systemd/user/tabbit-bridge.service` |
| Windows | windows-service（无黑窗：`#![windows_subsystem="windows"]`） | SCM 服务 `tabbit-bridge` |

注册/注销：
```bash
tabbit-bridge --install
tabbit-bridge --uninstall
tabbit-bridge --print-token   # 仅 stdout 输出当前 token，不进日志
```

---

## Tabbit 端集成（妙招脚本）

```javascript
(async () => {
  const BASE = 'http://127.0.0.1:47113';   // 端口以 config.toml 为准
  const TOKEN = '把安装时打印的 token 填这里';

  const exec = async (action) => {
    const r = await fetch(`${BASE}/v1/exec`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'Authorization': `Bearer ${TOKEN}`,
      },
      body: JSON.stringify({ action }),
    });
    if (!r.ok) throw new Error(`bridge ${r.status}`);
    return r.json();
  };

  try {
    const [gain, daily] = await Promise.all([exec('rtk_gain'), exec('cc_daily')]);
    console.log('rtk:', gain.data, 'ccusage:', daily.data);
  } catch (e) {
    console.error('无法连接 tabbit-bridge:', e);
  }
})();
```

---

## 从源码构建

```bash
cargo build --release
# 产物: target/release/tabbit-bridge
```

依赖（仅以下，无 tokio/axum 等异步重型栈）：`tiny_http` · `serde` · `serde_json` · `toml` · `getrandom` · `subtle` · `wait-timeout`（Windows 另加 `windows-service`）。

---

## 许可

MIT。

---

## 第 17 章 · 验收用例自测说明

以下命令假设已 `cargo build --release`，二进制在 `target/release/tabbit-bridge`，配置目录用 `TABBIT_BRIDGE_CONFIG_DIR` 隔离到一个临时目录避免污染真实配置。所有 `curl` 都强制带 `Host` 头以模拟浏览器 fetch。

### 准备：启动一个隔离实例

```bash
export TB_DIR=$(mktemp -d)
target/release/tabbit-bridge --config-dir "$TB_DIR" &
TB_PID=$!
# 取回自举生成的 token 与端口
TOKEN=$(target/release/tabbit-bridge --config-dir "$TB_DIR" --print-token)
PORT=$(grep '^port' "$TB_DIR/config.toml" | awk '{print $3}' | tr -d ' ')
echo "port=$PORT token=$TOKEN"
```

### 用例 1 · 正确 token 调用白名单命令返回 ok:true 与解析后的 JSON

```bash
curl -s -X POST "http://127.0.0.1:$PORT/v1/exec" \
  -H "Host: 127.0.0.1:$PORT" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"action":"rtk_gain"}'
# 期望: HTTP 200, body 含 "ok":true, "data": <rtk 的 JSON 输出>, "truncated":false
# 注: 若本机未装 rtk, exit_code 非 0, ok 为 false, raw/stderr 给出原因 —— 这是预期行为
```

### 用例 2 · 错误/缺失 token 返回 401，且比对为常量时间实现

```bash
# 缺失 token
curl -s -o /dev/null -w '%{http_code}\n' -X POST "http://127.0.0.1:$PORT/v1/exec" \
  -H "Host: 127.0.0.1:$PORT" -H "Content-Type: application/json" -d '{"action":"rtk_gain"}'
# 期望: 401

# 错误 token
curl -s -o /dev/null -w '%{http_code}\n' -X POST "http://127.0.0.1:$PORT/v1/exec" \
  -H "Host: 127.0.0.1:$PORT" -H "Authorization: Bearer deadbeef" \
  -H "Content-Type: application/json" -d '{"action":"rtk_gain"}'
# 期望: 401

# 常量时间比对: 见源码 server.rs bearer_ok() 使用 subtle::ConstantTimeEq;
# 单元化验证可用 cargo test 跑一个对 ct_eq 的针对性测试,或用 time(1) 对比
# 正确/错误 token 的响应耗时方差应无明显差异（subtle 已保证）。
```

### 用例 3 · 非白名单 action 返回 404 且不执行任何系统调用

```bash
curl -s -o /dev/null -w '%{http_code}\n' -X POST "http://127.0.0.1:$PORT/v1/exec" \
  -H "Host: 127.0.0.1:$PORT" -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" -d '{"action":"rm_rf"}'
# 期望: 404
# 不执行系统调用: registry::resolve("rm_rf") 返回 None, 根本不进入 exec::run。
```

### 用例 4 · 伪造 Host 头返回 403

```bash
curl -s -o /dev/null -w '%{http_code}\n' -X POST "http://127.0.0.1:$PORT/v1/exec" \
  -H "Host: evil.com" -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" -d '{"action":"rtk_gain"}'
# 期望: 403
```

### 用例 5 · OPTIONS 预检返回含 Private-Network 的正确 CORS 头

```bash
curl -s -i -X OPTIONS "http://127.0.0.1:$PORT/v1/exec" \
  -H "Host: 127.0.0.1:$PORT" \
  -H "Origin: https://tabbit.example" \
  -H "Access-Control-Request-Method: POST" \
  -H "Access-Control-Request-Headers: Authorization, Content-Type" \
  | grep -iE 'access-control'
# 期望出现:
#   Access-Control-Allow-Origin: *
#   Access-Control-Allow-Methods: GET, POST, OPTIONS
#   Access-Control-Allow-Headers: Authorization, Content-Type
#   Access-Control-Allow-Private-Network: true
```

### 用例 6 · 超时返回 408；超大输出截断且 truncated:true

需临时把白名单指向一个会卡 / 会爆输出的命令。本仓库已内置单元测试覆盖：

```bash
cargo test --quiet exec::tests
# 包含:
#   timeout_works     -> sleep 30 在 200ms 超时, timed_out=true, exit_code=None
#   truncation_works  -> yes x 输出在 1024B 截断, truncated=true
#   normal_exec       -> echo hello 正常 exit_code=0
```

如需端到端验证超时：把白名单某条改为 `sleep 30`（需改代码重编译）后调用，期望 HTTP 408 与 `ok:false`、`exit_code:null`。

### 用例 7 · 空闲常驻内存 < 8MB，无外部动态依赖

```bash
# 内存: 启动后空闲一段时间取样 RSS
ps -o rss= -p $TB_PID | awk '{print $1/1024 " MB"}'
# 期望 < 8 MB（tiny_http 同步栈, 无 tokio）

# 无外部动态依赖（musl 静态构建尤甚；macOS 仅系统 libc）:
otool -L target/release/tabbit-bridge        # macOS
ldd     target/release/tabbit-bridge         # Linux musl 应输出 "not a dynamic executable"
```

### 用例 8 · 三平台守护进程注册/注销、开机自启、无窗口无图标

```bash
# macOS
target/release/tabbit-bridge --config-dir "$TB_DIR" --install
launchctl list | grep com.tabbit.bridge           # 期望列出且 PID 非 -
ls ~/Library/LaunchAgents/com.tabbit.bridge.plist # 期望存在
target/release/tabbit-bridge --uninstall
launchctl list | grep -c com.tabbit.bridge        # 期望 0

# Linux (systemd-user)
target/release/tabbit-bridge --config-dir "$TB_DIR" --install
systemctl --user is-active tabbit-bridge         # 期望 active
systemctl --user is-enabled tabbit-bridge        # 期望 enabled
target/release/tabbit-bridge --uninstall
systemctl --user is-active tabbit-bridge         # 期望 inactive (已注销)

# Windows
#   以管理员 PowerShell 运行:
#   tabbit-bridge --install ; Get-Service tabbit-bridge
#   tabbit-bridge --uninstall
#   无黑窗由 #![windows_subsystem="windows"] 保证。
```

### 清理

```bash
kill $TB_PID 2>/dev/null
rm -rf "$TB_DIR"
```
