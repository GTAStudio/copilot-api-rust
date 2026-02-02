---
name: axum-patterns
description: Axum 路由、错误与中间件实践。
---

# Axum Patterns

- 路由按域拆分模块
- 错误类型统一实现 `IntoResponse`
- 使用 `Extension`/`State` 共享上下文
- 中间件使用 `tower::ServiceBuilder`
