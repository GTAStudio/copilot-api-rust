# Everything-Claude-Code 全量迁移到 Rust 计划（已执行版）

> 目标：将 everything-claude-code 的 Node.js hooks 逻辑、规则/代理/技能体系迁移到 Rust 原生实现，保证跨平台、零 Node.js 依赖、单一二进制分发。

## 范围

- **Hooks（Rust 原生实现）**：SessionStart / SessionEnd / PreCompact / PreToolUse / PostToolUse / Stop / Observe
- **Matcher 表达式引擎**：完整支持 `&&` / `||` / `matches` / `==` / `!=` / 括号
- **持续学习 v2**：使用 tokio broadcast channel 替代 Unix 信号
- **规则/代理/技能**：本地 `.claude/` 目录结构，Rust/Slint/Axum 定制化
- **GUI**：Hooks 配置与状态展示

## 实施阶段

### Phase 1: 基础设施（完成）
- 新增 `rust-server/src/hooks/` 模块
- 新增 `hooks/types.rs`、`hooks/claude_paths.rs`
- 新增 `.claude/` 目录结构

### Phase 2: Matcher 引擎（完成）
- PEG 语法（pest）
- AST 解析与表达式求值
- JSONPath 风格字段访问 + 正则匹配

### Phase 3: 核心 Hooks（完成）
- session_start：加载最近会话 + 已学习技能统计
- session_end：会话持久化
- pre_compact：压缩前状态保存
- suggest_compact：工具调用计数提示
- evaluate_session：会话分析生成学习记录
- check_console_log：提交前 console.log 检查

### Phase 4: 持续学习 v2（完成）
- tokio broadcast 通道
- 观察事件落盘 JSONL
- 跨平台支持

### Phase 5: 执行引擎（完成）
- hooks.json 加载
- matcher 匹配
- Rust 内置 hook 执行
- 可选外部命令执行（无需 Node）

### Phase 6: GUI 集成（完成）
- Hooks 配置 UI
- hooks.json 路径提示与快速打开
- hooks 启用/禁用开关

### Phase 7: Skills 全量同步（完成）
- 新增 `SyncSkills` 命令：从 upstream 同步全部 skills 到 `.claude/skills`
- 自动生成 `THIRD_PARTY_NOTICES.txt`

## 交付清单（核心文件）

- `rust-server/src/hooks/*`
- `rust-server/src/hooks/matcher/*`
- `rust-server/src/hooks/observe/*`
- `rust-server/src/cli.rs`（新增 hook 子命令）
- `rust-server/src/main.rs`（启动 hooks 系统）
- `.claude/rules/*`、`.claude/agents/*`、`.claude/skills/*`
- `.claude/hooks/hooks.json`
- `gui-slint/ui/app.slint` + `gui-slint/src/hooks_config.rs`

## 注意事项

- **无 Node.js 依赖**：所有逻辑 Rust 实现
- **跨平台**：Windows/macOS/Linux
- **安全**：默认阻止无意义的 `.md` 文件创建，输出安全提示
- **性能**：不阻塞主线程，后台观察器异步写入
