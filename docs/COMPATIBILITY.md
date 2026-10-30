# 兼容策略

## 当前验证基线

- Windows 11 x64
- Codex CLI `0.146.0`
- CC-Panes `0.11.10`、`0.11.11`
- CC-Panes `v0.12.3` (`7fad08d`) capability/schema 合成快照；本机安装 EXE
  FileVersion/ProductVersion 已只读确认为 `0.12.3`，真实 HTTP MCP read-only preflight
  已通过 dispatch 再资格 gate（READY、`dispatchReady=true`、inventory 存在、零 Worker
  副作用），并完成一次专用合成项目
  Coordinator→Adapter MCP launch/status/output 新 Codex 对话验证。2026-08-11 发现
  v0.12.3 会用 `launch_task.workspaceName` 对应 Workspace 的 `path` 作为实际 cwd；
  当前代码新增“source workspace 元数据 + projectPath 专用 launch workspace”路由合同，
  synthetic/fake 测试已覆盖 create/add setup 与 `cwdMatched=true`，官方 CC-Panes 新对话
  入口验收已确认 `launchWorkspaceName` 为 projectPath 专用 Workspace、`cwdMatched=true`
  和 `acceptancePassed=true`。

## 版本策略

- Codex CLI：不固定版本，通过 PATH 和官方安装目录探测。
- CC-Panes Launcher：最低建议版本 `0.11.3`，不设置最高版本上限。
- CC-Panes Adapter：不按版本字符串放行，按 MCP protocol、tool inventory 和 input schema
  再资格；当前已知兼容版本为 `v0.12.3`，证据覆盖合成合同、真实只读 preflight 和一次
  受控 Coordinator→Adapter 前台单 Worker challenge-response canary；cwd 路由恢复覆盖
  fake/synthetic 合同测试和官方 CC-Panes 新对话入口验收。
- LiveAgent：无运行依赖。

## 前向兼容

Launcher 不调用 CC-Panes 私有 API，只使用 Windows 进程启动和文件发现。Coordination
Plane 通过官方 HTTP MCP 边界工作；Adapter 基础 READY 只要求 `launch_task`、
`list_sessions`、`list_panes`，派发另要求 `get_session_status/get_session_output`，
submit/kill/wait/write 按逐工具 schema 门控。`get_session_output` 接受 `content`、`lines`
和历史 `output`，`kill_session.reason` 仅在上游 schema 声明时发送。
对 v0.12.3 的 `workspace.path` cwd 语义，Coordinator 不把 source `workspaceName`
直接传给 `launch_task`；它始终按 `projectPath` 生成
`codex-ccpanes-phase7-<sha256-prefix>` 专用 launch workspace，Adapter 在受控 canary/launch
前调用 `create_workspace(name,path=projectPath)` 和
`add_project_to_workspace(workspaceName,projectPath)`，并在报告中保留
`sourceWorkspaceName`、`launchWorkspaceName`、期望路径 digest 与 `cwdMatched`。prompt
只要求报告当前工作目录，不通过手动目录切换修正 cwd；cwd marker 缺失或不匹配时
canary typed failure。
官方更新后的常规 dispatch 再资格 runner 复用 Adapter 报告，并额外要求基础 READY、
`dispatchReady=true`、Session inventory 存在和 `externalWorkerSideEffects=false`；不满足时
以 typed error 非零退出，不进入 `launch_task`。

需要重新验证的上游变化：

- 官方安装目录或可执行文件名改变；
- CC-Panes 从子进程环境读取方式发生变化；
- Codex Desktop 私有环境变量命名发生变化；
- Windows 安装模型从 Win32 改为受限容器。
- MCP protocol 版本变化；
- 当前使用工具被删除、改名或 required/input schema 改变；
- launch/session 返回 shape 不再满足 Adapter 的 typed parser。
- `launch_task.workspaceName`、Workspace `path` 或终端 cwd 规则再次改变。

`docs/89-mcp-tool-surface-reduction.md` 描述的工具面收编方向按实际 capability 处理，不把
未使用工具列为全局 required。当前 v0.12.3 声明覆盖 fixture/schema snapshot、fake MCP、
真实 read-only preflight，以及一个新建 Codex 对话的
Coordinator→Adapter MCP `launch_task`、`get_session_status`、`get_session_output`
受控 canary。Worker 只计算随机 challenge 的 Unicode 逆序响应，expected response 不进入
prompt；此前用户级新对话入口另验证 skill/MCP discovered、READY、
`externalWorkerSideEffects=false`、launch=1、submit/kill/write=0、Session idle 和 Store
`ACTIVE/ACKNOWLEDGED/SUCCEEDED/SUCCEEDED/complete`，并在 cwd 路由恢复后确认
`launchWorkspaceName` 为 projectPath 专用 workspace、`cwdMatched=true`、
`acceptancePassed=true`。这些证据只覆盖一次性前台单 Worker success path，不外推到
submit/kill/write 控制、后台 Coordinator、多 Worker、restart 或 unknown-outcome。
