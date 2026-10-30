# Codex + CC-Panes Companion 权威定义

版本：`2.0`
状态：三组件 Authority v2 / Phase 8 external acceptance + CC-Panes v0.12.3 cwd 路由恢复
日期：2026-08-11

## 产品定位

Companion 是 Codex App 与官方 CC-Panes 之间的本机 Coordination Plane 与安全启动层。
它保留既有 Launcher 运维能力，并以独立组件恢复 Codex App 主控到 CC-Panes 外部
Worker 的高层派发、观察、取消和结果回收合同。

当前已发布运行能力仍是 `CodexCCPanes.exe` 和 `codex-ccpanes.exe` Launcher/诊断链路；
Phase 7 增加 `coordination-core`、`coordination-protocol`、`coordination-store`、
`coordinator`、`ccpanes-adapter` 与 `coordination-mcp` 可验证合同。MCP stdio 进程、
Coordinator IPC seam、单 Worker 派发/状态/取消/结果、fake CC-Panes 完成链路、多 Step
Workflow fan-out/fan-in、容量、超时、Writer Lease、只读 CC-Panes capability preflight
专用合成 Git 仓库登记/恢复 guard、专用 CC-Panes Workspace/Project 只读 gate 和 fake
Adapter 单 Worker success canary、cancellation canary、Coordinator restart canary、dropped
launch response / unknown dispatch canary、Token/discovery rotation canary、synthetic
CC-Panes process restart canary 与 synthetic multi-worker controlled capacity canary 已用
测试或本机只读 probe 覆盖。Phase 8 首个本地包装切片要求 release 包 inventory 含三个
产品组件 EXE 与既有辅助 CLI，并用 `ARTIFACT_MANIFEST.json`、`SHA256SUMS`、
`DEPENDENCIES.json` 和隔离安装状态登记验证；当前切片新增 `Configure-CodexMcp.ps1`
合成配置事务合同，要求显式 `ConfigPath`、写前备份、幂等 repair 和事务 rollback 恢复
present/absent 原状态，并把事务接入显式安装器 apply/repair 与卸载器 remove；当前切片还新增
`Manage-Coordinator.ps1` 生命周期入口合同，只允许显式 describe/verify/foreground health/
start-plan，不把后台 Coordinator 注册为默认安装行为；当前切片还新增
`EXTERNAL_WINDOWS_ACCEPTANCE.md`、`Test-ExternalWindowsAcceptance.ps1` 和
`RELEASE_NOTES_0.2.0-beta.1.md` 包内合同，并由 Manifest/verify/install/self-test 验证其存在、
marker 与副作用边界。Coordinator→Adapter 的一次性前台单 Worker execution seam 已在
专用合成项目上通过真实 CC-Panes `0.12.3` 新 Codex 对话 canary；本机开发验收已在
显式 `ConfigPath` 授权下事务注册托管 MCP block 并完成新对话入口验收。package/default
用户 MCP 注册、后台守护、多 Worker 派发、真实 Session 取消/restart/unknown-outcome
reconcile、外部 Windows 机器实际验收和真实发布属于后续门禁。

目标生产链路：

```text
Codex App
→ codex-ccpanes-mcp.exe
→ codex-ccpanes-coordinator.exe
→ CC-Panes HTTP MCP Adapter
→ 官方 CC-Panes Session
→ Codex CLI Worker
→ 状态/输出对账
→ 结果聚合
→ Codex App
```

## 组件权威边界

| 组件 | 拥有 | 不拥有 |
|---|---|---|
| `CodexCCPanes.exe` | 官方 CC-Panes 发现、安全启动、Codex Desktop 任务级私有环境清理、启动收据 | Workflow/Worker 权威状态、CC-Panes Session、结果聚合 |
| `codex-ccpanes-coordinator.exe` | Workflow、Worker、ExternalBinding、Lease、Event、Outbox、Budget、Result 的唯一本机生产状态源；并发、超时、重试、取消、恢复与聚合 | Codex App 对话、CC-Panes PTY/Session 本体、官方安装与更新 |
| `codex-ccpanes-mcp.exe` | MCP initialize、tools/list、tools/call、高层输入验证、公共错误映射、到 Coordinator 的本机 IPC；Phase 6 支持 synthetic multi-step Workflow create/start/result | Workflow 持久状态、重试、Session 对账、调度决定 |
| `coordination-core` | ID、revision、Workflow/Worker/Outbox 状态机、WorkflowSpec v1beta1、RepositoryRef 可选 `workspaceName`、DAG Step、fan-in policy、命令/查询/事件、公共错误码 | Store、MCP transport、Adapter、Windows、Launcher |
| `coordination-protocol` | MCP/IPC schema version、公共工具输入输出 DTO、RepositoryInput 可选 `workspaceName`、schema 合同、version negotiation、聚合结果来源/缺失/失败合同 | 运行时调度、持久化、真实外部副作用 |
| `coordination-store` | SQLite schema v1、migration/backup/integrity、Workflow/Worker/Event/Outbox/ExternalBinding/Lease/Result repository、external launch success→Binding ACTIVE 记录、optimistic revision、幂等记录、Writer Lease、retention 元数据 | Coordinator 调度策略、MCP transport、CC-Panes Adapter、Windows、Launcher、真实外部副作用 |
| `coordinator` | Runtime identity、单实例锁、IPC health handler、scheduler、capacity/budget admission、DAG fan-out、fan-in policy、Worker/Workflow timeout、dispatch/reconcile/control semaphore、pause/resume/cancel orchestration、result aggregation、metrics/log snapshot、repository baseline guard、专用合成 Git 仓库登记/恢复 guard，以及经 Adapter 执行的一次性前台单 Worker canary | CC-Panes HTTP 协议翻译、MCP stdio server、安装注册、后台守护、通用真实 Worker 池、用户仓库清理 |
| `ccpanes-adapter` | 显式 discovery、认证 Header 注入、Token/discovery rotation canary、synthetic process restart canary、HTTP MCP initialize/tools/list、capability digest、按工具 inventory/schema 门控的 `launch_task`、Session query/output/submit/kill/wait/write、unknown dispatch marker reconcile、只读 capability preflight、Session inventory 脱敏摘要、专用 Workspace/Project 只读 gate、deterministic fake server | Workflow 权威状态、重试策略、调度决定、CC-Panes Session 本体、CC-Panes 数据库、MCP stdio、Launcher、Workspace/Project 创建决定 |

## 上游与外部产品权威

1. Codex App 与 Codex CLI 的行为以 OpenAI 官方发行版为准。
2. CC-Panes 的功能、数据模型、数据库、PTY、Pane、Session 和更新器以
   `wuxiran/cc-pane` 官方发行版为准。
3. Companion 不复制上述产品源码，不固定其补丁版本，不介入其自动更新。
4. Coordinator 只通过公开 CC-Panes HTTP MCP 能力创建、查询、提交和停止外部 Session；
   不读取或修改 CC-Panes 数据库、WebView storage 或布局文件。
5. Codex App thread 与外部 Worker 通过 Workflow/Worker ID 关联；外部 Worker 不伪装成
   Codex App 内置 Subagent 或同一对话。

## 当前 CC-Panes 兼容合同

- 当前已知 capability/schema 合成兼容基线为官方 CC-Panes `v0.12.3`，tag commit
  `7fad08d8a210015eb12af283c720394e7324f471`；本机官方 `cc-panes.exe` 的
  FileVersion/ProductVersion 已只读确认均为 `0.12.3`。
- Adapter 的基础 READY 合同要求 MCP protocol `2025-03-26`，并要求
  `launch_task`、`list_sessions`、`list_panes` 存在且输入 schema 满足已使用字段。
- Worker 派发能力另要求 `get_session_status` 与 `get_session_output`；缺失时基础 capability
  可保持 READY，但 `dispatchReady=false`，`launch_task` 在外部请求前返回
  `adapter_incompatible`。
- 官方更新后的常规 dispatch 再资格 runner 必须要求基础 READY、`dispatchReady=true`、
  Session inventory 存在和 `externalWorkerSideEffects=false`；失败以 typed error 非零退出，
  不进入 `launch_task`。
- `submit_to_session`、`kill_session`、`wait_for_session`、`write_to_session` 按逐工具
  inventory/schema 门控。工具缺失时对应操作返回 typed incompatible；工具存在但 schema
  不满足合同时，capability negotiation 失败。
- `get_session_output` 兼容 v0.12.3 `content`/`lines` 与历史 `output`；`launch_task` 解析
  `taskId`、`sessionId`、`runtimeKind`；`kill_session` 仅在上游 schema 声明时发送 `reason`，
  因此兼容 v0.12.3 仅含 `sessionId` 的输入合同。
- v0.12.3 的终端 cwd 由 `workspaceName` 命中的 Workspace `path` 优先决定。Companion
  将调用来源的 source `workspaceName` 作为语义/审计元数据保留；Coordinator 始终按
  `projectPath` 生成确定性的
  `launchWorkspaceName=codex-ccpanes-phase7-<sha256-prefix>` 并传给 `launch_task`，不复用
  与该 projectPath 摘要不一致的同前缀 source workspace。
- 受控 canary/launch 前，Adapter 可在明确 setup 门内调用
  `create_workspace(name,path=projectPath)` 与
  `add_project_to_workspace(workspaceName,projectPath)`；只读 gate 仍只调用
  list_workspaces/list_projects。已有同名专用 Workspace 但 `path` 与 `projectPath`
  不一致时 fail closed，不用 prompt 或手动目录切换掩盖 cwd 问题。
- 专用合成 Git 仓库只为该 canary 预声明官方 CC-Panes 运行元数据 ignore 边界：
  `.ccpanes/history/`、`.ccpanes/projects.csv` 与 `CLAUDE.md`。其它 dirty path 继续
  fail closed；该边界不外推到真实用户项目。
- Canary 报告新增 `sourceWorkspaceName`、`launchWorkspaceName`、期望 projectPath digest、
  observed cwd digest 与 `cwdMatched`；诊断报告不输出完整项目路径。
- `docs/89-mcp-tool-surface-reduction.md` 是上游工具面收编方向，不作为固定全局工具表。
  Companion 只门控当前操作实际依赖的能力，避免未使用工具被删除时击穿基础 READY。
- 本轮证据包括上游只读源码/tag 对比、v0.12.3 fixture/schema snapshot、fake HTTP MCP、
  本机真实 read-only preflight，以及一个专用合成项目中新建 Codex 对话的
  `Coordinator Store intent → Adapter HTTP MCP launch_task → get_session_status/
  get_session_output → Store convergence` 前台 canary。该 canary 强制 `cliTool=codex`，
  解析 `taskId`、`sessionId`、`runtimeKind`；Worker 仅计算随机 challenge 的 Unicode
  逆序响应，expected response 不进入 prompt。Runner 观察到该响应和 `idle`，并使
  Binding/Outbox/Worker/Workflow/Result 收敛到
  `ACTIVE/ACKNOWLEDGED/SUCCEEDED/SUCCEEDED/complete`；未调用 submit/kill/write。
- 2026-08-11 用户级入口验收从官方 CC-Panes `0.12.3` 新建独立 Codex 对话；该对话确认
  `codex-ccpanes-companion` 与 `## Required order` 注入、发现 `codex_ccpanes` 工具，
  preflight 返回 READY，再调用一次 synthetic canary。最终
  `launch/submit/kill/write=1/0/0/0`、Session `idle`，Store 状态为
  `ACTIVE/ACKNOWLEDGED/SUCCEEDED/SUCCEEDED/complete`，`launchWorkspaceName` 为
  `codex-ccpanes-phase7-*` 专用 Workspace，`cwdMatched=true`，
  `acceptancePassed=true`。该轮只签署专用合成项目的一次性前台 success path，不外推到
  package/default MCP 注册、后台、多 Worker、真实控制、restart 或 unknown-outcome。

## 必须满足

- 从 Codex Desktop 环境启动时，CC-Panes 后代进程不得继承任务级 Codex 私有变量。
- 从开始菜单独立启动 CC-Panes 的原有方式保持可用。
- 官方 CC-Panes 升级后，只要安装路径和公开 MCP capability 仍满足合同，Companion 不按
  精确版本字符串阻断。
- 诊断不读取项目内容，不输出密钥或环境变量值。
- Companion 任一组件离线、损坏或卸载时，Codex App、Codex CLI 和官方 CC-Panes 仍可
  独立使用。
- 构建只编译本目录独立 Cargo workspace；使用仓库外独立 `CARGO_TARGET_DIR`，具体路径由维护者环境决定。
  作为 `CARGO_TARGET_DIR`。
- MCP 写命令必须携带 `requestId`，幂等范围为 `tool + caller + requestId`。
- 所有状态变更携带 revision；stale revision 返回结构化 `revision_conflict`。
- `DISPATCH_UNKNOWN`、`CANCELLING` 和终态必须在类型和测试中显式表达。

## 明确排除

- LiveAgent Desktop、Tauri GUI、Gateway、WebUI、workflowd 和持续 runner 不属于本仓库
  运行链路。
- 不替换官方 CC-Panes EXE，不修改 CC-Panes adapter、数据库、WebView storage 或真实
  项目 Session。
- `0.2.0-beta.1` 包装和默认安装不注册用户 MCP，不启动后台 Coordinator，不开放通用真实
  Worker 派发。本次开发验收仅在显式 `ConfigPath` 授权、写前备份和 transaction 记录下
  注册一个托管 MCP block，用于新对话验收；这不改变默认安装合同。唯一非只读真实验证是
  显式批准的专用合成项目前台 Codex canary；常规 preflight 仅允许
  initialize、notifications/initialized、
  tools/list、list_sessions 和 list_panes，报告中不得包含 Token、Authorization 值、
  Session ID 明文、Prompt、输出或未经脱敏的 endpoint secret。
- 专用合成 Git 仓库只能在显式空目录中创建；恢复/清理动作必须同时通过 parent containment
  和 marker digest guard。当前切片只建立登记/恢复合同，不删除目录、不写真实项目。
- 专用 CC-Panes Workspace/Project gate 当前只读调用 list_workspaces/list_projects，检查
  create_workspace、add_project_to_workspace 和 `launch_task.workspaceName` capability，并返回
  existing fixture、setup plan 或 path mismatch incompatible；实际 create/add side effect
  只在受控 canary/launch setup 门内执行，且 Workspace `path` 必须等于目标 `projectPath`。
- fake 单 Worker success canary 使用专用合成 Git 仓库、fake CC-Panes HTTP MCP 和已存在
  Workspace/Project fixture，验证 `workspaceName` 进入 `launch_task` payload、
  ExternalBinding 记录 session ref 且聚合结果完整。另有一次前台真实 canary 验证同一
  Coordinator→Adapter seam 可在官方 CC-Panes `0.12.3` 新建 Codex 对话并收敛成功；该证据
  不外推到后台、多 Worker、取消、restart 或 unknown-outcome 真实执行。
- cancellation canary 当前仅使用同类专用合成 Git 仓库、fake CC-Panes HTTP MCP 和已存在
  Workspace/Project fixture，验证 `workflow_cancel`/`worker_cancel` 经 Coordinator 进入
  `CANCEL_REQUESTED`，只产生一个 `kill_session` 控制 outbox，fake Session 被 kill 后
  Binding/Worker/Workflow/Result 收敛；它不代表真实 CC-Panes Session 控制已接线。
- Coordinator restart canary 当前仅重新打开同一合成 SQLite DB 中的 Coordinator Runtime，
  验证重启前已启动的 fake Worker 状态、Binding session ref、Outbox ACK 和 Result 聚合可继续；
  它不代表后台守护、进程 supervisor 或真实 CC-Panes restart 已接线。
- dropped launch response / unknown dispatch canary 当前仅使用 fake CC-Panes 验证 `launch_task`
  外部副作用已产生但响应丢失时，Store 在错误返回路径立即将 Outbox/Binding 记录为
  `RESULT_UNKNOWN`、Worker 记录为 `DISPATCH_UNKNOWN`，reopen 后保持该状态，随后通过
  list_sessions/list_panes reconcile 绑定原 fake Session 并聚合结果；它不代表真实
  CC-Panes unknown outcome 已验收。
- Token/discovery rotation canary 当前仅使用 fake CC-Panes endpoint A/B 验证旧 discovery/token
  成功、旧 token 对新 endpoint 返回 `authorization_failed`、新 discovery generation/token
  刷新 readiness 与 capability digest，且 sanitized report 与 fake request record 不包含 Token
  或 Authorization 明文；它不读取真实 discovery 文件。真实 CC-Panes 更新后再资格由独立的
  read-only dispatch gate 签署，不由该 fake rotation canary 外推。
- CC-Panes process restart canary 当前仅使用 fake endpoint A 停止与 fake endpoint B 重启，
  并以 seeded synthetic Session 验证旧 endpoint `adapter_not_ready`/DEGRADED、既有
  Workflow/Worker/Binding 可经 MCP 查询、tick 不重复 `launch_task`、新 endpoint READY 后
  reconcile 与 Result 聚合 complete；它不停止真实 CC-Panes，不代表真实进程 supervisor、
  真实 Session 恢复或真实重启验收已签署。
- multi-worker controlled capacity canary 当前仅使用专用合成 Git 仓库、fake CC-Panes HTTP
  MCP 和已存在 Workspace/Project fixture，验证 MCP 创建 6 个只读 Step、Coordinator 在
  host/dispatch/per-workflow cap=3 下分两批派发，active/queued 计数有界，Binding/Session
  与 `clientRequestId` 唯一，`stepId` 来源和聚合结果完整；它不代表真实多 Session 派发、
  真实 CC-Panes 容量或真实 Worker 池验收已签署。
- Phase 8 package inventory and manifest local contract 当前仅修改本地包装脚本与隔离包验证：
  包内必须包含 `CodexCCPanes.exe`、`codex-ccpanes-coordinator.exe`、
  `codex-ccpanes-mcp.exe` 和既有 `codex-ccpanes.exe`；`ARTIFACT_MANIFEST.json` 必须声明
  三组件 inventory、副作用边界、`SHA256SUMS` 全覆盖和 `DEPENDENCIES.json`；安装状态只登记
  Companion 自己的组件路径，不注册用户 MCP、不启动后台 Coordinator、不修改官方 Codex、
  官方 CC-Panes、用户配置或真实项目。
- Phase 8 synthetic Codex MCP config transaction contract 当前仅通过显式临时 `ConfigPath`
  验证 `Configure-CodexMcp.ps1`：写入托管 `codex_ccpanes` MCP block 前备份原文件，
  `Repair` 不重复 block，`Rollback` 按事务恢复原 SHA-256 或 absent 状态；安装器仅在显式
  `-ConfigureCodexMcp -CodexConfigPath <path>` 时调用 apply/repair，卸载器仅在显式
  `-RemoveCodexMcp -CodexConfigPath <path>` 时移除托管 block；默认安装仍不调用该脚本，
  不读取或修改真实 `~/.codex/config.toml`，不运行 `codex mcp add/remove`。
- Phase 8 coordinator lifecycle entry contract 当前仅通过 `Manage-Coordinator.ps1` 暴露显式
  `Describe`、`Verify`、前台一次性 `Health` 和 `StartPlan`；`Health` 只在调用者给定或默认
  install-root 子目录下打开合成 SQLite DB 并退出，`StartPlan` 只返回命令计划。默认安装不启动
  后台 Coordinator，不注册 Windows service、scheduled task 或自启动项。
- Phase 8 external acceptance + beta notes contract 当前仅打包并校验
  `EXTERNAL_WINDOWS_ACCEPTANCE.md`、`Test-ExternalWindowsAcceptance.ps1` 与
  `RELEASE_NOTES_0.2.0-beta.1.md`，用于说明另一台 Windows x64 机器的 package 验收步骤、
  记录格式、未启用能力和已知限制；runner 只使用临时 install root 与 synthetic Codex config。
  它不代表外部机器已经执行验收，也不代表 `0.2.0-beta.1` 已发布。
- Coordinator v1 不自动合并、不自动 push、不接管用户已有 Session。
- 安装、升级与卸载只处理 Companion 自己登记的文件。

## 公共合同与错误

公共 MCP 工具第一版固定为：

```text
coordinator_status
adapter_status
worker_dispatch
worker_status
worker_cancel
workflow_create
workflow_start
workflow_status
workflow_events
workflow_cancel
workflow_result
```

`worker_dispatch.repository.workspaceName` 是可选公共字段；仅用于绑定已通过 gate 的专用
CC-Panes Workspace，必须非空且不含控制字符。未提供时保留既有单仓库合成派发合同。

公共错误码由 `coordination-core` 拥有，至少包括：

```text
invalid_request
schema_version_unsupported
revision_conflict
invalid_transition
repository_baseline_changed
adapter_not_ready
adapter_incompatible
capacity_exhausted
budget_exhausted
dispatch_result_unknown
external_binding_conflict
worker_not_found
workflow_not_found
cancel_pending
result_pending
persistence_unavailable
authorization_failed
internal_invariant_violation
```

错误结构包含稳定 `code`、安全 `message`、`retryable`、`correlationId`，按需包含
`currentRevision`、`currentState` 和 `retryAfterMs`。错误、诊断、Event 和 Artifact
不得写入 Token、Authorization、Session ID 明文或未经脱敏的完整用户路径。

## 发布判定

`0.1.x` Launcher 测试版仍按既有 Launcher 发布门禁验收。

`0.2.0-beta.1` 三组件测试版还必须通过 Coordination Plane 门禁：Core/Protocol 合同测试、
Store/Outbox 恢复、Coordinator runtime 调度、MCP stdio、fake Adapter E2E、fan-out/
fan-in 容量、故障注入、受控真实 CC-Panes canary、安装/升级/卸载与敏感信息扫描。
