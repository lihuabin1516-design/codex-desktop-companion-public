# External Windows machine acceptance contract

Contract markers:

- contract: external-windows-acceptance/v1
- product: CodexCCPanesCompanion
- targetVersion: 0.2.0-beta.1
- acceptanceRunner: Test-ExternalWindowsAcceptance.ps1
- defaultRegisterUserMcp: false
- defaultStartBackgroundCoordinator: false
- configScope: synthetic
- realDispatchEnabled: false
- modifiesOfficialCodex: false
- modifiesOfficialCcPanes: false

本文件定义 Phase 8 package 在另一台 Windows x64 机器上的验收步骤。它是验收合同，
不是本仓库当前执行过的验收报告；每台外部机器必须记录实际命令、exit code 和关键输出。

## 验收边界

- 只验证 release package、安装脚本、三组件 inventory、合成 Codex MCP 配置事务和
  Coordinator lifecycle entry。
- 默认安装不得注册用户 Codex MCP，不得启动后台 Coordinator，不得修改官方 Codex、
  官方 CC-Panes、`~/.codex`、`~/.cc-panes` 或真实项目。
- Codex config 验收必须使用临时合成 `config.toml`。真实用户 config 只可在后续明确门禁中
  使用。
- 真实 CC-Panes/Codex 探测只能作为只读 probe/doctor 记录；缺少官方安装时，本合同中真实
  probe 记为 `not-run`，不使 Phase 8 package 验收失败。
- 任何真实 Worker 派发、真实 Session 控制、真实 CC-Panes process restart、后台守护注册、
  Windows service、scheduled task 或自启动项均不属于本合同。

## 机器与输入

记录以下事实：

```text
machineId: <local label, not PII>
windowsVersion: <winver or systeminfo summary>
architecture: x64
powershellVersion: $PSVersionTable.PSVersion
packageRoot: <expanded package directory>
zipSha256: <64 hex>
packageVersion: <VERSION file>
signatureStatus: <per EXE Authenticode status>
```

验收使用解压后的 package root，示例：

```powershell
$PackageRoot = "C:\Temp\CodexCCPanes-<VERSION>-win-x64"
$AcceptanceRoot = Join-Path $env:TEMP ("codex-ccpanes-external-acceptance-" + [guid]::NewGuid().ToString("N"))
$InstallRoot = Join-Path $AcceptanceRoot "install"
$SyntheticCodexConfig = Join-Path $AcceptanceRoot "codex-home\config.toml"
```

## Package 自带合成验收 runner

解压后的 package root 包含 `Test-ExternalWindowsAcceptance.ps1`。外部机器可以先运行自带
runner 完成 package inventory、checksum、隔离安装、Coordinator lifecycle、synthetic
Codex MCP config transaction 和卸载检查：

```powershell
powershell -NoLogo -NoProfile -ExecutionPolicy Bypass -File .\Test-ExternalWindowsAcceptance.ps1 -Json
```

该 runner 只使用 `%TEMP%\codex-ccpanes-external-acceptance-*` 下的临时 install root 和
synthetic Codex config；它不读取或修改真实 `~/.codex/config.toml`，不注册真实 MCP，不启动
后台 Coordinator，不执行真实 Worker 派发。runner 会记录启动前已存在的 Companion
MCP/Coordinator 进程基线，并只把本轮新增同名进程残留判为失败。runner 通过后，仍需把机器事实、
签名状态和真实 probe/doctor 的 `pass/warn/not-run` 状态写入验收记录。

## 必跑检查

### 1. Package inventory 与 checksum

```powershell
Set-Location $PackageRoot
Get-Content .\VERSION
Get-FileHash .\CodexCCPanes.exe, .\codex-ccpanes.exe, .\codex-ccpanes-coordinator.exe, .\codex-ccpanes-mcp.exe -Algorithm SHA256
Get-AuthenticodeSignature .\CodexCCPanes.exe, .\codex-ccpanes.exe, .\codex-ccpanes-coordinator.exe, .\codex-ccpanes-mcp.exe |
  Select-Object Path, Status, StatusMessage
```

逐行验证 `SHA256SUMS` 覆盖除自身以外的全部包文件；仓库脚本可用时运行：

```powershell
powershell -NoLogo -NoProfile -ExecutionPolicy Bypass -File <repo>\scripts\verify-package.ps1 -PackageRoot $PackageRoot
```

公开生产发布必须使用 `-RequireSignature`；本地 beta package 可以记录为 unsigned。

### 2. 隔离 clean install

```powershell
powershell -NoLogo -NoProfile -ExecutionPolicy Bypass -File .\Install-CodexCCPanes.ps1 `
  -InstallRoot $InstallRoot `
  -SkipShortcuts `
  -SkipRegistry `
  -Quiet
```

期望：

- exit code = 0；
- `$InstallRoot\install-state.json` 与 `$InstallRoot\current.json` 存在；
- `current.json` 登记 `CodexCCPanes.exe`、`codex-ccpanes.exe`、
  `codex-ccpanes-coordinator.exe`、`codex-ccpanes-mcp.exe` 和
  `Manage-Coordinator.ps1`；
- 默认安装未修改真实 Codex config，未启动后台 Coordinator。

### 3. Coordinator lifecycle entry

```powershell
$Lifecycle = Join-Path $InstallRoot "Manage-Coordinator.ps1"
powershell -NoLogo -NoProfile -ExecutionPolicy Bypass -File $Lifecycle -Action Describe -InstallRoot $InstallRoot -Json -Quiet
powershell -NoLogo -NoProfile -ExecutionPolicy Bypass -File $Lifecycle -Action Verify -InstallRoot $InstallRoot -Json -Quiet
powershell -NoLogo -NoProfile -ExecutionPolicy Bypass -File $Lifecycle -Action Health -InstallRoot $InstallRoot -Json -Quiet
powershell -NoLogo -NoProfile -ExecutionPolicy Bypass -File $Lifecycle -Action StartPlan -InstallRoot $InstallRoot -Json -Quiet
```

期望：

- 四个命令 exit code = 0；
- `Describe`/`Verify`/`Health`/`StartPlan` 均返回 `ok=true`；
- `defaultStartByInstaller=false`；
- `startsBackgroundCoordinator=false`；
- `Health` 前台运行一次并退出；
- `StartPlan.startPlan.startsProcess=false` 且只返回 `--stdio-ipc` 命令计划。

### 4. Synthetic Codex MCP config transaction

```powershell
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $SyntheticCodexConfig) | Out-Null
@"
# synthetic Codex config for external acceptance
[mcp_servers.other]
command = "other"
"@ | Set-Content -LiteralPath $SyntheticCodexConfig -Encoding UTF8
```

使用 `install-state.json` 中的 `currentVersionRoot` 和 `mcpExecutable` 继续执行：

```powershell
$State = Get-Content (Join-Path $InstallRoot "install-state.json") -Raw | ConvertFrom-Json
$ConfigManager = Join-Path $State.currentVersionRoot "Configure-CodexMcp.ps1"
$Before = (Get-FileHash -LiteralPath $SyntheticCodexConfig -Algorithm SHA256).Hash

$Apply = powershell -NoLogo -NoProfile -ExecutionPolicy Bypass -File $ConfigManager `
  -Action Apply `
  -ConfigPath $SyntheticCodexConfig `
  -InstallRoot $InstallRoot `
  -McpExecutable $State.mcpExecutable `
  -Json `
  -Quiet

powershell -NoLogo -NoProfile -ExecutionPolicy Bypass -File $ConfigManager `
  -Action Verify `
  -ConfigPath $SyntheticCodexConfig `
  -InstallRoot $InstallRoot `
  -McpExecutable $State.mcpExecutable `
  -RequireRegistered `
  -Json `
  -Quiet

powershell -NoLogo -NoProfile -ExecutionPolicy Bypass -File $ConfigManager `
  -Action Repair `
  -ConfigPath $SyntheticCodexConfig `
  -InstallRoot $InstallRoot `
  -McpExecutable $State.mcpExecutable `
  -Json `
  -Quiet

$Transaction = ($Apply | ConvertFrom-Json).transactionPath
powershell -NoLogo -NoProfile -ExecutionPolicy Bypass -File $ConfigManager `
  -Action Rollback `
  -TransactionPath $Transaction `
  -Json `
  -Quiet

$After = (Get-FileHash -LiteralPath $SyntheticCodexConfig -Algorithm SHA256).Hash
if ($After -ne $Before) { throw "synthetic Codex config rollback hash mismatch" }
```

期望：托管 block 只出现一次，rollback 恢复原 SHA-256。

### 5. Explicit uninstall

```powershell
powershell -NoLogo -NoProfile -ExecutionPolicy Bypass -File .\Uninstall-CodexCCPanes.ps1 `
  -InstallRoot $InstallRoot `
  -SkipShortcuts `
  -SkipRegistry `
  -Quiet
```

期望：Companion 登记的 version directories、`current.json`、`install-state.json` 和
root `Manage-Coordinator.ps1` 被移除；官方 Codex、官方 CC-Panes、真实用户 config 和真实
项目未被修改。

## 验收记录格式

每台外部机器输出一份独立记录：

```text
packageVersion:
zipSha256:
signatureStatus:
verifyPackage: pass/fail/not-run
install: pass/fail
coordinatorLifecycleDescribe: pass/fail
coordinatorLifecycleVerify: pass/fail
coordinatorLifecycleHealth: pass/fail
coordinatorLifecycleStartPlan: pass/fail
syntheticMcpConfigTransaction: pass/fail
uninstall: pass/fail
realProbe: pass/fail/not-run
realDoctor: pass/fail/warn/not-run
preExistingCompanionBackgroundProcesses: <count>
remainingNewCompanionBackgroundProcesses: 0
residualRisk:
```

出现 checksum mismatch、托管 block 重复、真实 config 被改动、后台 Coordinator 进程残留、
官方安装文件被修改或敏感字段进入报告时，该机器验收为 `fail`，停止后续发布动作。
