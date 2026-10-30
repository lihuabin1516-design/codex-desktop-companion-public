# ADR-0001：恢复独立三组件 Coordination Plane

状态：**Accepted**

日期：2026-08-07

接受日期：2026-08-07

决策所有者：Codex + PaneForge Companion

目标版本：`0.2.0-beta.1`

## 背景

当前 Companion 已稳定承担官方 PaneForge 发现、任务级 Codex Desktop 环境隔离、诊断、
安装和上游升级透明，但它没有 Codex App 到 PaneForge Session 的调度、状态、取消和结果
回传能力。

旧 Workflow Fabric 曾实现 Workflow MCP、持久化调度器、SQLite Outbox、PaneForge HTTP
MCP Adapter、Worker 控制与 Session 对账。清理 LiveAgent GUI、Tauri、Gateway、持续
runner 和重复构建链时，编排控制面没有迁移到独立仓库，导致产品合同被收缩为 Launcher。

## 决策

在 Companion 独立代码库内实现一个无 GUI、无 LiveAgent 运行依赖的 Coordination Plane，
形成三个可独立部署和失效隔离的可执行文件：

1. `CodexPaneForge.exe`
   - 继续负责官方 PaneForge 的洁净启动。
   - 保持现有环境隔离、发现和升级透明合同。
   - 不持有 Workflow 或 Worker 权威状态。

2. `codex-paneforge-coordinator.exe`
   - 成为 Workflow、Worker、ExternalBinding、Lease、Event、Outbox 和 Result 的唯一
     本机生产状态源。
   - 负责并发准入、fan-out/fan-in、超时、重试、取消、未知结果对账和结果聚合。
   - 通过公开 PaneForge HTTP MCP 能力驱动 Session，不读取或修改 PaneForge 数据库。

3. `codex-paneforge-mcp.exe`
   - 作为 Codex App 的 stdio MCP Server。
   - 只暴露高层 Workflow/Worker 工具，不向 Codex 暴露低层 PTY 操作。
   - 自身保持无权威持久状态，所有命令和查询进入 Coordinator。

产品结构采用“现有根 package + 新 workspace members”，避免为恢复控制面搬迁并重写已经
验证的 Launcher 代码。

Phase 1 的接受边界只落地 `coordination-core` 和 `coordination-protocol` 合同，不实现
SQLite、Coordinator runtime、PaneForge Adapter、MCP 进程或真实 E2E。

## 权威边界

| 概念 | 唯一权威 |
|---|---|
| Codex App 对话、内置 Subagent | OpenAI Codex |
| PaneForge PTY、Session、Pane、实时输出 | 官方 PaneForge |
| Workflow、Worker、派发意图、重试、聚合结果 | Coordinator |
| MCP 工具协议与公共错误码 | Companion protocol crate |
| 官方 EXE 安装与更新 | 各自官方更新器 |
| Launcher 环境清理规则 | Companion launcher |

Coordinator 只记录 PaneForge Session 的外部绑定镜像，不取得 PaneForge Session 本体的
状态所有权。Codex App thread 与外部 Worker 通过 Workflow/Worker ID 关联，不伪装成同一
对话或同一 Session。

## 拒绝的方向

### 恢复完整 LiveAgent

该方向重新引入 Tauri、Gateway、WebUI、大型 workspace、sidecar staging 和持续 runner，
恢复此前的 CPU、磁盘和发布耦合风险。

### 让 Codex App 与 PaneForge GUI 互相接管

两者没有共享的对话、审批、任务树或 Session 状态权威。目标链路采用公开 MCP 和结构化
Worker 结果，不建立 GUI 注入或私有状态耦合。

### 只给 Launcher 增加若干 Session 命令

低层命令缺少持久状态、幂等、未知结果、恢复和 fan-in，无法形成生产级工作流语义。

## 影响

- `docs/AUTHORITY.md` 已升级为 v2，从单一 Launcher 运维桥改为三组件边界。
- Cargo workspace 将增加 Core、Store、Coordinator、PaneForge Adapter、Protocol/MCP
  crates。
- 安装包需要注册两个新可执行文件，并提供 Coordinator 的启动、停止、修复和卸载语义。
- 发布门禁需要加入状态机、MCP Contract、故障注入、重启恢复、并发容量和上游版本
  再资格。
- 现有 Launcher CLI、GUI 快捷方式和官方 PaneForge 独立启动行为保持兼容。

## 验证

接受该 ADR 后分阶段验证：

1. Phase 0/1：当前 Launcher 回归保持通过；Core/Protocol 合同测试覆盖状态机、revision、
   duplicate request、unknown schema version 和公共错误码。
2. Phase 2+：MCP 重放同一 `requestId` 最多创建一个 Worker 和一个外部 Session。
3. Phase 3+：Coordinator 在派发响应丢失、进程退出和重启后能够对账到唯一 Session。
4. Phase 6+：Codex App 可以通过 synthetic MCP/Coordinator 合同派发多个只读 Worker，
   并收到有来源标识的聚合结果。
5. Beta 前：Coordinator、MCP 或 Launcher 任一退出不破坏另外两个产品的独立使用。
6. Beta 前：PaneForge/Codex 更新后按能力协议和 Schema 再资格，而不是按精确版本字符串放行。
7. Beta 前：空闲 Coordinator 资源受限，构建缓存保持单一外置目录。

## 回退与反转条件

- Coordination Plane 通过 feature/install profile 与 Launcher 解耦；Beta 阶段可以停止并
  卸载 Coordinator/MCP，同时保留 Launcher。
- SQLite schema 每次变更都必须有前向迁移、备份和完整性检查；禁止依赖降级二进制读取
  新 schema。
- 出现重复 Session、权威分裂、不可对账的外部副作用或真实项目状态污染时，冻结新派发，
  保留状态与审计证据，回到最后一个通过的发布包。
