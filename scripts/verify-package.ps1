[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$PackageRoot,
    [switch]$RequireSignature
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PackageRoot = [System.IO.Path]::GetFullPath($PackageRoot)
$PackageVersion = (Get-Content -LiteralPath (Join-Path $PackageRoot "VERSION") -Raw).Trim()
$ComponentBinaries = @(
    "CodexCCPanes.exe",
    "codex-ccpanes-coordinator.exe",
    "codex-ccpanes-mcp.exe"
)
$SupportBinaries = @(
    "codex-ccpanes.exe"
)
$PackagedBinaries = @($SupportBinaries + $ComponentBinaries)

function Assert-ChildPath {
    param([string]$Parent, [string]$Child)
    $parentFull = [System.IO.Path]::GetFullPath($Parent).TrimEnd('\') + '\'
    $childFull = [System.IO.Path]::GetFullPath($Child)
    if (-not $childFull.StartsWith($parentFull, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Path escapes package root: $childFull"
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

foreach ($required in @(
    "codex-ccpanes.exe",
    "CodexCCPanes.exe",
    "codex-ccpanes-coordinator.exe",
    "codex-ccpanes-mcp.exe",
    "Configure-CodexMcp.ps1",
    "Manage-Coordinator.ps1",
    "Test-ExternalWindowsAcceptance.ps1",
    "Install-CodexCCPanes.ps1",
    "Install-CodexCCPanes.cmd",
    "Uninstall-CodexCCPanes.ps1",
    "Uninstall-CodexCCPanes.cmd",
    "ARTIFACT_MANIFEST.json",
    "DEPENDENCIES.json",
    "THIRD_PARTY_LICENSES\MANIFEST.json",
    "EXTERNAL_WINDOWS_ACCEPTANCE.md",
    "README.md",
    "RELEASE_NOTES_0.2.0-beta.1.md",
    "THIRD_PARTY_NOTICES.md",
    "VERSION",
    "SHA256SUMS",
    "docs\EXTERNAL_WINDOWS_ACCEPTANCE.md",
    "docs\AUTHORITY.md",
    "docs\OPERATIONS.md",
    "docs\RELEASE_NOTES_0.2.0-beta.1.md"
)) {
    if (-not (Test-Path -LiteralPath (Join-Path $PackageRoot $required) -PathType Leaf)) {
        throw "Required package file missing: $required"
    }
}

$signatureStates = @{}
foreach ($binary in $PackagedBinaries) {
    $signature = Get-AuthenticodeSignature -LiteralPath (Join-Path $PackageRoot $binary)
    $signatureStates[$binary] = [string]$signature.Status
    if ($RequireSignature -and $signature.Status -ne "Valid") {
        throw "Required Authenticode signature is not valid for ${binary}: $($signature.Status)"
    }
}

$artifactManifest = Get-Content -LiteralPath (Join-Path $PackageRoot "ARTIFACT_MANIFEST.json") -Raw |
    ConvertFrom-Json
if ($artifactManifest.schemaVersion -ne 1 -or
    $artifactManifest.product -ne "CodexCCPanesCompanion" -or
    $artifactManifest.artifactKind -ne "coordination-plane-package" -or
    $artifactManifest.productVersion -ne $PackageVersion) {
    throw "Artifact manifest header is invalid."
}
$manifestComponents = @($artifactManifest.componentExecutables | ForEach-Object { $_.name } | Sort-Object)
$expectedComponents = @($ComponentBinaries | Sort-Object)
if (($manifestComponents -join "`n") -ne ($expectedComponents -join "`n")) {
    throw "Artifact manifest component executable inventory is invalid."
}
$manifestSupport = @($artifactManifest.supportExecutables | Sort-Object)
$expectedSupport = @($SupportBinaries | Sort-Object)
if (($manifestSupport -join "`n") -ne ($expectedSupport -join "`n")) {
    throw "Artifact manifest support executable inventory is invalid."
}
if ($artifactManifest.sideEffectBoundary.managesOfficialCodex -ne $false -or
    $artifactManifest.sideEffectBoundary.managesOfficialCcPanes -ne $false -or
    $artifactManifest.sideEffectBoundary.registersUserMcp -ne $false -or
    $artifactManifest.sideEffectBoundary.startsBackgroundCoordinator -ne $false -or
    $artifactManifest.sideEffectBoundary.checksumCoversPackagePayload -ne $true -or
    $artifactManifest.sideEffectBoundary.installedVersionWritesInstallManifest -ne $true) {
    throw "Artifact manifest side-effect boundary is invalid."
}
if ($artifactManifest.codexMcpConfiguration.transactionScript -ne "Configure-CodexMcp.ps1" -or
    $artifactManifest.codexMcpConfiguration.requiresExplicitConfigPath -ne $true -or
    $artifactManifest.codexMcpConfiguration.backupBeforeWrite -ne $true -or
    $artifactManifest.codexMcpConfiguration.rollbackRestoresBeforeSnapshot -ne $true -or
    $artifactManifest.codexMcpConfiguration.installerRunsByDefault -ne $false -or
    $artifactManifest.codexMcpConfiguration.installerSupportsExplicitApplyRepair -ne $true -or
    $artifactManifest.codexMcpConfiguration.uninstallerSupportsExplicitRemove -ne $true) {
    throw "Artifact manifest Codex MCP configuration contract is invalid."
}
if ($artifactManifest.coordinatorLifecycle.entryScript -ne "Manage-Coordinator.ps1" -or
    $artifactManifest.coordinatorLifecycle.installerRunsByDefault -ne $false -or
    $artifactManifest.coordinatorLifecycle.startsBackgroundCoordinator -ne $false -or
    $artifactManifest.coordinatorLifecycle.supportsDescribe -ne $true -or
    $artifactManifest.coordinatorLifecycle.supportsVerify -ne $true -or
    $artifactManifest.coordinatorLifecycle.supportsHealth -ne $true -or
    $artifactManifest.coordinatorLifecycle.supportsStartPlan -ne $true -or
    $artifactManifest.coordinatorLifecycle.healthRunsForeground -ne $true -or
    $artifactManifest.coordinatorLifecycle.defaultDataDirUnderInstallRoot -ne $true -or
    $artifactManifest.coordinatorLifecycle.defaultDatabaseUnderDataDir -ne $true) {
    throw "Artifact manifest Coordinator lifecycle contract is invalid."
}
if ($artifactManifest.externalWindowsAcceptance.contractVersion -ne "external-windows-acceptance/v1" -or
    $artifactManifest.externalWindowsAcceptance.packagePath -ne "EXTERNAL_WINDOWS_ACCEPTANCE.md" -or
    $artifactManifest.externalWindowsAcceptance.docsPath -ne "docs/EXTERNAL_WINDOWS_ACCEPTANCE.md" -or
    $artifactManifest.externalWindowsAcceptance.acceptanceRunner -ne "Test-ExternalWindowsAcceptance.ps1" -or
    $artifactManifest.externalWindowsAcceptance.requiresWindowsX64 -ne $true -or
    $artifactManifest.externalWindowsAcceptance.requiresExplicitInstallRoot -ne $true -or
    $artifactManifest.externalWindowsAcceptance.usesSyntheticCodexConfigOnly -ne $true -or
    $artifactManifest.externalWindowsAcceptance.allowsReadOnlyProbeDoctor -ne $true -or
    $artifactManifest.externalWindowsAcceptance.realDispatchEnabled -ne $false -or
    $artifactManifest.externalWindowsAcceptance.startsBackgroundCoordinator -ne $false -or
    $artifactManifest.externalWindowsAcceptance.runnerUsesWindowsPowerShell -ne $true) {
    throw "Artifact manifest external Windows acceptance contract is invalid."
}
if ($artifactManifest.betaReleaseNotes.contractVersion -ne "beta-release-notes/v1" -or
    $artifactManifest.betaReleaseNotes.targetVersion -ne "0.2.0-beta.1" -or
    $artifactManifest.betaReleaseNotes.packagePath -ne "RELEASE_NOTES_0.2.0-beta.1.md" -or
    $artifactManifest.betaReleaseNotes.docsPath -ne "docs/RELEASE_NOTES_0.2.0-beta.1.md" -or
    $artifactManifest.betaReleaseNotes.packageVersion -ne $PackageVersion -or
    $artifactManifest.betaReleaseNotes.statesNoDefaultMcpRegistration -ne $true -or
    $artifactManifest.betaReleaseNotes.statesNoBackgroundCoordinator -ne $true -or
    $artifactManifest.betaReleaseNotes.statesNoRealDispatch -ne $true -or
    $artifactManifest.betaReleaseNotes.statesUnsignedLocalBuildAllowed -ne $true -or
    $artifactManifest.betaReleaseNotes.statesProductionSignatureRequired -ne $true) {
    throw "Artifact manifest beta release notes contract is invalid."
}

$contractCopyPairs = @(
    [pscustomobject]@{ Root = "EXTERNAL_WINDOWS_ACCEPTANCE.md"; Docs = "docs\EXTERNAL_WINDOWS_ACCEPTANCE.md" },
    [pscustomobject]@{ Root = "RELEASE_NOTES_0.2.0-beta.1.md"; Docs = "docs\RELEASE_NOTES_0.2.0-beta.1.md" }
)
foreach ($copyPair in $contractCopyPairs) {
    $rootCopy = Join-Path $PackageRoot $copyPair.Root
    $docsCopy = Join-Path $PackageRoot $copyPair.Docs
    $rootHash = (Get-FileHash -LiteralPath $rootCopy -Algorithm SHA256).Hash
    $docsHash = (Get-FileHash -LiteralPath $docsCopy -Algorithm SHA256).Hash
    if ($rootHash -ne $docsHash) {
        throw "Package root copy differs from docs copy: $($copyPair.Root)"
    }
}

$externalAcceptanceText = Get-Content -LiteralPath (Join-Path $PackageRoot "EXTERNAL_WINDOWS_ACCEPTANCE.md") -Raw
foreach ($marker in @(
    "contract: external-windows-acceptance/v1",
    "acceptanceRunner: Test-ExternalWindowsAcceptance.ps1",
    "defaultRegisterUserMcp: false",
    "defaultStartBackgroundCoordinator: false",
    "configScope: synthetic",
    "realDispatchEnabled: false"
)) {
    if (-not $externalAcceptanceText.Contains($marker)) {
        throw "External Windows acceptance contract marker missing: $marker"
    }
}
$betaReleaseNotesText = Get-Content -LiteralPath (Join-Path $PackageRoot "RELEASE_NOTES_0.2.0-beta.1.md") -Raw
foreach ($marker in @(
    "contract: beta-release-notes/v1",
    "targetVersion: 0.2.0-beta.1",
    "defaultRegisterUserMcp: false",
    "defaultStartBackgroundCoordinator: false",
    "realDispatchEnabled: false",
    "productionSignatureRequired: true"
)) {
    if (-not $betaReleaseNotesText.Contains($marker)) {
        throw "Beta release notes contract marker missing: $marker"
    }
}

$dependencies = Get-Content -LiteralPath (Join-Path $PackageRoot "DEPENDENCIES.json") -Raw |
    ConvertFrom-Json
if ($dependencies.schemaVersion -ne 1 -or @($dependencies.dependencies).Count -lt 1) {
    throw "Dependency manifest is empty or invalid."
}

$licenseRoot = Join-Path $PackageRoot "THIRD_PARTY_LICENSES"
$licenseManifest = Get-Content -LiteralPath (Join-Path $licenseRoot "MANIFEST.json") -Raw |
    ConvertFrom-Json
if ($licenseManifest.schemaVersion -ne 1 -or
    $licenseManifest.productVersion -ne $PackageVersion -or
    @($licenseManifest.packages).Count -lt 1) {
    throw "Third-party license manifest is empty or invalid."
}
$dependencyKeys = [System.Collections.Generic.HashSet[string]]::new()
foreach ($dependency in @($dependencies.dependencies) |
    Where-Object { -not [string]::IsNullOrWhiteSpace([string]$_.source) }) {
    [void]$dependencyKeys.Add("$($dependency.name)@$($dependency.version)")
}
$licensedKeys = [System.Collections.Generic.HashSet[string]]::new()
$licensedFiles = [System.Collections.Generic.HashSet[string]]::new()
foreach ($licensed in @($licenseManifest.packages)) {
    $key = "$($licensed.name)@$($licensed.version)"
    if (-not $licensedKeys.Add($key)) {
        throw "Duplicate third-party license entry: $key"
    }
    if ([string]::IsNullOrWhiteSpace([string]$licensed.license) -or
        [string]::IsNullOrWhiteSpace([string]$licensed.directory)) {
        throw "Incomplete third-party license entry: $key"
    }
    foreach ($licenseFile in @($licensed.files)) {
        $relative = Join-Path (Join-Path "THIRD_PARTY_LICENSES" $licensed.directory) ([string]$licenseFile)
        if ([System.IO.Path]::IsPathRooted($relative) -or $relative.Contains("..")) {
            throw "Unsafe third-party license path: $relative"
        }
        $fullPath = Join-Path $PackageRoot $relative
        if (-not (Test-Path -LiteralPath $fullPath -PathType Leaf)) {
            throw "Third-party license text missing: $relative"
        }
        if (-not $licensedFiles.Add($relative)) {
            throw "Duplicate third-party license file: $relative"
        }
    }
    if (@($licensed.files).Count -lt 1) {
        throw "Third-party license entry has no text files: $key"
    }
}
if ($dependencyKeys.Count -ne $licensedKeys.Count -or
    @($dependencyKeys | Where-Object { -not $licensedKeys.Contains($_) }).Count -gt 0 -or
    @($licensedKeys | Where-Object { -not $dependencyKeys.Contains($_) }).Count -gt 0) {
    throw "Third-party license manifest does not match external dependencies."
}

$expected = @{}
foreach ($line in Get-Content -LiteralPath (Join-Path $PackageRoot "SHA256SUMS")) {
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

$packageFiles = @(
    Get-ChildItem -LiteralPath $PackageRoot -Recurse -File |
        Where-Object { $_.Name -ne "SHA256SUMS" }
)
foreach ($file in $packageFiles) {
    $relative = Convert-ToRelativePath -Root $PackageRoot -Path $file.FullName
    if (-not $expected.ContainsKey($relative)) {
        throw "Package file is not covered by SHA256SUMS: $relative"
    }
}
if ($packageFiles.Count -ne $expected.Count) {
    throw "SHA256SUMS file count does not match the package payload."
}
foreach ($entry in $expected.GetEnumerator()) {
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

$descriptorJson = & (Join-Path $PackageRoot "codex-ccpanes.exe") describe --json
if ($LASTEXITCODE -ne 0) {
    throw "Packaged descriptor command failed with exit code $LASTEXITCODE"
}
$descriptor = $descriptorJson | ConvertFrom-Json
if ($descriptor.liveagentRuntimeDependency -ne $false) {
    throw "Descriptor unexpectedly depends on LiveAgent runtime."
}
if ($descriptor.upstreamPolicy -ne "official-external-binaries") {
    throw "Unexpected upstream policy."
}

$coordinatorVersion = & (Join-Path $PackageRoot "codex-ccpanes-coordinator.exe") --version
if ($LASTEXITCODE -ne 0 -or $coordinatorVersion -notmatch '^codex-ccpanes-coordinator\s+') {
    throw "Coordinator packaged version contract failed."
}
$mcpDescriptorJson = & (Join-Path $PackageRoot "codex-ccpanes-mcp.exe") --describe
if ($LASTEXITCODE -ne 0) {
    throw "MCP packaged descriptor command failed with exit code $LASTEXITCODE"
}
$mcpDescriptor = $mcpDescriptorJson | ConvertFrom-Json
if ($mcpDescriptor.name -ne "codex-ccpanes-mcp" -or
    [string]::IsNullOrWhiteSpace($mcpDescriptor.mcpSchemaVersion) -or
    [string]::IsNullOrWhiteSpace($mcpDescriptor.ipcSchemaVersion)) {
    throw "MCP packaged descriptor contract failed."
}

[pscustomobject]@{
    PackageRoot = $PackageRoot
    Version = $PackageVersion
    FilesVerified = $expected.Count
    Descriptor = "pass"
    ComponentInventory = "pass"
    CodexMcpConfigContract = "pass"
    CoordinatorLifecycle = "pass"
    ExternalWindowsAcceptance = "pass"
    BetaReleaseNotes = "pass"
    ProcessContracts = "pass"
    Dependencies = @($dependencies.dependencies).Count
    Signatures = ($signatureStates.GetEnumerator() |
        ForEach-Object { "$($_.Key)=$($_.Value)" }) -join ","
}
