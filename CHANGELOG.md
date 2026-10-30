# Changelog

本项目遵循语义化版本，并将测试版作为 GitHub Pre-release 发布。

## 0.1.0-beta.3 - 2026-08-05

### Changed

- 迁移为独立 GitHub 仓库和独立 Cargo workspace；
- CI、缓存和发布路径从 LiveAgent 子目录调整到仓库根；
- 增加最高权威 `AGENTS.md`、维护入口、接手交接和安全策略；
- 增加 CODEOWNERS、Dependabot、MIT License 和贡献指南；
- 保持 beta.2 的启动、诊断、安装和兼容行为。

## 0.1.0-beta.2 - 2026-08-05

### Added

- 独立 `codex-paneforge.exe` CLI 和无控制台 `CodexPaneForge.exe` GUI launcher；
- 官方 PaneForge 多来源自动发现；
- Codex Desktop 任务级私有环境变量清理；
- `describe`、`probe`、`doctor`、`audit-builds` 和 `migrate-config`；
- JSONL 日志轮转；
- 用户级版本化安装、开始菜单入口和精确卸载；
- SHA-256 全包覆盖、篡改拒绝、依赖清单和第三方声明；
- Windows GitHub Actions 发布门禁。

### Verified

- Codex CLI `0.146.0`；
- PaneForge 从 `0.11.10` 自动升级到 `0.11.11` 后透明兼容；
- Console 与 GUI 两条子进程环境隔离链；
- 隔离安装、配置迁移、同版本重装与卸载；
- 真实安装升级保留上一版本目录。
