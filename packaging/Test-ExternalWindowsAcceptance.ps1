[CmdletBinding()]
param(
    [string]$PackageRoot,
    [string]$AcceptanceRoot,
    [switch]$KeepAcceptanceRoot,
    [switch]$Json,
    [switch]$Quiet
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ProductId = "CodexCCPanesCompanion"
$AcceptancePrefix = "codex-ccpanes-external-acceptance-"

if ([string]::IsNullOrWhiteSpace($PackageRoot)) {
    if (-not [string]::IsNullOrWhiteSpace($PSScriptRoot)) {
        $PackageRoot = $PSScriptRoot
    }
    elseif (-not [string]::IsNullOrWhiteSpace($MyInvocation.MyCommand.Path)) {
        $PackageRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
    }
    else {
        $PackageRoot = (Get-Location).Path
    }
}

function Write-Step {
    param([string]$Message)
    if (-not $Quiet -and -not $Json) {
        Write-Host "[Codex + CC-Panes] $Message"
    }
}

function Assert-ChildPath {
    param(
        [string]$Parent,
        [string]$Child
    )
    $parentFull = [System.IO.Path]::GetFullPath($Parent).TrimEnd('\') + '\'
    $childFull = [System.IO.Path]::GetFullPath($Child)
    if (-not $childFull.StartsWith($parentFull, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Path escapes parent: $childFull"
    }
}

function Assert-AcceptanceRoot {
    param([string]$Path)
    $full = [System.IO.Path]::GetFullPath($Path)
    $temp = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (-not $full.StartsWith($temp, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "AcceptanceRoot must be under the user temp directory: $full"
    }
    if ((Split-Path -Leaf $full) -notlike "$AcceptancePrefix*") {
        throw "AcceptanceRoot leaf must start with $AcceptancePrefix"
    }
}

function Convert-ToRelativePath {
    param(
        [string]$Root,
        [string]$Path
    )
    $rootFull = [System.IO.Path]::GetFullPath($Root).TrimEnd('\') + '\'
    $pathFull = [System.IO.Path]::GetFullPath($Path)
    if (-not $pathFull.StartsWith($rootFull, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Path escapes package root: $pathFull"
    }
    return $pathFull.Substring($rootFull.Length)
}

function Invoke-WindowsPowerShell {
    param(
        [string]$ScriptPath,
        [string[]]$Arguments
    )
    $output = & powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File $ScriptPath @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Windows PowerShell command failed with exit code ${LASTEXITCODE}: $ScriptPath"
    }
    return ,$output
}

function ConvertFrom-JsonOutput {
    param([object[]]$Output)
    $text = ($Output | Out-String).Trim()
    if ([string]::IsNullOrWhiteSpace($text)) {
        throw "Expected JSON output was empty."
    }
    return $text | ConvertFrom-Json
}

function Get-FileSnapshot {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        return [ordered]@{
            exists = $false
            bytes = 0
            sha256 = $null
        }
    }
    $item = Get-Item -LiteralPath $Path
    return [ordered]@{
        exists = $true
        bytes = $item.Length
        sha256 = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash
    }
}

function Get-CompanionBackgroundProcesses {
    return @(Get-Process -ErrorAction SilentlyContinue |
        Where-Object { $_.ProcessName -in @("codex-ccpanes-coordinator", "codex-ccpanes-mcp") } |
        ForEach-Object {
            [ordered]@{
                processName = [string]$_.ProcessName
                id = [int]$_.Id
            }
        })
}

function Verify-Checksums {
    param([string]$Root)
    $checksums = Join-Path $Root "SHA256SUMS"
    if (-not (Test-Path -LiteralPath $checksums -PathType Leaf)) {
        throw "SHA256SUMS is missing."
    }
    $expected = @{}
    foreach ($line in Get-Content -LiteralPath $checksums) {
        if ([string]::IsNullOrWhiteSpace($line)) {
            continue
        }
        if ($line -notmatch '^([A-Fa-f0-9]{64})\s+\*(.+)$') {
            throw "Malformed SHA256SUMS entry: $line"
        }
        $relative = $Matches[2].Replace('/', '\')
        if ([string]::IsNullOrWhiteSpace($relative) -or
            [System.IO.Path]::IsPathRooted($relative) -or
            $relative -eq "SHA256SUMS") {
            throw "Unsafe SHA256SUMS path: $relative"
        }
        if ($expected.ContainsKey($relative)) {
            throw "Duplicate SHA256SUMS path: $relative"
        }
        $expected[$relative] = $Matches[1].ToUpperInvariant()
    }
    if ($expected.Count -lt 1) {
        throw "SHA256SUMS is empty."
    }

    $packageFiles = @(Get-ChildItem -LiteralPath $Root -Recurse -File |
        Where-Object { $_.Name -ne "SHA256SUMS" })
    foreach ($file in $packageFiles) {
        $relative = Convert-ToRelativePath -Root $Root -Path $file.FullName
        if (-not $expected.ContainsKey($relative)) {
            throw "Package file is not covered by SHA256SUMS: $relative"
        }
    }
    if ($packageFiles.Count -ne $expected.Count) {
        throw "SHA256SUMS file count does not match the package payload."
    }
    foreach ($entry in $expected.GetEnumerator()) {
        $path = Join-Path $Root $entry.Key
        Assert-ChildPath -Parent $Root -Child $path
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            throw "Checksum entry missing file: $($entry.Key)"
        }
        $actual = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash
        if ($actual -ne $entry.Value) {
            throw "Checksum mismatch: $($entry.Key)"
        }
    }
    return $expected.Count
}

$PackageRoot = [System.IO.Path]::GetFullPath($PackageRoot)
if ([string]::IsNullOrWhiteSpace($AcceptanceRoot)) {
    $AcceptanceRoot = Join-Path ([System.IO.Path]::GetTempPath()) ($AcceptancePrefix + [guid]::NewGuid().ToString("N"))
}
$AcceptanceRoot = [System.IO.Path]::GetFullPath($AcceptanceRoot)
Assert-AcceptanceRoot -Path $AcceptanceRoot
if (Test-Path -LiteralPath $AcceptanceRoot) {
    throw "AcceptanceRoot already exists: $AcceptanceRoot"
}

$InstallRoot = Join-Path $AcceptanceRoot "install"
$SyntheticCodexConfig = Join-Path $AcceptanceRoot "codex-home\config.toml"
$AbsentCodexConfig = Join-Path $AcceptanceRoot "absent-codex-home\config.toml"
$preExistingBackground = @(Get-CompanionBackgroundProcesses)
$preExistingBackgroundIds = @{}
foreach ($process in $preExistingBackground) {
    $preExistingBackgroundIds[[string]$process.id] = $true
}
$report = $null

try {
    Write-Step "Validating package payload"
    foreach ($required in @(
        "VERSION",
        "SHA256SUMS",
        "ARTIFACT_MANIFEST.json",
        "EXTERNAL_WINDOWS_ACCEPTANCE.md",
        "RELEASE_NOTES_0.2.0-beta.1.md",
        "Test-ExternalWindowsAcceptance.ps1",
        "Install-CodexCCPanes.ps1",
        "Uninstall-CodexCCPanes.ps1",
        "codex-ccpanes.exe",
        "CodexCCPanes.exe",
        "codex-ccpanes-coordinator.exe",
        "codex-ccpanes-mcp.exe"
    )) {
        if (-not (Test-Path -LiteralPath (Join-Path $PackageRoot $required) -PathType Leaf)) {
            throw "Required package file missing: $required"
        }
    }

    $version = (Get-Content -LiteralPath (Join-Path $PackageRoot "VERSION") -Raw).Trim()
    $artifactManifest = Get-Content -LiteralPath (Join-Path $PackageRoot "ARTIFACT_MANIFEST.json") -Raw |
        ConvertFrom-Json
    if ($artifactManifest.externalWindowsAcceptance.contractVersion -ne "external-windows-acceptance/v1" -or
        $artifactManifest.externalWindowsAcceptance.acceptanceRunner -ne "Test-ExternalWindowsAcceptance.ps1" -or
        $artifactManifest.externalWindowsAcceptance.usesSyntheticCodexConfigOnly -ne $true -or
        $artifactManifest.externalWindowsAcceptance.realDispatchEnabled -ne $false -or
        $artifactManifest.betaReleaseNotes.contractVersion -ne "beta-release-notes/v1" -or
        $artifactManifest.betaReleaseNotes.statesNoRealDispatch -ne $true) {
        throw "Artifact manifest acceptance contract is invalid."
    }
    $filesVerified = Verify-Checksums -Root $PackageRoot

    $signatureStates = @()
    foreach ($binary in @(
        "CodexCCPanes.exe",
        "codex-ccpanes.exe",
        "codex-ccpanes-coordinator.exe",
        "codex-ccpanes-mcp.exe"
    )) {
        $signature = Get-AuthenticodeSignature -LiteralPath (Join-Path $PackageRoot $binary)
        $signatureStates += "$binary=$($signature.Status)"
    }

    Write-Step "Installing into isolated root"
    New-Item -ItemType Directory -Force -Path $AcceptanceRoot | Out-Null
    Invoke-WindowsPowerShell -ScriptPath (Join-Path $PackageRoot "Install-CodexCCPanes.ps1") `
        -Arguments @("-InstallRoot", $InstallRoot, "-SkipShortcuts", "-SkipRegistry", "-Quiet") | Out-Null
    $statePath = Join-Path $InstallRoot "install-state.json"
    $currentPath = Join-Path $InstallRoot "current.json"
    if (-not (Test-Path -LiteralPath $statePath -PathType Leaf) -or
        -not (Test-Path -LiteralPath $currentPath -PathType Leaf)) {
        throw "Install state or current descriptor is missing."
    }
    $state = Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json
    $current = Get-Content -LiteralPath $currentPath -Raw | ConvertFrom-Json
    if ($state.productId -ne $ProductId -or
        [string]::IsNullOrWhiteSpace([string]$state.coordinatorExecutable) -or
        [string]::IsNullOrWhiteSpace([string]$state.mcpExecutable) -or
        [string]::IsNullOrWhiteSpace([string]$current.coordinatorLifecycleScript)) {
        throw "Installed Coordination Plane state is incomplete."
    }

    Write-Step "Checking Coordinator lifecycle"
    $lifecycle = Join-Path $InstallRoot "Manage-Coordinator.ps1"
    $describe = ConvertFrom-JsonOutput -Output (Invoke-WindowsPowerShell -ScriptPath $lifecycle `
        -Arguments @("-Action", "Describe", "-InstallRoot", $InstallRoot, "-Json", "-Quiet"))
    $verify = ConvertFrom-JsonOutput -Output (Invoke-WindowsPowerShell -ScriptPath $lifecycle `
        -Arguments @("-Action", "Verify", "-InstallRoot", $InstallRoot, "-Json", "-Quiet"))
    $health = ConvertFrom-JsonOutput -Output (Invoke-WindowsPowerShell -ScriptPath $lifecycle `
        -Arguments @("-Action", "Health", "-InstallRoot", $InstallRoot, "-Json", "-Quiet"))
    $startPlan = ConvertFrom-JsonOutput -Output (Invoke-WindowsPowerShell -ScriptPath $lifecycle `
        -Arguments @("-Action", "StartPlan", "-InstallRoot", $InstallRoot, "-Json", "-Quiet"))
    if ($describe.ok -ne $true -or
        $verify.ok -ne $true -or
        $health.ok -ne $true -or
        $startPlan.ok -ne $true -or
        $describe.startsBackgroundCoordinator -ne $false -or
        $verify.startsBackgroundCoordinator -ne $false -or
        $health.startsBackgroundCoordinator -ne $false -or
        $startPlan.startsBackgroundCoordinator -ne $false -or
        $health.health.status -ne "FROZEN" -or
        $health.health.dispatchReady -ne $false -or
        $startPlan.startPlan.startsProcess -ne $false) {
        throw "Coordinator lifecycle acceptance contract failed."
    }

    Write-Step "Checking synthetic Codex MCP config transaction"
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $SyntheticCodexConfig) | Out-Null
    Set-Content -LiteralPath $SyntheticCodexConfig -Value @(
        "# synthetic Codex config for external acceptance",
        "[mcp_servers.other]",
        "command = `"other`""
    ) -Encoding ASCII
    $beforeConfig = Get-FileSnapshot -Path $SyntheticCodexConfig
    $configManager = Join-Path $state.currentVersionRoot "Configure-CodexMcp.ps1"
    $apply = ConvertFrom-JsonOutput -Output (Invoke-WindowsPowerShell -ScriptPath $configManager `
        -Arguments @("-Action", "Apply", "-ConfigPath", $SyntheticCodexConfig, "-InstallRoot", $InstallRoot,
            "-McpExecutable", ([string]$state.mcpExecutable), "-Json", "-Quiet"))
    if ($apply.ok -ne $true -or -not (Test-Path -LiteralPath $apply.transactionPath -PathType Leaf)) {
        throw "Synthetic Codex MCP apply did not create a transaction."
    }
    ConvertFrom-JsonOutput -Output (Invoke-WindowsPowerShell -ScriptPath $configManager `
        -Arguments @("-Action", "Verify", "-ConfigPath", $SyntheticCodexConfig, "-InstallRoot", $InstallRoot,
            "-McpExecutable", ([string]$state.mcpExecutable), "-RequireRegistered", "-Json", "-Quiet")) | Out-Null
    ConvertFrom-JsonOutput -Output (Invoke-WindowsPowerShell -ScriptPath $configManager `
        -Arguments @("-Action", "Repair", "-ConfigPath", $SyntheticCodexConfig, "-InstallRoot", $InstallRoot,
            "-McpExecutable", ([string]$state.mcpExecutable), "-Json", "-Quiet")) | Out-Null
    $managedMatches = Select-String -LiteralPath $SyntheticCodexConfig `
        -Pattern "# BEGIN CodexCCPanesCompanion MCP codex_ccpanes" | Measure-Object
    if ($managedMatches.Count -ne 1) {
        throw "Synthetic Codex MCP repair duplicated the managed block."
    }
    ConvertFrom-JsonOutput -Output (Invoke-WindowsPowerShell -ScriptPath $configManager `
        -Arguments @("-Action", "Rollback", "-TransactionPath", ([string]$apply.transactionPath), "-Json", "-Quiet")) |
        Out-Null
    $afterConfig = Get-FileSnapshot -Path $SyntheticCodexConfig
    if ($beforeConfig.exists -ne $afterConfig.exists -or
        $beforeConfig.bytes -ne $afterConfig.bytes -or
        $beforeConfig.sha256 -ne $afterConfig.sha256) {
        throw "Synthetic Codex MCP rollback did not restore the original config snapshot."
    }

    $absentApply = ConvertFrom-JsonOutput -Output (Invoke-WindowsPowerShell -ScriptPath $configManager `
        -Arguments @("-Action", "Apply", "-ConfigPath", $AbsentCodexConfig, "-InstallRoot", $InstallRoot,
            "-McpExecutable", ([string]$state.mcpExecutable), "-Json", "-Quiet"))
    ConvertFrom-JsonOutput -Output (Invoke-WindowsPowerShell -ScriptPath $configManager `
        -Arguments @("-Action", "Rollback", "-TransactionPath", ([string]$absentApply.transactionPath),
            "-Json", "-Quiet")) | Out-Null
    if (Test-Path -LiteralPath $AbsentCodexConfig) {
        throw "Synthetic Codex MCP absent-config rollback left a config file."
    }

    Write-Step "Uninstalling isolated root"
    Invoke-WindowsPowerShell -ScriptPath (Join-Path $PackageRoot "Uninstall-CodexCCPanes.ps1") `
        -Arguments @("-InstallRoot", $InstallRoot, "-SkipShortcuts", "-SkipRegistry", "-Quiet") | Out-Null
    if ((Test-Path -LiteralPath $statePath -PathType Leaf) -or
        (Test-Path -LiteralPath (Join-Path $InstallRoot "Manage-Coordinator.ps1") -PathType Leaf)) {
        throw "Uninstall left managed install state."
    }

    $newBackground = @(Get-CompanionBackgroundProcesses |
        Where-Object { -not $preExistingBackgroundIds.ContainsKey([string]$_.id) })
    if ($newBackground.Count -ne 0) {
        throw "New Companion background process remains after acceptance run."
    }

    $report = [ordered]@{
        schemaVersion = 1
        productId = $ProductId
        packageRoot = $PackageRoot
        packageVersion = $version
        filesVerified = $filesVerified
        signatureStatus = $signatureStates
        verifyPackage = "pass"
        install = "pass"
        coordinatorLifecycleDescribe = "pass"
        coordinatorLifecycleVerify = "pass"
        coordinatorLifecycleHealth = "pass"
        coordinatorLifecycleStartPlan = "pass"
        syntheticMcpConfigTransaction = "pass"
        uninstall = "pass"
        realProbe = "not-run"
        realDoctor = "not-run"
        defaultRegisterUserMcp = $false
        defaultStartBackgroundCoordinator = $false
        realDispatchEnabled = $false
        configScope = "synthetic"
        preExistingCompanionBackgroundProcesses = $preExistingBackground.Count
        remainingNewCompanionBackgroundProcesses = $newBackground.Count
        acceptanceRoot = $AcceptanceRoot
        acceptanceRootCleanup = if ($KeepAcceptanceRoot) { "kept" } else { "removed" }
    }
}
finally {
    if (-not $KeepAcceptanceRoot -and (Test-Path -LiteralPath $AcceptanceRoot)) {
        Assert-AcceptanceRoot -Path $AcceptanceRoot
        Remove-Item -LiteralPath $AcceptanceRoot -Recurse -Force
    }
}

if ($null -eq $report) {
    throw "External Windows acceptance did not produce a report."
}
if ($Json) {
    $report | ConvertTo-Json -Depth 8
}
else {
    $report
}
