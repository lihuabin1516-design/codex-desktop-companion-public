# Codex + PaneForge Companion 最高权威工程规则

本仓库是独立产品边界，目标是连接 Codex App 与用户安装的官方 PaneForge。
本文件与 `docs/AUTHORITY.md` 共同构成仓库最高权威；冲突时先以本文件为准，
再以路径更具体的 `AGENTS.md` 为准。

## 接手顺序

任何新的 Codex 维护任务先执行：

1. 阅读本文件；
2. 阅读 `docs/START_HERE.md`、`docs/AUTHORITY.md` 和相关架构/发布文档；
3. 记录仓库根、分支、HEAD 和完整 `git status`；
4. 明确本轮验收断言后再修改；
5. 构建统一使用仓库外单一 `CARGO_TARGET_DIR`。

## 不变量

- 官方 `paneforge.exe` 始终是外部依赖；不得复制、补丁或替换其安装文件。
- Codex App、Codex CLI 和 PaneForge 必须继续支持各自独立启动与官方升级。
- Companion 只清理任务级 Codex Desktop 私有环境变量，保留
  `CODEX_HOME`、`CODEX_API_KEY`、`OPENAI_API_KEY` 和用户 PATH。
- 默认不修改 `~/.codex`、`~/.paneforge`、PaneForge 数据库或真实项目。
- 诊断产物不得包含环境变量值、Token、项目文件内容或明文敏感路径。
- 安装、升级与卸载只处理 Companion 自己登记的文件；Codex MCP 配置只在显式
  `ConfigPath` 授权、写前备份和事务记录下处理托管 block。
- 目标运行链路只包含 `CodexPaneForge.exe`、`codex-paneforge-coordinator.exe`、
  `codex-paneforge-mcp.exe`、官方 PaneForge HTTP MCP 能力和 Codex CLI Worker；
  旧 LiveAgent Workflow Fabric、Tauri GUI、Gateway、WebUI、workflowd 和持续 runner
  均不属于本仓库运行链路。
- Coordinator/MCP 运行时只在对应阶段完成后描述为可用；Store/Core/Protocol
  合同落地不等于后台 Coordinator、MCP 进程或真实派发可用。
- Coordinator 生命周期入口默认只允许显式 describe、verify、前台 health 和 start-plan；
  安装流程默认不注册 service、scheduled task、自启动项或后台进程。
- `0.2.0-beta.1` 包装合同必须区分三个产品组件 EXE 与既有辅助 CLI；包内
  Manifest、SHA-256 和安装状态不得宣称已注册用户 MCP、后台 Coordinator 或真实派发。
- MCP 配置事务脚本在真实用户配置上运行前必须有显式路径授权、写前备份、事务记录和
  rollback 证据；合成测试只使用临时 `ConfigPath`。
- 仓库不得提交 `target/`、`dist/`、真实诊断报告、Token 或用户项目数据。

## 构建与测试

- 本仓库本身就是独立 Cargo workspace。
- 构建必须显式设置单一 `CARGO_TARGET_DIR`，不得在多个 worktree 产生重复
  Rust `target`。
- 单元与集成测试使用临时目录和合成可执行文件；真实 PaneForge 只做只读
  probe，不中断已有 Session。
- 发布门禁见 `docs/RELEASE.md`。

## Git 与发布

- 功能修改使用短生命周期分支并通过 PR 合入 `main`。
- 提交前执行格式、编译、测试、PowerShell 解析和 `git diff --check`。
- 测试 ZIP 通过 GitHub Pre-release 分发；源码仓库不跟踪构建产物。
- 公开生产发布要求 Authenticode 门禁通过。
- 版本号、`Cargo.lock`、发布说明和 Git tag 必须一致。
