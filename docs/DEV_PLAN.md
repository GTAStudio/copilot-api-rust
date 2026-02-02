# Rust 全量迁移计划与进度

> 目标：将现有 copilot-api（TS/Node）完整重写为 Rust 服务，并与 Slint GUI 无缝集成，确保稳定性与性能，同时兼容 **Azure AI、GitHub Copilot、OpenAI/Anthropic API**，并保持全平台兼容。

## 文档基线（已核对）
- **Rust**：Rust 1.90+，Edition 2024（官方《Rust Book》与 Edition Guide）
- **Slint**：Slint 1.14.1（官方 docs + releases）
- **API 设计**：Rust API Guidelines

## 总体计划

### Phase 0：冻结行为与契约（完成）
- ✅ 明确 API 契约与路由：/chat/completions、/messages、/responses、/models、/usage、/token
- ✅ 兼容 /v1 前缀与 Anthropic messages
- ✅ 明确供应商：Azure / Copilot / OpenAI / Anthropic

### Phase 1：Rust 服务骨架与核心模块（完成）
- ✅ 采用 **axum + tokio + reqwest**
- ✅ 分层模块：auth / token / rate_limit / services / routes / state
- ✅ CORS & logging & tracing

### Phase 2：功能等价实现（进行中）
- ✅ **Copilot**：chat-completions、responses、models、token
- ✅ **OpenAI**：chat-completions / responses / embeddings / models
- ✅ **Anthropic**：/v1/messages、count_tokens、streaming
- ✅ **Azure**：chat-completions / responses / embeddings
- ✅ 统一 SSE 响应头（content-type/cache-control/keep-alive）
- ✅ 流式输出的边界一致性（SSE payload 细节）
- ✅ CLI 参数对齐

### Phase 3：稳定性与性能（进行中）
- ✅ reqwest 连接池与超时策略
- ✅ 并发安全状态管理（Arc + RwLock）
- ✅ 缓存层（模型/Token）完整复用
- ✅ 统一错误处理与可观测性增强

### Phase 4：GUI 集成（进行中）
- ✅ GUI 仍使用 Slint
- ✅ **内嵌服务器**：支持 Rust server exe
- ✅ 已替换旧 Bun 编译流程（Rust server build）

### Phase 5：测试与发布（待完成）
- ⏳ 路由回归测试
- ⏳ SSE/流式一致性测试
- ⏳ 性能与负载回归
- ⏳ 全平台构建验证（Windows/macOS/Linux）

## 当前进度（本地）

- ✅ Rust 服务已存在完整目录结构（rust-server/）
- ✅ 主要服务与路由已实现
- ✅ Azure/OpenAI/Anthropic/Copilot 兼容逻辑已接入
- ✅ GUI 构建脚本支持优先使用 Rust server exe

## 与原 TypeScript 版本对照（待补齐）

以下为完整阅读 TS 实现后，发现的功能/一致性缺口：

1. **Anthropic SSE 事件一致性**
   - ✅ 已补齐 tool_calls streaming 翻译（tool_use 块、input_json_delta）
   - ✅ 已补齐 message_start / content_block_start / content_block_delta / message_delta / message_stop 序列
   - ✅ 已补齐 error event（解析失败/异常）
   - ✅ 已处理空 chunk/边界情况

2. **Anthropic 非流式翻译一致性**
   - ✅ 已补齐 tool_use block 与 usage 细节

3. **Responses API → Anthropic streaming 细节**
   - TS 版本对 response.output_text.delta 做了完整事件转换，并维护 usage、message id
   - ✅ Rust 已补齐 response.completed → output_tokens
   - ⏳ input_tokens 仍为 0（TS 同样未提供）

4. **Responses API → Chat Completions streaming**
   - ✅ Rust 已补齐 final finish_reason chunk 与 usage

5. **Token 计数 / tokenizer 对齐**
   - ✅ Rust 已加入 /chat/completions 的启发式 token 估算日志
   - ✅ 可选启用 tiktoken 精确估算（COPILOT_USE_TIKTOKEN=1）

6. **Copilot Token 刷新**
   - ✅ Rust 已改为循环刷新（与 TS 行为一致）

7. **Models 端点一致性**
   - TS 版本提供 MODEL_ALIASES 和合成模型
   - ✅ Rust 已对齐主要 alias（含 legacy Claude）
   - ✅ 支持可选 alias 展示（COPILOT_EXPOSE_MODEL_ALIASES）

8. **Chat Completions 默认 max_tokens**
   - ✅ Rust 已在缺省时从模型 limits 填充 max_tokens

9. **SSE 头与响应格式**
   - 已统一 SSE 响应头（content-type/cache-control/keep-alive）
   - ✅ 已增加分块缓冲，处理 SSE 边界
   - ✅ 已补齐 error event
   - ✅ 支持多 data 行合并
   - ✅ 空事件已忽略

10. **CLI 功能对齐**
   - ✅ 已加入 `--claude-code` 启动助手（生成环境变量命令）
   - ✅ 可选复制到剪贴板（COPILOT_CLIPBOARD=1）

11. **手动审批对齐**
   - ✅ Rust 已实现控制台确认提示（与 TS 行为一致）

12. **Usage Viewer 提示**
    - ✅ 支持可选环境变量 `COPILOT_USAGE_VIEWER_URL` 输出链接

## 近期变更（本地）
- GUI build 脚本支持：
  - ../rust-server/target/release/copilot-api-server.exe
  - ../copilot-api-server.exe
- CI 构建（本地已更新，暂未推送）：优先构建 Rust server
- 新增 Rust server 跨平台 CI（Windows/macOS/Linux），包含 release build

## 下一步执行清单（立即）
1. **测试与回归**：SSE/路由/负载（已新增基础单测）
   - ✅ 已新增 tokenizer 基础单测
   - ✅ 已新增 /models alias 基础单测
   - ✅ 已新增 / health 基础单测
   - ✅ 已新增 Anthropic streaming / Responses 转换基础单测
   - ✅ 已新增 Anthropic message 翻译/工具结果拆分基础单测
   - ✅ 已新增 count_tokens 估算规则单测（claude 工具开销与倍率）
   - ✅ 已新增 map_content 图像 data URL 生成单测
   - ✅ 已新增 Responses 输入映射与 system instructions 提取单测
   - ✅ 已新增 chat_completions SSE 边界与默认模型单测
   - ✅ 已新增 Azure 配置加载单测（model prefix 与 env 回退）
   - ✅ 已新增 rate limit 基础单测（包含通过/拒绝场景）
   - ✅ 已新增 SSE 多行 data 解析单测
   - ✅ 已新增 Anthropic responses 转换与 alias 前缀单测
   - ✅ 已新增 SSE 响应头单测
   - ✅ 已通过本地 cargo test
2. **全平台构建验证**：Windows/macOS/Linux（已配置 CI）
   - ✅ 已加入 release build 验证

---

**作者**：Jason Liang
