# Codex Desktop Companion

面向 Windows 的 Codex App 与本机 pane terminal workspace 安全启动、诊断和协调工具。它发现并调用用户已安装的 `cc-panes.exe`，不重新打包或修改该宿主程序。

它不重新打包 Codex App 或 CC-Panes，而是始终发现并启动用户已经安装的官方
`cc-panes.exe`。启动前会从子进程环境中移除 Codex Desktop 的任务级私有变量，
避免 CC-Panes 内部启动 Codex CLI 时误入受控 launcher 上下文。

状态：源码预览。当前 Cargo manifest 版本为 `0.1.0-beta.3`；文档中的 `0.2.0-beta.1` 是发行包合同目标，不代表该版本已发布。此公开快照没有可下载安装包或公开 CI 二进制；外部 Windows 机器验收及二进制依赖许可证包审查仍待完成。

## 核心价值

- 保持 Codex、CC-Panes 各自独立启动和官方升级；
- 把环境清理放在真实子进程边界，而不是修改 CC-Panes 源码或官方 EXE；
- 提供可审计的 probe、doctor、配置迁移、构建目录扫描和版本化安装；
- 三组件目标链路不引入 LiveAgent、Tauri、Gateway、WebUI 或 workflowd 运行依赖；
- 故障时仍可直接使用官方 Codex App 与 CC-Panes。

## 能力

- 安全启动官方 CC-Panes，保留官方自动更新能力；
- 自动发现 LocalAppData、Program Files、PATH 或显式指定的 CC-Panes；
- 只移除 `CODEX_INTERNAL_*`、`CODEX_THREAD_ID`、
  `CODEX_PERMISSION_PROFILE` 等任务级变量；
- 检查 Codex CLI、CC-Panes、遗留 Workflow 进程和构建进程；
- 生成不包含 Token 与环境变量值的 JSON 诊断报告；
- 只读扫描大型 Rust `target` 目录；
- 用户级一键安装、版本化升级和精确卸载。

## Coordination Plane 状态

三组件 Authority v2 已接受，Phase 8 external acceptance + beta notes contract
合同基线在 workspace 中包含：

- `coordination-core`：ID、revision、Workflow/Worker/Outbox 状态机、WorkflowSpec v1beta1、
  RepositoryRef 可选 `workspaceName`、DAG Step、fan-in policy、命令、查询、事件和公共错误码；
- `coordination-protocol`：MCP/IPC schema version、公共工具 DTO、RepositoryInput 可选
  `workspaceName`、JSON Schema 合同和 version negotiation，`workflow_result` 保留缺失结果与失败来源；
- `coordination-store`：SQLite schema v1、migration/backup/integrity、Workflow/Worker/
  Event/Outbox/Binding/Lease/Result repository、optimistic revision、幂等记录和 Writer
  Lease；
- `coordinator`：runtime identity、单实例锁、IPC health handler、scheduler、capacity/
  budget admission、DAG fan-out、`all-required`/`allow-partial`/`first-success` fan-in、
  Worker/Workflow timeout、dispatch/reconcile/control semaphore、pause/resume/cancel、
  result aggregation、metrics/log snapshot、repository baseline guard 和专用合成 Git
  仓库登记/恢复 guard；
- `ccpanes-adapter`：显式 discovery、认证 Header 注入、HTTP MCP initialize/tools/list、
  capability digest、launch/query/output/submit/kill、unknown dispatch reconcile、只读 preflight
  报告、Session inventory 脱敏摘要、专用 Workspace/Project 只读 gate 和 deterministic fake
  server；
- `coordination-mcp`：stdio initialize/tools/list/tools/call、公共工具 schema 曝露、输入验证、
  公共错误映射、Coordinator IPC consumer seam 和 synthetic 多 Step Workflow create/start。

当前发布包仍以 Launcher/诊断能力为主；Phase 7 当前增加只读 capability preflight 合同、
示例 runner、专用合成 Git 仓库准备合同、专用 CC-Panes Workspace/Project 只读 gate、
fake 单 Worker success canary、cancellation canary、Coordinator restart canary、dropped
launch response / unknown dispatch canary、Token/discovery rotation canary 和 synthetic
CC-Panes process restart canary，以及 synthetic multi-worker controlled capacity canary。
真实 CC-Panes preflight 仅执行 initialize、tools/list、list_sessions 和 list_panes；合成
Git 仓库只在专用空目录创建并记录 marker digest、baseline HEAD 与 dirty 状态；
Workspace/Project gate 只执行 list_workspaces/list_projects 并生成 setup plan，不调用
create_workspace 或 add_project_to_workspace；success canary 仅通过 fake Adapter 验证
`workspaceName` payload、Binding session ref 与聚合结果；cancellation canary 仅通过 fake
Adapter 验证唯一 `kill_session` 控制 outbox 与取消收敛；Coordinator restart canary 仅重新
打开同一合成 SQLite DB，验证 fake Worker 可继续查询和聚合收敛；unknown dispatch canary
仅通过 fake Adapter 验证 launch 响应丢失后 Store 立即进入
`RESULT_UNKNOWN`/`DISPATCH_UNKNOWN`、reopen 保持，再经 list_sessions/list_panes 绑定原
fake Session 并聚合结果；Token/discovery rotation
canary 仅通过 fake Adapter 验证旧 discovery/token 成功、旧 token 对新 endpoint 返回
`authorization_failed`、新 discovery generation/token 刷新 readiness 与 capability digest，
且报告和 fake 记录只保留 Header digest；process restart canary 仅通过 fake endpoint A
停止、fake endpoint B 携带同一 synthetic Session，验证旧 endpoint
`adapter_not_ready`/DEGRADED、MCP 持久状态可查询、tick 不重复 launch、新 endpoint READY
后 reconcile 和 `workflow_result` complete；multi-worker capacity canary 仅通过 MCP
`workflow_create/start`、Coordinator 和 fake Adapter 验证 6 个只读 Worker 在 host/dispatch/
per-workflow cap=3 下分两批派发，active/queued 计数有界，Binding/Session 与
`clientRequestId` 唯一，`stepId` 来源和 `workflow_result` complete 保留。通用真实
Worker 派发、真实 Session 控制、真实 Token/discovery rotation、真实 CC-Panes process restart 和后台
进程 supervisor 保持关闭；开发验收在专用合成项目执行前台 CC-Panes `0.12.3`
Codex challenge-response canary，并通过官方 CC-Panes 新对话确认全局 skill/MCP、preflight、
`launchWorkspaceName=codex-ccpanes-phase7-*`、`cwdMatched=true` 和 `acceptancePassed=true`，
未调用 submit/kill/write。Phase 8 首个本地包装切片只要求
release 包包含三个产品组件 EXE
`CodexCCPanes.exe`、`codex-ccpanes-coordinator.exe`、`codex-ccpanes-mcp.exe` 与既有辅助
CLI `codex-ccpanes.exe`，并通过 `ARTIFACT_MANIFEST.json`、`SHA256SUMS`、
`DEPENDENCIES.json` 和隔离安装状态登记验证；`Configure-CodexMcp.ps1` 通过显式临时
`ConfigPath` 验证托管 MCP block 写入、写前备份、幂等 repair 和事务 rollback 恢复
present/absent 原状态；安装器仅在显式 `-ConfigureCodexMcp -CodexConfigPath <path>`
时 apply/repair，卸载器仅在显式 `-RemoveCodexMcp -CodexConfigPath <path>` 时移除托管
block；`Manage-Coordinator.ps1` 提供显式 `Describe`、`Verify`、前台一次性 `Health` 和
`StartPlan`，安装默认仍不会启动后台 Coordinator；`EXTERNAL_WINDOWS_ACCEPTANCE.md`、
`Test-ExternalWindowsAcceptance.ps1` 和 `RELEASE_NOTES_0.2.0-beta.1.md` 进入 package，
并由 Manifest 声明外部 Windows 验收 runner 与 beta release notes 合同。package/default
MCP 注册、后台守护、通用 Coordinator↔Adapter 派发、真实 CC-Panes 多 Worker E2E、外部
Windows 机器实际验收和真实发布属于后续阶段。

## 下载与安装

当前公开快照暂未发布可下载的安装包。仓库源码可审阅，外部 Windows 机器验收仍未完成；请勿将历史 beta 测试记录视为当前版本的发布验收。发布资产可用后，本节会补充对应版本的下载与 SHA-256 链接。

开发者可按 [贡献指南](CONTRIBUTING.md) 编译和运行检查。源码根目录中的安装脚本面向构建后的发行包，不应直接从源码检出目录运行。

## 命令行使用

```powershell
# 默认安全启动 CC-Panes
codex-ccpanes.exe start

# 健康检查
codex-ccpanes.exe doctor
codex-ccpanes.exe doctor --json --output .\doctor.json

# 只读检查真实官方安装
codex-ccpanes.exe probe

# 查找大型 Rust 构建目录，不执行删除
codex-ccpanes.exe audit-builds <build-cache-root> --threshold-gb 5

# 预览旧配置迁移；加入 --apply 才会写入 Companion 自己的配置
codex-ccpanes.exe migrate-config --from .\legacy.json --json
codex-ccpanes.exe migrate-config --ccpanes-bin "$env:LOCALAPPDATA\cc-panes\cc-panes.exe" --apply
```

桌面快捷方式使用无控制台窗口的 `CodexCCPanes.exe`。

## 维护入口

维护本仓库时，先从仓库根目录阅读：

```text
AGENTS.md
docs/START_HERE.md
docs/AUTHORITY.md
```

详细架构、运维、兼容与发布门禁见 [`docs/`](docs/)。历史 beta 测试记录及外部验收合同分别标明其适用范围，不代表当前版本已完成发布验收。
