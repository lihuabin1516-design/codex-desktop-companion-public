[CmdletBinding()]
param(
    [string]$InstallRoot = (Join-Path $env:LOCALAPPDATA "CodexPaneForge"),
    [switch]$DesktopShortcut,
    [switch]$SkipShortcuts,
    [switch]$SkipRegistry,
    [switch]$Quiet,
    [switch]$ConfigureCodexMcp,
    [string]$CodexConfigPath,
    [string]$CodexMcpRegistrationName = "codex_paneforge"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ProductId = "CodexPaneForgeCompanion"
$PackageRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSCommandPath))
$InstallRoot = [System.IO.Path]::GetFullPath($InstallRoot)
$VersionFile = Join-Path $PackageRoot "VERSION"
$ChecksumsFile = Join-Path $PackageRoot "SHA256SUMS"

function Write-Step {
    param([string]$Message)
    if (-not $Quiet) {
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
        throw "Path escapes install root: $childFull"
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
        throw "Path escapes root: $pathFull"
    }
    return $pathFull.Substring($rootFull.Length)
}

function Write-JsonAtomic {
    param(
        [string]$Path,
        [object]$Value
    )
    $parent = Split-Path -Parent $Path
    New-Item -ItemType Directory -Force -Path $parent | Out-Null
    $temporary = Join-Path $parent (".{0}.tmp-{1}" -f (Split-Path -Leaf $Path), [guid]::NewGuid().ToString("N"))
    $json = $Value | ConvertTo-Json -Depth 8
    $encoding = New-Object System.Text.UTF8Encoding -ArgumentList $false
    [System.IO.File]::WriteAllText($temporary, $json, $encoding)
    Move-Item -LiteralPath $temporary -Destination $Path -Force
}

function Read-ExpectedHashes {
    param([string]$Path)
    $hashes = @{}
    foreach ($line in Get-Content -LiteralPath $Path) {
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
        if ($hashes.ContainsKey($relative)) {
            throw "Duplicate SHA256SUMS path: $relative"
        }
        $hashes[$relative] = $Matches[1].ToUpperInvariant()
    }
    return $hashes
}

function New-Shortcut {
    param(
        [string]$Path,
        [string]$Target,
        [string]$WorkingDirectory,
        [string]$Description
    )
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($Path)
    $shortcut.TargetPath = $Target
    $shortcut.WorkingDirectory = $WorkingDirectory
    $shortcut.Description = $Description
    $shortcut.IconLocation = "$Target,0"
    $shortcut.Save()
}

function Assert-ManifestMatch {
    param(
        [object]$Expected,
        [object]$Actual
    )
    $expectedMap = @{}
    foreach ($file in @($Expected.files)) {
        $expectedMap[$file.path] = $file.sha256
    }
    $actualMap = @{}
    foreach ($file in @($Actual.files)) {
        $actualMap[$file.path] = $file.sha256
    }
    if ($expectedMap.Count -ne $actualMap.Count) {
        throw "Existing version payload differs from this package."
    }
    foreach ($entry in $expectedMap.GetEnumerator()) {
        if (-not $actualMap.ContainsKey($entry.Key) -or $actualMap[$entry.Key] -ne $entry.Value) {
            throw "Existing version payload differs at $($entry.Key)."
        }
    }
}

Assert-SafeInstallRoot -Path $InstallRoot
if (-not $ConfigureCodexMcp -and -not [string]::IsNullOrWhiteSpace($CodexConfigPath)) {
    throw "CodexConfigPath requires -ConfigureCodexMcp."
}
if ($ConfigureCodexMcp -and [string]::IsNullOrWhiteSpace($CodexConfigPath)) {
    throw "CodexConfigPath is required when -ConfigureCodexMcp is set."
}
if ($CodexMcpRegistrationName -notmatch '^[A-Za-z0-9_-]+$') {
    throw "Invalid MCP registration name: $CodexMcpRegistrationName"
}

foreach ($required in @(
    "VERSION",
    "SHA256SUMS",
    "codex-paneforge.exe",
    "CodexPaneForge.exe",
    "codex-paneforge-coordinator.exe",
    "codex-paneforge-mcp.exe",
    "ARTIFACT_MANIFEST.json",
    "Configure-CodexMcp.ps1",
    "EXTERNAL_WINDOWS_ACCEPTANCE.md",
    "Manage-Coordinator.ps1",
    "RELEASE_NOTES_0.2.0-beta.1.md",
    "Uninstall-CodexPaneForge.ps1"
)) {
    $path = Join-Path $PackageRoot $required
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Package file missing: $required"
    }
}

$Version = (Get-Content -LiteralPath $VersionFile -Raw).Trim()
if ($Version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?$') {
    throw "Invalid package version: $Version"
}

$ExpectedHashes = Read-ExpectedHashes -Path $ChecksumsFile
if ($ExpectedHashes.Count -lt 1) {
    throw "SHA256SUMS is empty."
}

$packageFiles = @(
    Get-ChildItem -LiteralPath $PackageRoot -Recurse -File |
        Where-Object { $_.Name -ne "SHA256SUMS" }
)
foreach ($file in $packageFiles) {
    $relative = Convert-ToRelativePath -Root $PackageRoot -Path $file.FullName
    if (-not $ExpectedHashes.ContainsKey($relative)) {
        throw "Package file is not covered by SHA256SUMS: $relative"
    }
}
if ($packageFiles.Count -ne $ExpectedHashes.Count) {
    throw "SHA256SUMS file count does not match the package payload."
}

foreach ($entry in $ExpectedHashes.GetEnumerator()) {
    $path = Join-Path $PackageRoot $entry.Key
    Assert-ChildPath -Parent $PackageRoot -Child $path
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Checksum entry missing file: $($entry.Key)"
    }
    $actual = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash
    if ($actual -ne $entry.Value) {
        throw "Checksum mismatch: $($entry.Key)"
    }
}

$VersionsRoot = Join-Path $InstallRoot "versions"
$VersionRoot = Join-Path $VersionsRoot $Version
$StagingRoot = Join-Path $InstallRoot (".staging-" + [guid]::NewGuid().ToString("N"))
$ReusedVersionRoot = $false
Assert-ChildPath -Parent $InstallRoot -Child $VersionsRoot
Assert-ChildPath -Parent $InstallRoot -Child $VersionRoot
Assert-ChildPath -Parent $InstallRoot -Child $StagingRoot

New-Item -ItemType Directory -Force -Path $InstallRoot, $VersionsRoot | Out-Null
New-Item -ItemType Directory -Path $StagingRoot | Out-Null

try {
    Write-Step "Verifying and staging $Version"
    $payload = @(
        "codex-paneforge.exe",
        "CodexPaneForge.exe",
        "codex-paneforge-coordinator.exe",
        "codex-paneforge-mcp.exe",
        "ARTIFACT_MANIFEST.json",
        "Configure-CodexMcp.ps1",
        "Manage-Coordinator.ps1",
        "DEPENDENCIES.json",
        "EXTERNAL_WINDOWS_ACCEPTANCE.md",
        "README.md",
        "RELEASE_NOTES_0.2.0-beta.1.md",
        "THIRD_PARTY_NOTICES.md",
        "VERSION",
        "SHA256SUMS"
    )
    foreach ($name in $payload) {
        $source = Join-Path $PackageRoot $name
        if (Test-Path -LiteralPath $source -PathType Leaf) {
            Copy-Item -LiteralPath $source -Destination (Join-Path $StagingRoot $name)
        }
    }
    $docsSource = Join-Path $PackageRoot "docs"
    if (Test-Path -LiteralPath $docsSource -PathType Container) {
        Copy-Item -LiteralPath $docsSource -Destination (Join-Path $StagingRoot "docs") -Recurse
    }

    $installedFiles = @()
    foreach ($file in Get-ChildItem -LiteralPath $StagingRoot -Recurse -File) {
        $relative = Convert-ToRelativePath -Root $StagingRoot -Path $file.FullName
        $installedFiles += [ordered]@{
            path = $relative.Replace('\', '/')
            bytes = $file.Length
            sha256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash
        }
    }
    $versionManifest = [ordered]@{
        schemaVersion = 1
        productId = $ProductId
        version = $Version
        files = $installedFiles
    }
    Write-JsonAtomic -Path (Join-Path $StagingRoot "install-manifest.json") -Value $versionManifest

    if (Test-Path -LiteralPath $VersionRoot) {
        $existingManifest = Join-Path $VersionRoot "install-manifest.json"
        if (-not (Test-Path -LiteralPath $existingManifest -PathType Leaf)) {
            throw "Existing version directory is not managed by this installer: $VersionRoot"
        }
        $existing = Get-Content -LiteralPath $existingManifest -Raw | ConvertFrom-Json
        if ($existing.productId -ne $ProductId -or $existing.version -ne $Version) {
            throw "Existing version manifest does not match package."
        }
        Assert-ManifestMatch -Expected $versionManifest -Actual $existing
        Write-Step "Version $Version already exists; reusing verified directory"
        $ReusedVersionRoot = $true
        Remove-Item -LiteralPath $StagingRoot -Recurse -Force
    }
    else {
        Move-Item -LiteralPath $StagingRoot -Destination $VersionRoot
    }

    Copy-Item -LiteralPath (Join-Path $PackageRoot "Uninstall-CodexPaneForge.ps1") `
        -Destination (Join-Path $InstallRoot "Uninstall-CodexPaneForge.ps1") -Force
    Copy-Item -LiteralPath (Join-Path $PackageRoot "Manage-Coordinator.ps1") `
        -Destination (Join-Path $InstallRoot "Manage-Coordinator.ps1") -Force

    $guiExecutable = Join-Path $VersionRoot "CodexPaneForge.exe"
    $cliExecutable = Join-Path $VersionRoot "codex-paneforge.exe"
    $coordinatorExecutable = Join-Path $VersionRoot "codex-paneforge-coordinator.exe"
    $mcpExecutable = Join-Path $VersionRoot "codex-paneforge-mcp.exe"
    $coordinatorLifecycleScript = Join-Path $InstallRoot "Manage-Coordinator.ps1"
    foreach ($file in @($guiExecutable, $cliExecutable, $coordinatorExecutable, $mcpExecutable)) {
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) {
            throw "Installed executable missing: $file"
        }
    }

    $codexMcpConfiguration = [ordered]@{
        configuredThisRun = $false
        installerRunsByDefault = $false
    }
    if ($ConfigureCodexMcp) {
        $configManager = Join-Path $VersionRoot "Configure-CodexMcp.ps1"
        if (-not (Test-Path -LiteralPath $configManager -PathType Leaf)) {
            throw "Installed Codex MCP configuration manager missing: $configManager"
        }
        $codexConfigPathFull = [System.IO.Path]::GetFullPath($CodexConfigPath)
        $configAction = if ($ReusedVersionRoot) { "Repair" } else { "Apply" }
        $configJson = & $configManager -Action $configAction -ConfigPath $codexConfigPathFull `
            -InstallRoot $InstallRoot -McpExecutable $mcpExecutable `
            -RegistrationName $CodexMcpRegistrationName -Json -Quiet
        $configReport = $configJson | ConvertFrom-Json
        if ($configReport.ok -ne $true -or [string]::IsNullOrWhiteSpace([string]$configReport.transactionPath)) {
            throw "Codex MCP configuration $configAction did not produce a valid transaction."
        }
        $codexMcpConfiguration = [ordered]@{
            configuredThisRun = $true
            installerRunsByDefault = $false
            action = $configAction.ToLowerInvariant()
            registrationName = $CodexMcpRegistrationName
            configPath = $codexConfigPathFull
            transactionPath = [string]$configReport.transactionPath
        }
    }

    $shortcutPaths = @()
    if (-not $SkipShortcuts) {
        $startMenu = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs"
        New-Item -ItemType Directory -Force -Path $startMenu | Out-Null
        $startShortcut = Join-Path $startMenu "Codex + PaneForge.lnk"
        New-Shortcut -Path $startShortcut -Target $guiExecutable `
            -WorkingDirectory $VersionRoot -Description "Safely launch official PaneForge for Codex"
        $shortcutPaths += $startShortcut
        if ($DesktopShortcut) {
            $desktop = [Environment]::GetFolderPath("Desktop")
            $desktopLink = Join-Path $desktop "Codex + PaneForge.lnk"
            New-Shortcut -Path $desktopLink -Target $guiExecutable `
                -WorkingDirectory $VersionRoot -Description "Safely launch official PaneForge for Codex"
            $shortcutPaths += $desktopLink
        }
    }

    if (-not $SkipRegistry) {
        $uninstallKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$ProductId"
        New-Item -Path $uninstallKey -Force | Out-Null
        $uninstallScript = Join-Path $InstallRoot "Uninstall-CodexPaneForge.ps1"
        $uninstallCommand = "powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File `"$uninstallScript`""
        New-ItemProperty -Path $uninstallKey -Name DisplayName -Value "Codex + PaneForge Companion" -Force | Out-Null
        New-ItemProperty -Path $uninstallKey -Name DisplayVersion -Value $Version -Force | Out-Null
        New-ItemProperty -Path $uninstallKey -Name Publisher -Value "Codex + PaneForge Companion" -Force | Out-Null
        New-ItemProperty -Path $uninstallKey -Name DisplayIcon -Value $guiExecutable -Force | Out-Null
        New-ItemProperty -Path $uninstallKey -Name UninstallString -Value $uninstallCommand -Force | Out-Null
        New-ItemProperty -Path $uninstallKey -Name NoModify -PropertyType DWord -Value 1 -Force | Out-Null
        New-ItemProperty -Path $uninstallKey -Name NoRepair -PropertyType DWord -Value 1 -Force | Out-Null
    }

    $state = [ordered]@{
        schemaVersion = 1
        productId = $ProductId
        currentVersion = $Version
        currentVersionRoot = $VersionRoot
        cliExecutable = $cliExecutable
        guiExecutable = $guiExecutable
        coordinatorExecutable = $coordinatorExecutable
        mcpExecutable = $mcpExecutable
        coordinatorLifecycleScript = $coordinatorLifecycleScript
        versionCoordinatorLifecycleScript = (Join-Path $VersionRoot "Manage-Coordinator.ps1")
        codexMcpConfiguration = $codexMcpConfiguration
        shortcutPaths = $shortcutPaths
        installedAtUtc = [DateTime]::UtcNow.ToString("o")
    }
    Write-JsonAtomic -Path (Join-Path $InstallRoot "install-state.json") -Value $state
    Write-JsonAtomic -Path (Join-Path $InstallRoot "current.json") -Value @{
        schemaVersion = 1
        version = $Version
        executable = $guiExecutable
        cliExecutable = $cliExecutable
        coordinatorExecutable = $coordinatorExecutable
        mcpExecutable = $mcpExecutable
        coordinatorLifecycleScript = $coordinatorLifecycleScript
    }

    Write-Step "Install complete: $Version"
    [pscustomobject]@{
        ProductId = $ProductId
        Version = $Version
        InstallRoot = $InstallRoot
        CliExecutable = $cliExecutable
        GuiExecutable = $guiExecutable
        CoordinatorExecutable = $coordinatorExecutable
        McpExecutable = $mcpExecutable
    }
}
catch {
    if (Test-Path -LiteralPath $StagingRoot) {
        Assert-ChildPath -Parent $InstallRoot -Child $StagingRoot
        Remove-Item -LiteralPath $StagingRoot -Recurse -Force
    }
    throw
}
