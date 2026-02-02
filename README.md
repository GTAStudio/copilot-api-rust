# GitHub Copilot API GUI (Rust Integrated)

A Windows GUI built with Slint/Rust that **embeds the Rust copilot-api server**. This is the fully integrated Rust version (no Node/Bun required).

## Features

- **One-Click Start** - Embedded Rust server, works out of the box
- **Copilot Auth** - GitHub Device Code authentication flow
- **Proxy Support** - HTTP/SOCKS5 proxy configuration
- **Model Selection** - Auto-fetch available models list
- **Azure/OpenAI/Anthropic** - Multi-provider compatibility
- **Log Viewer** - Built-in real-time log display

## Artifacts

- **GUI (embedded server)**: deployment/copilot-api-gui.exe
- **Standalone server**: deployment/copilot-api-server.exe

## Usage (GUI)

1. Run deployment/copilot-api-gui.exe
2. Click **Copilot Auth** to complete GitHub device code authentication
3. Configure port and account type
4. Click **Start Server** to launch the service
5. Use http://localhost:PORT as the API endpoint in your application

## Usage (Server Only)

Run the standalone server if you don’t need the GUI:

```
deployment/copilot-api-server.exe start --host 127.0.0.1 --port 8989
```

## Configuration

| Option | Description |
|--------|-------------|
| Port | API server port (default: 4141) |
| Account Type | GitHub account type: individual or enterprise |
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
cargo build --release

# 2. Build GUI (embeds server)
cd ..\gui-slint
cargo build --release
```

Outputs:
- gui-slint/target/release/copilot-api-gui.exe
- rust-server/target/release/copilot-api-server.exe

## Tech Stack

- **GUI Framework**: Slint v1.14
- **Backend Service**: copilot-api (Rust, embedded)
- **Bundler**: flate2 compression

## Download

Download the latest release from [GitHub Releases](https://github.com/GTAStudio/copilot-api-rust/releases).

## License

MIT License

---

# GitHub Copilot API GUI（中文）

一个基于 Slint/Rust 的 Windows 图形界面程序，**内嵌 Rust 版 copilot-api 服务端**。

## 功能特性

- **一键启动** - 内嵌 Rust 服务端，开箱即用
- **Copilot 认证** - 支持 GitHub Device Code 登录流程
- **代理配置** - 支持 HTTP/SOCKS5 代理设置
- **模型选择** - 自动获取可用模型列表
- **Azure/OpenAI/Anthropic** - 多供应商兼容
- **日志查看器** - 内置实时日志显示

## 产物

- **GUI（内嵌服务）**：deployment/copilot-api-gui.exe
- **独立服务端**：deployment/copilot-api-server.exe

## 下载

从 [GitHub Releases](https://github.com/GTAStudio/copilot-api-rust/releases) 下载最新版本。

## 使用方法（GUI）

1. 运行 deployment/copilot-api-gui.exe
2. 点击 **Copilot Auth** 完成 GitHub 设备码认证
3. 配置端口和账户类型
4. 点击 **Start Server** 启动服务
5. 在你的应用中使用 http://localhost:端口 作为 API 端点

## 使用方法（仅服务端）

```
deployment/copilot-api-server.exe start --host 127.0.0.1 --port 8989
```

## 配置说明

| 选项 | 说明 |
|------|------|
| Port | API 服务端口（默认 4141） |
| Account Type | GitHub 账户类型：individual 或 enterprise |
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
cargo build --release

# 2. 构建 GUI（内嵌服务端）
cd ..\gui-slint
cargo build --release
```

---

**Author / 作者**: Jason Liang
