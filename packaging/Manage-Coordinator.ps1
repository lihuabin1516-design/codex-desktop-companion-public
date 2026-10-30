[CmdletBinding()]
param(
    [ValidateSet("Describe", "Verify", "Health", "StartPlan")]
    [string]$Action = "Describe",
    [string]$InstallRoot = (Join-Path $env:LOCALAPPDATA "CodexPaneForge"),
    [string]$CoordinatorExecutable,
    [string]$DataDir,
    [string]$DatabasePath,
    [string]$IpcEndpoint = "codex-paneforge-coordinator-v1",
    [switch]$Json,
    [switch]$Quiet
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ProductId = "CodexPaneForgeCompanion"
$ScriptName = "Manage-Coordinator.ps1"

function Write-Step {
    param([string]$Message)
    if (-not $Quiet -and -not $Json) {
        Write-Host "[Codex + PaneForge] $Message"
    }
}

function Assert-SafeInstallRoot {
    param([string]$Path)
    $root = [System.IO.Path]::GetPathRoot($Path)
    if ([string]::IsNullOrWhiteSpace($root) -or $Path.TrimEnd('\') -eq $root.TrimEnd('\')) {
        throw "InstallRoot must not be a drive root: $Path"
    }
    if ($Path -eq [Environment]::GetFolderPath("UserProfile")) {
        throw "InstallRoot must not be the user profile root."
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

function Resolve-CoordinatorExecutable {
    param(
        [string]$Root,
        [string]$ExplicitExecutable
    )
    if (-not [string]::IsNullOrWhiteSpace($ExplicitExecutable)) {
        return [System.IO.Path]::GetFullPath($ExplicitExecutable)
    }
    $statePath = Join-Path $Root "install-state.json"
    if (Test-Path -LiteralPath $statePath -PathType Leaf) {
        $state = Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json
        if ($state.productId -ne $ProductId) {
            throw "Install state does not belong to $ProductId"
        }
        if (-not [string]::IsNullOrWhiteSpace([string]$state.coordinatorExecutable)) {
            return [System.IO.Path]::GetFullPath([string]$state.coordinatorExecutable)
        }
    }
    $currentPath = Join-Path $Root "current.json"
    if (Test-Path -LiteralPath $currentPath -PathType Leaf) {
        $current = Get-Content -LiteralPath $currentPath -Raw | ConvertFrom-Json
        if (-not [string]::IsNullOrWhiteSpace([string]$current.coordinatorExecutable)) {
            return [System.IO.Path]::GetFullPath([string]$current.coordinatorExecutable)
        }
    }
    throw "Coordinator executable is not registered; pass -CoordinatorExecutable or install first."
}

function Resolve-LifecyclePaths {
    param(
        [string]$Root,
        [string]$ExplicitDataDir,
        [string]$ExplicitDatabasePath
    )
    $resolvedDataDir = if ([string]::IsNullOrWhiteSpace($ExplicitDataDir)) {
        Join-Path $Root "data\coordinator"
    }
    else {
        $ExplicitDataDir
    }
    $resolvedDataDir = [System.IO.Path]::GetFullPath($resolvedDataDir)
    Assert-ChildPath -Parent $Root -Child $resolvedDataDir

    $resolvedDatabasePath = if ([string]::IsNullOrWhiteSpace($ExplicitDatabasePath)) {
        Join-Path $resolvedDataDir "coordination.db"
    }
    else {
        $ExplicitDatabasePath
    }
    $resolvedDatabasePath = [System.IO.Path]::GetFullPath($resolvedDatabasePath)
    Assert-ChildPath -Parent $resolvedDataDir -Child $resolvedDatabasePath

    return [ordered]@{
        dataDir = $resolvedDataDir
        databasePath = $resolvedDatabasePath
    }
}

function Invoke-CoordinatorVersion {
    param([string]$Executable)
    $version = & $Executable --version
    if ($LASTEXITCODE -ne 0) {
        throw "Coordinator version command failed with exit code $LASTEXITCODE"
    }
    if ($version -notmatch '^codex-paneforge-coordinator\s+') {
        throw "Coordinator version contract failed: $version"
    }
    return [string]$version
}

function Invoke-CoordinatorHealth {
    param(
        [string]$Executable,
        [string]$ResolvedDataDir,
        [string]$ResolvedDatabasePath,
        [string]$ResolvedIpcEndpoint
    )
    New-Item -ItemType Directory -Force -Path $ResolvedDataDir | Out-Null
    $healthLines = & $Executable --data-dir $ResolvedDataDir --database $ResolvedDatabasePath `
        --ipc-endpoint $ResolvedIpcEndpoint
    if ($LASTEXITCODE -ne 0) {
        throw "Coordinator health command failed with exit code $LASTEXITCODE"
    }
    $healthText = ($healthLines | Out-String).Trim()
    $health = $healthText | ConvertFrom-Json
    if ([string]::IsNullOrWhiteSpace([string]$health.schemaVersion) -or
        [string]::IsNullOrWhiteSpace([string]$health.protocolVersion) -or
        [string]::IsNullOrWhiteSpace([string]$health.runtimeIdentity)) {
        throw "Coordinator health contract is incomplete."
    }
    return $health
}

function New-StartPlan {
    param(
        [string]$Executable,
        [string]$ResolvedDataDir,
        [string]$ResolvedDatabasePath,
        [string]$ResolvedIpcEndpoint
    )
    return [ordered]@{
        command = $Executable
        arguments = @(
            "--stdio-ipc",
            "--data-dir", $ResolvedDataDir,
            "--database", $ResolvedDatabasePath,
            "--ipc-endpoint", $ResolvedIpcEndpoint
        )
        workingDirectory = $ResolvedDataDir
        dataDir = $ResolvedDataDir
        databasePath = $ResolvedDatabasePath
        ipcEndpoint = $ResolvedIpcEndpoint
        mode = "foreground-stdio-ipc"
        startsProcess = $false
    }
}

function Complete-Report {
    param([object]$Report)
    if ($Json) {
        $Report | ConvertTo-Json -Depth 8
    }
    else {
        $status = if ($Report.ok) { "pass" } else { "fail" }
        Write-Step "$($Report.action): $status"
        $Report
    }
}

$InstallRoot = [System.IO.Path]::GetFullPath($InstallRoot)
Assert-SafeInstallRoot -Path $InstallRoot
if ($IpcEndpoint -notmatch '^[A-Za-z0-9_.:-]+$') {
    throw "Invalid IPC endpoint name: $IpcEndpoint"
}

$resolvedExecutable = Resolve-CoordinatorExecutable -Root $InstallRoot -ExplicitExecutable $CoordinatorExecutable
if (-not (Test-Path -LiteralPath $resolvedExecutable -PathType Leaf)) {
    throw "Coordinator executable is missing: $resolvedExecutable"
}
$paths = Resolve-LifecyclePaths -Root $InstallRoot -ExplicitDataDir $DataDir -ExplicitDatabasePath $DatabasePath
$startPlan = New-StartPlan -Executable $resolvedExecutable -ResolvedDataDir $paths.dataDir `
    -ResolvedDatabasePath $paths.databasePath -ResolvedIpcEndpoint $IpcEndpoint

if ($Action -eq "Describe") {
    Complete-Report -Report ([ordered]@{
        schemaVersion = 1
        productId = $ProductId
        action = "describe"
        ok = $true
        lifecycleScript = $ScriptName
        coordinatorExecutable = $resolvedExecutable
        coordinatorExecutableSnapshot = Get-FileSnapshot -Path $resolvedExecutable
        defaultStartByInstaller = $false
        startsBackgroundCoordinator = $false
        supportedActions = @("Describe", "Verify", "Health", "StartPlan")
        startPlan = $startPlan
    })
    return
}

if ($Action -eq "Verify") {
    $version = Invoke-CoordinatorVersion -Executable $resolvedExecutable
    Complete-Report -Report ([ordered]@{
        schemaVersion = 1
        productId = $ProductId
        action = "verify"
        ok = $true
        coordinatorExecutable = $resolvedExecutable
        version = $version
        defaultStartByInstaller = $false
        startsBackgroundCoordinator = $false
        startPlan = $startPlan
    })
    return
}

if ($Action -eq "Health") {
    $health = Invoke-CoordinatorHealth -Executable $resolvedExecutable -ResolvedDataDir $paths.dataDir `
        -ResolvedDatabasePath $paths.databasePath -ResolvedIpcEndpoint $IpcEndpoint
    Complete-Report -Report ([ordered]@{
        schemaVersion = 1
        productId = $ProductId
        action = "health"
        ok = $true
        defaultStartByInstaller = $false
        startsBackgroundCoordinator = $false
        health = $health
        database = Get-FileSnapshot -Path $paths.databasePath
        startPlan = $startPlan
    })
    return
}

if ($Action -eq "StartPlan") {
    Complete-Report -Report ([ordered]@{
        schemaVersion = 1
        productId = $ProductId
        action = "startPlan"
        ok = $true
        defaultStartByInstaller = $false
        startsBackgroundCoordinator = $false
        startPlan = $startPlan
    })
    return
}
