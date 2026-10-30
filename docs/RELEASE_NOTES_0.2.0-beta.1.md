# Codex + PaneForge Companion 0.2.0-beta.1 release notes contract

Contract markers:

- contract: beta-release-notes/v1
- product: CodexPaneForgeCompanion
- targetVersion: 0.2.0-beta.1
- defaultRegisterUserMcp: false
- defaultStartBackgroundCoordinator: false
- realDispatchEnabled: false
- localUnsignedAllowed: true
- productionSignatureRequired: true

状态：Draft / Phase 8 package contract。该文件定义 `0.2.0-beta.1` 测试版发布说明必须包含的
用户可见事实；在版本号、签名、外部 Windows 机器验收和发布门禁全部完成前，不代表已经发布。

## 本测试版目标

`0.2.0-beta.1` 的目标是把 Companion 从 Launcher/诊断工具扩展为三组件 Coordination Plane
测试版交付形态，同时保持 Codex App、Codex CLI 和官方 PaneForge 的独立安装、启动、升级和
卸载能力。

包内组件：

- `CodexPaneForge.exe`：继续负责官方 PaneForge 发现、安全启动和 Codex Desktop 私有环境隔离；
- `codex-paneforge-coordinator.exe`：Coordinator 组件二进制，拥有 Workflow/Worker/Outbox/
  Binding/Lease/Result 的目标权威合同；
- `codex-paneforge-mcp.exe`：Codex App 可启动的 stdio MCP 边界二进制；
- `codex-paneforge.exe`：既有辅助 CLI，用于 describe、probe、doctor、audit-builds 和
  migrate-config。

## 当前 Phase 8 package 行为

- Release 包包含三组件 EXE、辅助 CLI、安装/卸载脚本、`ARTIFACT_MANIFEST.json`、
  `DEPENDENCIES.json`、`SHA256SUMS`、`Test-ExternalWindowsAcceptance.ps1`、第三方声明、
  运维文档和本 release notes contract。
- `ARTIFACT_MANIFEST.json` 声明 package inventory、side-effect boundary、Codex MCP
  configuration transaction contract、Coordinator lifecycle entry contract、外部 Windows
  机器验收合同和 beta release notes 合同。
- `Configure-CodexMcp.ps1` 只在显式 `ConfigPath` 上 apply/verify/repair/rollback/remove；
  安装器默认不调用它。显式使用时会先备份原文件，并用 transaction JSON 支持 rollback。
- `Manage-Coordinator.ps1` 支持显式 `Describe`、`Verify`、前台一次性 `Health` 和
  `StartPlan`；默认安装不启动后台 Coordinator，`StartPlan` 只返回命令计划。
- `Test-ExternalWindowsAcceptance.ps1` 在临时目录内自动执行 package inventory、checksum、
  隔离安装、Coordinator lifecycle、synthetic MCP config transaction 和卸载验收。
- 安装状态只登记 Companion 自己的文件和组件路径；卸载只移除 Companion 登记的文件。

## 默认未启用能力

本 release notes contract 必须明确说明以下能力在默认安装中未启用：

- 不注册真实用户 Codex MCP；
- 不启动后台 Coordinator；
- 不注册 Windows service、scheduled task 或自启动项；
- 不连接或控制真实 PaneForge Session；
- 不执行真实 Worker 派发、真实 Session submit/kill 或真实 PaneForge process restart；
- 不读取或修改 PaneForge 数据库、WebView storage、布局文件、真实项目或 `~/.paneforge`；
- 不修改真实 `~/.codex/config.toml`，除非用户在后续门禁中显式提供 config path 并接受该动作。

## 验证要求

发布说明必须引用并随包交付 `EXTERNAL_WINDOWS_ACCEPTANCE.md`。发布记录至少列出：

- package version、zip SHA-256 和 artifact JSON；
- 四个 EXE 的 Authenticode 状态；
- `scripts\verify-package.ps1` 结果；
- package 自带 `Test-ExternalWindowsAcceptance.ps1 -Json` 结果；
- 隔离 install/reinstall/uninstall/tamper rejection 结果；
- synthetic Codex MCP config transaction 结果；
- Coordinator lifecycle Describe/Verify/Health/StartPlan 结果；
- 外部 Windows 机器验收的 pass/fail/not-run 表；
- 真实 probe/doctor 的 pass/warn/not-run 状态；
- 敏感信息扫描结果。

## 已知限制

- 本地测试包可以 unsigned；公开生产发布必须通过 Authenticode `-RequireSignature` 门禁。
- synthetic/fake canary 证明调度、状态和协议合同，不替代真实 PaneForge Worker 池验收。
- 真实 Codex MCP 默认注册、后台 Coordinator bootstrap、通用 Coordinator→Adapter 派发启用
  和真实多 Worker E2E 仍按后续门禁推进。
- 如果外部机器缺少官方 Codex 或 PaneForge，真实 probe/doctor 可以记录为 `not-run`；package
  inventory、安装、MCP config transaction、Coordinator lifecycle 和卸载仍必须可验收。

## 回退

Companion 与官方 Codex/PaneForge 保持独立。Beta package 失效时，用户可直接启动官方
PaneForge 和 Codex；重新运行上一版 Companion 安装包可切回。卸载不得移除官方安装、真实
项目、真实 Codex config 或 PaneForge 数据。
