[CmdletBinding()]
param(
    [string]$InstallRoot = (Join-Path $env:LOCALAPPDATA "CodexPaneForge"),
    [switch]$PurgeData,
    [switch]$SkipShortcuts,
    [switch]$SkipRegistry,
    [switch]$Quiet,
    [switch]$RemoveCodexMcp,
    [string]$CodexConfigPath,
    [string]$CodexMcpRegistrationName = "codex_paneforge"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ProductId = "CodexPaneForgeCompanion"
$InstallRoot = [System.IO.Path]::GetFullPath($InstallRoot)

function Write-Step {
    param([string]$Message)
    if (-not $Quiet) {
        Write-Host "[Codex + PaneForge] $Message"
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
        throw "Path escapes install root: $childFull"
    }
}

$statePath = Join-Path $InstallRoot "install-state.json"
$state = $null
if (Test-Path -LiteralPath $statePath -PathType Leaf) {
    $state = Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json
    if ($state.productId -ne $ProductId) {
        throw "Install state does not belong to $ProductId"
    }
}

if (-not $RemoveCodexMcp -and -not [string]::IsNullOrWhiteSpace($CodexConfigPath)) {
    throw "CodexConfigPath requires -RemoveCodexMcp."
}
if ($RemoveCodexMcp) {
    if ([string]::IsNullOrWhiteSpace($CodexConfigPath)) {
        throw "CodexConfigPath is required when -RemoveCodexMcp is set."
    }
    if ($CodexMcpRegistrationName -notmatch '^[A-Za-z0-9_-]+$') {
        throw "Invalid MCP registration name: $CodexMcpRegistrationName"
    }
    if ($null -eq $state) {
        throw "Install state is required to remove the managed Codex MCP registration."
    }
    $configManager = Join-Path ([string]$state.currentVersionRoot) "Configure-CodexMcp.ps1"
    if (-not (Test-Path -LiteralPath $configManager -PathType Leaf)) {
        throw "Installed Codex MCP configuration manager missing: $configManager"
    }
    $codexConfigPathFull = [System.IO.Path]::GetFullPath($CodexConfigPath)
    & $configManager -Action Remove -ConfigPath $codexConfigPathFull -InstallRoot $InstallRoot `
        -RegistrationName $CodexMcpRegistrationName -Json -Quiet | Out-Null
}

if (-not $SkipShortcuts -and $null -ne $state) {
    foreach ($shortcut in @($state.shortcutPaths)) {
        if (-not [string]::IsNullOrWhiteSpace($shortcut) -and (Test-Path -LiteralPath $shortcut -PathType Leaf)) {
            Remove-Item -LiteralPath $shortcut -Force
        }
    }
}

if (-not $SkipRegistry) {
    $uninstallKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$ProductId"
    if (Test-Path -LiteralPath $uninstallKey) {
        Remove-Item -LiteralPath $uninstallKey -Recurse -Force
    }
}

$versionsRoot = Join-Path $InstallRoot "versions"
if (Test-Path -LiteralPath $versionsRoot -PathType Container) {
    Assert-ChildPath -Parent $InstallRoot -Child $versionsRoot
    foreach ($directory in Get-ChildItem -LiteralPath $versionsRoot -Directory -Force) {
        Assert-ChildPath -Parent $versionsRoot -Child $directory.FullName
        $manifestPath = Join-Path $directory.FullName "install-manifest.json"
        if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
            Write-Step "Preserving unmanaged directory: $($directory.FullName)"
            continue
        }
        $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
        if ($manifest.productId -ne $ProductId) {
            Write-Step "Preserving directory owned by another product: $($directory.FullName)"
            continue
        }
        Remove-Item -LiteralPath $directory.FullName -Recurse -Force
    }
    if (-not (Get-ChildItem -LiteralPath $versionsRoot -Force | Select-Object -First 1)) {
        Remove-Item -LiteralPath $versionsRoot -Force
    }
}

foreach ($file in @(
    "current.json",
    "install-state.json",
    "Manage-Coordinator.ps1",
    "Uninstall-CodexPaneForge.ps1"
)) {
    $path = Join-Path $InstallRoot $file
    Assert-ChildPath -Parent $InstallRoot -Child $path
    if (Test-Path -LiteralPath $path -PathType Leaf) {
        Remove-Item -LiteralPath $path -Force
    }
}

if ($PurgeData -and (Test-Path -LiteralPath $InstallRoot -PathType Container)) {
    $root = [System.IO.Path]::GetPathRoot($InstallRoot)
    if ($InstallRoot.TrimEnd('\') -eq $root.TrimEnd('\')) {
        throw "Refusing to purge a drive root."
    }
    Remove-Item -LiteralPath $InstallRoot -Recurse -Force
}

Write-Step "Uninstall complete; Codex and official PaneForge were not modified"
