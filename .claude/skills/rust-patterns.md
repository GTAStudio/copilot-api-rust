---
name: rust-patterns
description: Rust 常用惯用法与最佳实践。
---

# Rust Patterns

- 使用 `Result<T, E>` 串联错误：`?`
- 错误封装：`fmt.Errorf("%w", err)` 风格在 Rust 中使用 `thiserror`/`anyhow`
- 共享状态使用 `Arc<RwLock<T>>`
- 避免 `unsafe`
