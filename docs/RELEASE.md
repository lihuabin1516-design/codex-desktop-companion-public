# 发布门禁

所有 Cargo 命令必须显式使用仓库外统一构建目录：

```powershell
$env:CARGO_TARGET_DIR = Join-Path $env:TEMP "codex-desktop-companion-target"
$env:RELEASE_OUTPUT_ROOT = Join-Path $env:TEMP "codex-desktop-companion-release"
```

## 必要检查

| 检查 | 命令 | 要求 |
|---|---|---|
| 格式 | `cargo fmt --all -- --check` | pass |
| 编译 | `cargo check --workspace --all-targets --locked` | pass |
| 静态检查 | `cargo clippy --workspace --all-targets --locked -- -D warnings` | pass |
| 单元/集成测试 | `cargo test --workspace --all-targets --locked` | pass |
| Core 依赖边界 | `cargo tree -p coordination-core --locked` + 禁止依赖审查 | pass |
| Phase 1 状态机 | `cargo test -p coordination-core state::tests --locked` | pass |
| Phase 1 协议合同 | `cargo test -p coordination-protocol --locked` | pass |
| Store 依赖边界 | `cargo tree -p coordination-store --locked` + 禁止反向依赖审查 | pass |
| Phase 2 Store/Outbox | `cargo test -p coordination-store --locked` | pass |
| Coordinator 依赖边界 | `cargo tree -p coordinator --locked -e normal` + 允许 Coordinator→Adapter、禁止 Coordinator→MCP/Launcher 与 Adapter 反向依赖审查 | pass |
| Phase 3 Coordinator Runtime | `cargo test -p coordinator --locked` | pass |
| Adapter 依赖边界 | `cargo tree -p ccpanes-adapter --locked -e normal` + 禁止依赖 Store/MCP/Launcher/Windows 审查 | pass |
| Phase 4 CC-Panes Adapter | `cargo test -p ccpanes-adapter --locked` | pass |
| CC-Panes v0.12.3 capability fixture | `cargo test -p ccpanes-adapter --test adapter_contract v0123_capability_fixture_negotiates_session_primitives --locked` | `7fad08d` 九工具 schema snapshot 与 capability 分组 pass |
| Adapter v0.12.3 capability/DTO/parser | `cargo test -p ccpanes-adapter --locked` | READY 或 typed incompatible、launch/output/kill/wait/write 与无副作用合同 pass |
| CC-Panes v0.12.3 真实 preflight + foreground canary | `read_only_preflight` + `coordinator` 的 `real_ccpanes_canary` example | READY；Coordinator→Adapter MCP 新 Codex 对话观察 Worker 计算的 Unicode 逆序 challenge response + `idle`，expected response 不进入 prompt，Store 收敛成功，submit/kill/write=0；cwd 路由恢复还要求 source workspace 与 launch workspace 分离、专用 Workspace `path=projectPath`、`cwdMatched=true` |
| CC-Panes v0.12.3 新对话用户级验收 | 官方 CC-Panes 新建 Codex 对话 → 全局 skill/MCP discovery → `ccpanes_preflight` → `ccpanes_synthetic_canary` | skill/MCP discovered、READY、`externalWorkerSideEffects=false`、launch=1、submit/kill/write=0、Session idle、Store success convergence、`launchWorkspaceName` 为 projectPath 专用 workspace、`cwdMatched=true` |
| MCP 依赖边界 | `cargo tree -p coordination-mcp --locked -e normal` + 禁止依赖 Store/Adapter/Launcher/Windows normal path 审查 | pass |
| Phase 5 MCP + synthetic E2E | `cargo test -p coordination-mcp --locked` | pass |
| Phase 6 fan-out/fan-in | `cargo test -p coordinator phase6 --locked` + `cargo test -p coordination-mcp workflow_create_start_fans_out_multiple_steps_through_mcp --locked` | pass |
| Phase 7 synthetic repository | `cargo test -p coordinator --locked phase7` | pass |
| Phase 7 dedicated Workspace/Project gate | `cargo test -p ccpanes-adapter --locked workspace` | pass |
| Phase 7 single Worker success canary | `cargo test -p coordination-mcp --test mcp_stdio phase7_single_worker_success_canary_uses_dedicated_repo_workspace_and_records_result --locked` | pass |
| Phase 7 cancellation canary | `cargo test -p coordination-mcp --test mcp_stdio phase7_cancellation_canary_uses_dedicated_repo_workspace_and_kills_session_once --locked` | pass |
| Phase 7 Coordinator restart canary | `cargo test -p coordination-mcp --test mcp_stdio phase7_coordinator_restart_canary_reopens_store_and_finishes_fake_worker_once --locked` | pass |
| Phase 7 unknown dispatch canary | `cargo test -p coordination-mcp --test mcp_stdio phase7_unknown_dispatch_canary_reconciles_dropped_launch_response_once --locked` | pass |
| Phase 7 Token/discovery rotation canary | `cargo test -p ccpanes-adapter --test adapter_contract phase7_token_discovery_rotation_canary_refreshes_preflight_without_secret_leak --locked` | pass |
| Phase 7 CC-Panes process restart canary | `cargo test -p coordination-mcp --test mcp_stdio phase7_ccpanes_process_restart_canary_recovers_fake_endpoint_without_duplicate_launch --locked` | pass |
| Phase 7 multi-worker capacity canary | `cargo test -p coordination-mcp --test mcp_stdio phase7_multi_worker_capacity_canary_batches_fake_launches_without_overdispatch --locked` | pass |
| Phase 7 只读 dispatch 再资格 preflight | `cargo run --quiet -p ccpanes-adapter --example read_only_preflight -- --discovery <discovery>` | `READY`、`status.capability.dispatchReady=true`、Session inventory 存在、`externalWorkerSideEffects=false`；否则 typed error + 非零退出 |
| Release 构建 | `scripts\build-release.ps1` | pass |
| Phase 8 包结构 | `scripts\verify-package.ps1` | 三组件 EXE + 辅助 CLI、`ARTIFACT_MANIFEST.json`、SHA-256、依赖清单 pass |
| Phase 8 合成 MCP 配置事务 | `scripts\test-installer.ps1` | 临时 Codex config apply/verify/repair/rollback 与显式 installer/uninstaller 串联 pass |
| Phase 8 Coordinator 生命周期入口 | `scripts\test-installer.ps1` | `Manage-Coordinator.ps1` describe/verify/foreground health/start-plan pass |
| Phase 8 外部 Windows 验收合同 | `scripts\verify-package.ps1` + `scripts\test-installer.ps1` | `EXTERNAL_WINDOWS_ACCEPTANCE.md` 随 package/docs/installed version root 交付，Manifest marker pass |
| Phase 8 package 自带外部验收 runner | 包内 `Test-ExternalWindowsAcceptance.ps1 -Json` | 临时 install root + synthetic Codex config 验收 package/checksum/lifecycle/MCP transaction/uninstall pass |
| Phase 8 Beta release notes 合同 | `scripts\verify-package.ps1` + `scripts\test-installer.ps1` | `RELEASE_NOTES_0.2.0-beta.1.md` 随 package/docs/installed version root 交付，未启用能力和签名边界 marker pass |
| Phase 8 隔离安装/升级/卸载 | `scripts\test-installer.ps1` | 临时根 install/reinstall/uninstall/tamper rejection pass |
| 依赖清单 | 包内 `DEPENDENCIES.json` | pass |
| Authenticode | `verify-package.ps1 -RequireSignature` | 公开生产发布必须 pass |
| 真实 CC-Panes probe | `codex-ccpanes probe --json` | pass |
| 真实环境 doctor | `codex-ccpanes doctor --json` | pass 或已解释 warn |
| Git whitespace | `git diff --check` | pass |

## Coordination Plane 阶段门禁

`0.2.0-beta.1` 三组件测试版在既有 Launcher 门禁外，还需要逐阶段通过：

1. Core/Protocol：状态转换、duplicate request、stale revision、unknown schema version、
   公共错误码与 schema snapshot。
2. Store/Outbox：SQLite migration、backup、integrity、optimistic revision、transactional
   outbox、idempotency、Binding/Lease/Result repository 和重启恢复。
3. Coordinator：单实例、IPC health、scheduler、capacity、budget、cancel、reconcile。
4. Adapter：公开 CC-Panes HTTP MCP capability negotiation、fake server、unknown dispatch、
   Token/discovery rotation、synthetic process restart，以及官方 v0.12.3 (`7fad08d`)
   capability fixture/schema snapshot。基础 READY、dispatch capability 与 optional Session
   control 必须分层门控，不能依赖全局固定工具表。
5. MCP：stdio initialize/tools/list/tools/call、受控启动/连接、结构化结果、request replay、
   worker_cancel、MCP restart 查询同一 Workflow、fake CC-Panes 单 Worker complete process。
6. fan-out/fan-in：DAG dependency unlock、`all-required`、`allow-partial`、`first-success`、
   host capacity、两 Workflow 公平派发、Worker/Workflow timeout、24/48 synthetic pressure、
   单 worktree Writer Lease。
7. 只读真实 CC-Panes dispatch 再资格 preflight：initialize、tools/list、list_sessions、
   list_panes、能力 digest、脱敏 Session/Panes inventory，且
   `require_dispatch_requalification` 要求基础 READY、`dispatchReady=true`、inventory
   存在、`externalWorkerSideEffects=false`；报告敏感信息扫描为零命中。
8. 专用合成 Git 仓库：显式空目录创建、marker digest、baseline HEAD、clean status digest、
   dirty/head drift 拒绝、parent containment recovery guard。
9. 专用 CC-Panes Workspace/Project：只读 list_workspaces/list_projects gate、必需工具检查、
   `workspaceName` launch capability、missing setup plan、同名 Workspace path mismatch
   fail closed 和无 create/add side effect 断言；受控 setup 门再验证 create/add 参数。
10. fake 单 Worker success canary：专用合成仓库、existing Workspace/Project fixture、
   `workspaceName` launch payload、Binding session ref、Worker/Outbox/Result 收敛与
   `workflow_result` complete，且无 create/add/submit/kill side effect。
11. fake cancellation canary：专用合成仓库、existing Workspace/Project fixture、
   active Binding cancel、唯一 `kill_session` 控制 outbox、fake kill/reconcile、取消来源在
   Worker/Workflow/Result 中可查询，且无 create/add/submit side effect。
12. fake Coordinator restart canary：同一合成 SQLite DB 重新打开 Runtime、MCP 查询 active
   Worker/Binding、tick 不重复 launch、fake reconcile 后继续记录 Result 并聚合 complete。
13. fake unknown dispatch canary：fake `launch_task` side effect 后丢响应，Store 恢复为
   `RESULT_UNKNOWN`/`DISPATCH_UNKNOWN`，list_sessions/list_panes 绑定原 fake Session，
   `workflow_result` complete，且 launch 不重复、无 create/add/submit/kill side effect。
14. fake Token/discovery rotation canary：fake endpoint/token A→B，旧 token 对新 endpoint
   返回 `authorization_failed`，新 discovery generation/token 刷新 readiness 与 capability
   digest，且 report/request record 不暴露 Token 或 Authorization 明文、无 create/add/launch/
   submit/kill side effect。
15. fake CC-Panes process restart canary：fake endpoint A 停止后旧 adapter preflight/reconcile
   返回 `adapter_not_ready`/DEGRADED，MCP 仍可查询 RUNNING Worker 与 ACTIVE Binding，
   dispatch freeze 与新 endpoint READY 后 tick 均不重复 launch，fake endpoint B 携带同一
   synthetic Session 后 reconcile 和 `workflow_result` complete，且无 create/add/submit/kill
   side effect。
16. fake multi-worker controlled capacity canary：MCP `workflow_create/start` 创建 6 个只读
   Step，Coordinator 在 host/dispatch/per-workflow cap=3 下分两批 fake `launch_task`，
   active/queued 计数受控，Binding/Session 与 `clientRequestId` 唯一，`workflow_result`
   complete 保留全部 `stepId` 来源，且无 create/add/submit/kill side effect。
17. E2E/故障：Coordinator→Adapter 一次性前台单 Worker success seam 已签署；真实重启、
   timeout、重复派发、取消和 unknown dispatch reconcile 分别保持独立门禁。
18. 受控真实 CC-Panes canary：只在专用合成仓库和专用 Workspace 门打开后执行；当前证据
   为新 Codex 对话 Unicode 逆序 challenge-response + `idle`、Store success convergence
   和零 submit/kill/write；expected response 不进入 prompt。v0.12.3 cwd 路由恢复后还必须
   验证 `create_workspace(name,path=projectPath)`、`add_project_to_workspace`、实际
   `launch_task.workspaceName=launchWorkspaceName`、prompt 不进行目录切换，以及 `cwdMatched=true`。
19. Phase 8 package inventory and manifest：`CodexCCPanes.exe`、
    `codex-ccpanes-coordinator.exe`、`codex-ccpanes-mcp.exe` 三个产品组件 EXE 与既有
    `codex-ccpanes.exe` 辅助 CLI、`ARTIFACT_MANIFEST.json`、SHA-256 全覆盖、依赖清单、
    隔离安装状态登记、签名状态与敏感信息扫描。
20. Phase 8 synthetic Codex MCP config transaction：显式临时 `ConfigPath`、写前 backup、
    托管 MCP block、`verify`、幂等 `repair`、transaction `rollback` 恢复 present/absent
    原状态；安装器显式 apply/repair、卸载器显式 remove，且不触碰真实 `~/.codex`。
21. Phase 8 Coordinator lifecycle entry：`Manage-Coordinator.ps1` 进入 package 和 install root，
    支持 describe、verify、前台一次性 health、start-plan，默认安装不启动后台 Coordinator。
22. Phase 8 external acceptance + beta notes package contract：包内外部 Windows 验收清单、
    `Test-ExternalWindowsAcceptance.ps1` runner、beta release notes、Manifest marker 与安装版本根交付。

## 真机限制

- 不停止当前 CC-Panes；
- 仅允许在已登记的专用合成项目中新建一个明确标题的 canary Session；不启动或恢复其他
  真实项目 Session；
- 不修改 `~/.cc-panes`；package/default install 不修改 `~/.codex`。本次开发验收只在显式
  `ConfigPath` 授权下通过事务脚本备份、写入并 verify 一个托管 MCP block；
- 真实验证默认只执行 probe、doctor、进程观察和只读 CC-Panes MCP dispatch 再资格 preflight；
  本轮额外批准一次 Coordinator→Adapter MCP 前台单 Worker launch/status/output canary；
- 只读 preflight 仅调用 initialize、notifications/initialized、tools/list、list_sessions 和
  list_panes，报告隐藏 endpoint secret、Authorization 值、Token、Session ID 明文、Prompt 和输出；
- 专用 Workspace/Project gate 仅可只读调用 list_workspaces/list_projects 并返回 setup plan
  或 path mismatch incompatible；create_workspace/add_project_to_workspace 只在受控 canary
  setup 门内执行，且 Workspace `path` 必须等于目标 `projectPath`；
- 启动链路使用合成 child fixture 验证环境清理；
- v0.12.3 真实 read-only dispatch 再资格 preflight、常规 gate evaluator、一次
  Coordinator→Adapter MCP 前台 canary，以及官方 CC-Panes 新对话的
  skill/MCP discovery → preflight → synthetic canary 用户级入口验收已完成；入口验收确认
  Store 收敛、`launchWorkspaceName` 为 projectPath 专用 Workspace、`cwdMatched=true`、
  `acceptancePassed=true`。该证据不等价于后台 Coordinator、真实 Session submit/kill/write、
  restart、unknown-outcome 或多 Worker E2E。

## Artifact

`0.1.x` Launcher 测试版发布目录包含：

- `codex-ccpanes.exe`
- `CodexCCPanes.exe`
- 一键安装与卸载入口
- `Configure-CodexMcp.ps1`
- `Manage-Coordinator.ps1`
- SHA-256 清单
- `ARTIFACT_MANIFEST.json`
- `EXTERNAL_WINDOWS_ACCEPTANCE.md`
- `Test-ExternalWindowsAcceptance.ps1`
- `RELEASE_NOTES_0.2.0-beta.1.md`
- README、运维和兼容说明
- `DEPENDENCIES.json` 与第三方声明

`0.2.0-beta.1` Coordination Plane 测试版发布目录还需包含：

- `CodexCCPanes.exe`
- `codex-ccpanes-coordinator.exe`
- `codex-ccpanes-mcp.exe`
- 既有辅助 CLI `codex-ccpanes.exe`
- Coordinator 生命周期入口、repair、rollback、uninstall
- 三组件 inventory、schema/contract 摘要和能力再资格记录
- 外部 Windows 机器验收清单与 beta release notes 合同

本地测试版可以 unsigned，但 Artifact 和验收报告必须明确记录。公开生产发布必须提供签名
证书，并使用：

```powershell
.\scripts\build-release.ps1 `
  -CargoTargetDir $env:CARGO_TARGET_DIR `
  -OutputRoot $env:RELEASE_OUTPUT_ROOT `
  -CertificateThumbprint $env:CODEX_CCPANES_SIGNING_CERT_THUMBPRINT `
  -RequireSignature
```
