# GitHub Copilot API GUI (Rust Integrated)

A Windows GUI built with Slint/Rust that **embeds the Rust copilot-api server**. This is the fully integrated Rust version (no Node/Bun required).

Use the top-right **中文 / English** selector to change the interface language
without restarting. The preference is saved independently of unsaved settings.
Labels, tooltips, application status, and dependency results follow the selection;
upstream logs and technical error details remain in their original language.

## Current Compatibility And Security

See [Client Configuration](docs/CLIENTS.md) for current GitHub Copilot Custom
Endpoint, Claude Code, and Claude Desktop Gateway setup. Rust 1.92+ is required.
Real account and desktop acceptance must be verified separately from local tests.
See the [2026-09-14 Audit](docs/AUDIT-2026-09-14.md) for findings, dependency
versions, verification, and remaining live-client acceptance work. Current Windows
production line coverage is 82.66% for the server and 85.02% for the GUI.

The server defaults to loopback-only access. Set `COPILOT_API_KEY` for authenticated
access; non-loopback binding requires it. Browser-origin requests and raw-token
HTTP endpoints are disabled. Claude models are no longer silently mapped to GPT.

The GUI's **Configure Claude Code** button (under **Models & Clients**) applies client settings explicitly.
Starting or saving the GUI does not rewrite global Claude configuration.

## Features

- **GTAStudio Branding** - Original GameCheater logo, gold/charcoal theme, light/dark appearance, and embedded Windows icons
- **One-Click Start** - Embedded Rust server, works out of the box
- **Copilot Auth** - GitHub Device Code authentication flow
- **Proxy Support** - HTTP/SOCKS5 proxy configuration
- **Model Selection** - Auto-fetch available models list
- **Azure/OpenAI/Anthropic** - Multi-provider compatibility
- **Log Viewer** - Built-in real-time log display
- **Claude Code Integration** - Rust-native hooks + full skills sync

## Artifacts

- **GUI (embedded server)**: [gui-slint/target/release/copilot-api-gui.exe](gui-slint/target/release/copilot-api-gui.exe)
- **Standalone server**: [rust-server/target/release/copilot-api-server.exe](rust-server/target/release/copilot-api-server.exe)

## Usage (GUI)

1. Run the GUI from `gui-slint/target/release` after building, or from your extracted release download
2. Select **English**, then open **Connection** and click **Sign In to GitHub Copilot**
3. Configure port and account type
4. Click **Start Server** to launch the service
5. Use http://localhost:PORT as the API endpoint in your application

## Usage (Server Only)

Run the standalone server if you don’t need the GUI:

```
./rust-server/target/release/copilot-api-server.exe start --host 127.0.0.1 --port 8989
```

## Claude Code Integration

- **Hooks**: Config at .claude/hooks/hooks.json
- **Enable/Disable**: GUI switch or env COPILOT_HOOKS_ENABLED=0
- **Sync skills (full)**:

```
copilot-api-server.exe sync-skills
```

This downloads missing skills from a fixed everything-claude-code revision into
.claude/skills, preserves existing local files, and writes THIRD_PARTY_NOTICES.txt.
External command hooks require `COPILOT_ALLOW_COMMAND_HOOKS=1`. Observation logs
are opt-in (`COPILOT_OBSERVATIONS_ENABLED=1`) and contain metadata only.

## Configuration

| Option | Description |
|--------|-------------|
| Provider | Explicit Copilot, Anthropic, OpenAI, or Azure; auto preserves legacy inference |
| Port | API server port (default: 4141) |
| Account Type | GitHub account type: individual, business, or enterprise |
| Request Interval | Seconds between requests, from 0 to 86400 |
| Proxy URL | Proxy server address (optional) |
| Model | Model to use, click refresh to get available list |

### Provider Environment Variables

- **Copilot (default)**: no extra env required
- **OpenAI**: set COPILOT_PROVIDER=openai and OPENAI_API_KEY
- **Anthropic**: set COPILOT_PROVIDER=anthropic and ANTHROPIC_API_KEY
- **Azure OpenAI**: set COPILOT_PROVIDER=azure, AZURE_OPENAI_ENDPOINT, AZURE_OPENAI_KEY, AZURE_OPENAI_DEPLOYMENT

## Build from Source

```
# 1. Build Rust server
cd rust-server
cargo build --release --locked

# 2. Build GUI (embeds server)
cd ..\gui-slint
cargo build --release --locked
```

Outputs:
- gui-slint/target/release/copilot-api-gui.exe
- rust-server/target/release/copilot-api-server.exe

## Tech Stack

- **GUI Framework**: Slint v1.17.1
- **Backend Service**: copilot-api (Rust, embedded)
- **Bundler**: flate2 compression

## Download

Download the latest release from [GitHub Releases](https://github.com/GTAStudio/copilot-api-rust/releases).

## License

MIT License

---

# GitHub Copilot API GUI（中文）

一个基于 Slint/Rust 的 Windows 图形界面程序，**内嵌 Rust 版 copilot-api 服务端**。

右上角 **中文 / English** 可即时切换界面语言，无需重启，并自动记住选择。
切换语言不会保存其他未提交的设置。界面文案、提示和环境结果随语言更新，
上游日志与底层错误详情保留原文。

新版 GitHub Copilot Custom Endpoint、Claude Code 与 Claude Desktop Gateway 接入方法见
[客户端配置](docs/CLIENTS.md)。需要 Rust 1.92+。默认仅供本机原生客户端使用，远程监听必须设置
`COPILOT_API_KEY` 并部署 HTTPS 反向代理。已关闭原始 token HTTP 接口和任意来源 CORS。

GUI 不再在启动或保存时自动改写 Claude 设置；请在 **模型与客户端** 中明确点击 **配置 Claude Code**。
模型以真实上游目录为准，不再把 Claude 请求静默替换为 GPT。真实账号与桌面 App 联调须单独验证。

## 功能特性

- **统一品牌界面** - 复用 GameCheater 原始 logo、金色与炭黑主题，支持明暗切换，内嵌 Windows 多尺寸图标
- **一键启动** - 内嵌 Rust 服务端，开箱即用
- **Copilot 认证** - 支持 GitHub Device Code 登录流程
- **代理配置** - 支持 HTTP/SOCKS5 代理设置
- **模型选择** - 自动获取可用模型列表
- **Azure/OpenAI/Anthropic** - 多供应商兼容
- **日志查看器** - 内置实时日志显示
- **Claude Code 集成** - Rust 原生 hooks + 全量 skills 同步

## 产物

- **GUI（内嵌服务）**：[gui-slint/target/release/copilot-api-gui.exe](gui-slint/target/release/copilot-api-gui.exe)
- **独立服务端**：[rust-server/target/release/copilot-api-server.exe](rust-server/target/release/copilot-api-server.exe)

## 下载

从 [GitHub Releases](https://github.com/GTAStudio/copilot-api-rust/releases) 下载最新版本。

## 使用方法（GUI）

1. 运行构建生成的 GUI（`gui-slint/target/release`），或下载版本中解压出的 GUI
2. 在 **服务连接** 中点击 **登录 GitHub Copilot** 完成设备码认证
3. 配置端口和账户类型
4. 点击 **启动服务** 启动服务
5. 在你的应用中使用 http://localhost:端口 作为 API 端点

## 使用方法（仅服务端）

```
./rust-server/target/release/copilot-api-server.exe start --host 127.0.0.1 --port 8989
```

## Claude Code 集成

- **Hooks**：配置文件在 .claude/hooks/hooks.json
- **启用/禁用**：GUI 开关或环境变量 COPILOT_HOOKS_ENABLED=0
- **全量同步 skills**：

```
copilot-api-server.exe sync-skills
```

此命令从固定上游版本下载缺少的 skills，保留已有本地文件，并写入 THIRD_PARTY_NOTICES.txt。
外部命令 hooks 需明确设置 `COPILOT_ALLOW_COMMAND_HOOKS=1`，观察日志默认关闭且只记录元数据。

## 配置说明

| 选项 | 说明 |
|------|------|
| Provider | 明确选择 Copilot、Anthropic、OpenAI 或 Azure；auto 保留旧配置推断 |
| Port | API 服务端口（默认 4141） |
| Account Type | GitHub 账户类型：individual、business 或 enterprise |
| Request Interval | 请求间隔秒数，范围 0 到 86400 |
| Proxy URL | 代理服务器地址（可选） |
| Model | 使用的模型，点击刷新按钮获取可用列表 |

### 供应商环境变量

- **Copilot（默认）**：无需额外环境变量
- **OpenAI**：设置 COPILOT_PROVIDER=openai 与 OPENAI_API_KEY
- **Anthropic**：设置 COPILOT_PROVIDER=anthropic 与 ANTHROPIC_API_KEY
- **Azure OpenAI**：设置 COPILOT_PROVIDER=azure、AZURE_OPENAI_ENDPOINT、AZURE_OPENAI_KEY、AZURE_OPENAI_DEPLOYMENT

## 从源码构建

```
# 1. 构建 Rust 服务端
cd rust-server
cargo build --release --locked

# 2. 构建 GUI（内嵌服务端）
cd ..\gui-slint
cargo build --release --locked
```

---

**Author / 作者**: Jason Liang
