# 历史 beta.3 测试记录

日期：2026-08-05
版本：`0.1.0-beta.3`
记录性质：维护者在 2026-08-05 记录的历史测试结果；未在当前公开快照上重新执行，不构成当前版本或后续 beta 的验收报告。

## 范围

- 只连接 Codex App、Codex CLI 与官方 CC-Panes；
- 不启动 LiveAgent Workflow Fabric；
- 不修改官方 CC-Panes 二进制、数据库、配置和真实项目；
- 使用独立 Cargo workspace 和单一构建缓存。

## 已验证

| 门禁 | 状态 | 证据 |
|---|---|---|
| `cargo fmt --check` | pass | 本地执行 |
| `cargo check --all-targets --locked` | pass | 本地执行 |
| 单元、CLI 契约与启动隔离 | pass | 12 + 2 + 2 项 |
| Console 子进程环境隔离 | pass | 合成 `cmd.exe` child |
| GUI detached 环境隔离 | pass | 合成 `.cmd` child |
| 配置预览、写入、备份和旧字段迁移 | pass | 单元与隔离安装回归 |
| Release 构建 | pass | 两个 Windows x64 二进制 |
| 包结构与 SHA-256 | pass | `verify-package.ps1` |
| 隔离安装、同版本重装、卸载 | pass | 临时用户目录 |
| 安装包篡改拒绝 | pass | 修改 README 后校验失败 |
| 真实 CC-Panes probe | pass | `0.11.10`、`0.11.11` |
| 真实 Codex CLI probe | pass | `0.146.0` |
| 遗留 Workflow MCP | pass | `enabled=false`，无运行进程 |
| 大型 Rust target 扫描 | pass | 5 个目录，均低于 5 GiB |
| 实际用户级安装 | pass | 独立目录、开始菜单、卸载注册 |
| 上游自动升级透明性 | pass | CC-Panes 升级后无需重构或迁移 |

## 预期告警

从 Codex Desktop 内执行 doctor 时，`parent_environment=warn`。它表示当前父进程含有
任务级 Codex 私有变量；Console 和 GUI 两条启动链均已验证会在 CC-Panes 子进程边界
移除这些变量。

## 发布前门禁

- 本机 stable Rust toolchain 缺少 `clippy` 组件，本地 Clippy 为 `blocked`；GitHub CI
  已配置安装 `rustfmt, clippy` 并以 `-D warnings` 执行。
- 本地测试包未配置 Authenticode 证书，签名状态为 unsigned。公开生产发布必须使用
  `-RequireSignature`。
- 当前不执行真实 CC-Panes Session 启动，避免影响用户项目；启动边界由合成 child
  完整覆盖，真实安装只做 probe 和 doctor。
