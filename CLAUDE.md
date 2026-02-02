# 项目 CLAUDE.md

## 项目概述
- **项目**：copilot-api-rust
- **用途**：提供 GitHub Copilot/OpenAI/Anthropic/Azure 兼容 API 服务 + Slint GUI
- **技术栈**：Rust 1.90+、Axum 0.7、Tokio、Slint 1.14、serde

## 关键规则（必须遵循）
- 安全：禁止硬编码密钥，所有输入必须校验
- 代码风格：优先小文件、低耦合、高内聚
- 测试：新增/修改逻辑必须补测
- Git：使用规范化提交信息

## 目录结构
- rust-server/：服务端 API
- gui-slint/：桌面 GUI
- .claude/：规则/代理/技能/钩子

## 重要环境变量
- COPILOT_PROVIDER
- ANTHROPIC_API_KEY
- OPENAI_API_KEY
- AZURE_OPENAI_ENDPOINT
- AZURE_OPENAI_KEY

## 可用 Hooks
- SessionStart / SessionEnd
- PreToolUse / PostToolUse
- PreCompact / Stop

## 可用代理
- planner
- architect
- tdd-guide
- code-reviewer
- security-reviewer
- rust-reviewer（本项目自定义）
