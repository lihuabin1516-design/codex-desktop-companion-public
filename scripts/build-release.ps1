[CmdletBinding()]
param(
    [string]$CargoTargetDir = $env:CARGO_TARGET_DIR,
    [string]$OutputRoot,
    [string]$SignToolPath = $env:SIGNTOOL_PATH,
    [string]$CertificateThumbprint = $env:CODEX_CCPANES_SIGNING_CERT_THUMBPRINT,
    [string]$TimestampUrl = "http://timestamp.digicert.com",
    [switch]$RequireSignature
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ProductRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
if ([string]::IsNullOrWhiteSpace($CargoTargetDir)) {
    $CargoTargetDir = Join-Path ([System.IO.Path]::GetTempPath()) "codex-ccpanes-cargo-target"
}
$CargoTargetDir = [System.IO.Path]::GetFullPath($CargoTargetDir)
if ([string]::IsNullOrWhiteSpace($OutputRoot)) {
    $OutputRoot = Join-Path $ProductRoot "dist"
}
$OutputRoot = [System.IO.Path]::GetFullPath($OutputRoot)

$metadataJson = & cargo metadata --manifest-path (Join-Path $ProductRoot "Cargo.toml") `
    --locked --format-version 1
if ($LASTEXITCODE -ne 0) {
    throw "cargo metadata failed with exit code $LASTEXITCODE"
}
$metadata = $metadataJson | ConvertFrom-Json
$package = $metadata.packages | Where-Object { $_.name -eq "codex-ccpanes" } | Select-Object -First 1
if ($null -eq $package) {
    throw "codex-ccpanes package metadata not found"
}
$Version = $package.version
$PackageName = "CodexCCPanes-$Version-win-x64"
$PackageRoot = Join-Path $OutputRoot $PackageName
$ZipPath = Join-Path $OutputRoot "$PackageName.zip"
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
        throw "Path escapes output root: $childFull"
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

function Write-Utf8NoBomText {
    param(
        [string]$Path,
        [string]$Text
    )
    $encoding = New-Object System.Text.UTF8Encoding -ArgumentList $false
    [System.IO.File]::WriteAllText($Path, $Text, $encoding)
}

New-Item -ItemType Directory -Force -Path $OutputRoot | Out-Null
Assert-ChildPath -Parent $OutputRoot -Child $PackageRoot
Assert-ChildPath -Parent $OutputRoot -Child $ZipPath
if (Test-Path -LiteralPath $PackageRoot) {
    Remove-Item -LiteralPath $PackageRoot -Recurse -Force
}
if (Test-Path -LiteralPath $ZipPath) {
    Remove-Item -LiteralPath $ZipPath -Force
}

$env:CARGO_TARGET_DIR = $CargoTargetDir
Push-Location $ProductRoot
try {
    & cargo build --release --locked --workspace --bins
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit code $LASTEXITCODE"
    }
}
finally {
    Pop-Location
}

New-Item -ItemType Directory -Path $PackageRoot | Out-Null
foreach ($binary in $PackagedBinaries) {
    $source = Join-Path $CargoTargetDir "release\$binary"
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
        throw "Release binary missing: $source"
    }
    Copy-Item -LiteralPath $source -Destination (Join-Path $PackageRoot $binary)
}

$signatureStatus = "unsigned"
if (-not [string]::IsNullOrWhiteSpace($CertificateThumbprint)) {
    if ([string]::IsNullOrWhiteSpace($SignToolPath)) {
        $resolvedSignTool = Get-Command signtool.exe -ErrorAction SilentlyContinue
        if ($null -ne $resolvedSignTool) {
            $SignToolPath = $resolvedSignTool.Source
        }
    }
    if ([string]::IsNullOrWhiteSpace($SignToolPath) -or
        -not (Test-Path -LiteralPath $SignToolPath -PathType Leaf)) {
        throw "Authenticode signing requested, but signtool.exe was not found."
    }
    foreach ($binary in $PackagedBinaries) {
        $path = Join-Path $PackageRoot $binary
        & $SignToolPath sign /sha1 $CertificateThumbprint /fd SHA256 /tr $TimestampUrl /td SHA256 $path
        if ($LASTEXITCODE -ne 0) {
            throw "signtool failed for $binary with exit code $LASTEXITCODE"
        }
        $signature = Get-AuthenticodeSignature -LiteralPath $path
        if ($signature.Status -ne "Valid") {
            throw "Authenticode signature validation failed for ${binary}: $($signature.Status)"
        }
    }
    $signatureStatus = "valid"
}
elseif ($RequireSignature) {
    throw "A production signature is required, but no certificate thumbprint was supplied."
}

foreach ($file in @(
    "Configure-CodexMcp.ps1",
    "Manage-Coordinator.ps1",
    "Test-ExternalWindowsAcceptance.ps1",
    "Install-CodexCCPanes.ps1",
    "Install-CodexCCPanes.cmd",
    "Uninstall-CodexCCPanes.ps1",
    "Uninstall-CodexCCPanes.cmd"
)) {
    Copy-Item -LiteralPath (Join-Path $ProductRoot "packaging\$file") `
        -Destination (Join-Path $PackageRoot $file)
}
Copy-Item -LiteralPath (Join-Path $ProductRoot "README.md") -Destination $PackageRoot
Copy-Item -LiteralPath (Join-Path $ProductRoot "docs") -Destination (Join-Path $PackageRoot "docs") -Recurse
Copy-Item -LiteralPath (Join-Path $ProductRoot "docs\EXTERNAL_WINDOWS_ACCEPTANCE.md") `
    -Destination (Join-Path $PackageRoot "EXTERNAL_WINDOWS_ACCEPTANCE.md")
Copy-Item -LiteralPath (Join-Path $ProductRoot "docs\RELEASE_NOTES_0.2.0-beta.1.md") `
    -Destination (Join-Path $PackageRoot "RELEASE_NOTES_0.2.0-beta.1.md")
Copy-Item -LiteralPath (Join-Path $ProductRoot "packaging\THIRD_PARTY_NOTICES.md") `
    -Destination (Join-Path $PackageRoot "THIRD_PARTY_NOTICES.md")
Set-Content -LiteralPath (Join-Path $PackageRoot "VERSION") -Value $Version -Encoding ascii -NoNewline

$artifactManifest = [ordered]@{
    schemaVersion = 1
    product = "CodexCCPanesCompanion"
    productVersion = $Version
    artifactKind = "coordination-plane-package"
    componentExecutables = @(
        [ordered]@{
            name = "CodexCCPanes.exe"
            role = "launcher"
            owns = @("official-cc-panes-discovery", "codex-desktop-environment-isolation")
        },
        [ordered]@{
            name = "codex-ccpanes-coordinator.exe"
            role = "coordinator"
            owns = @("workflow-state", "worker-state", "outbox", "lease", "event", "budget", "result")
        },
        [ordered]@{
            name = "codex-ccpanes-mcp.exe"
            role = "mcp-stdio"
            owns = @("mcp-tool-boundary", "input-validation", "local-ipc-client")
        }
    )
    supportExecutables = $SupportBinaries
    sideEffectBoundary = [ordered]@{
        managesOfficialCodex = $false
        managesOfficialCcPanes = $false
        registersUserMcp = $false
        startsBackgroundCoordinator = $false
        checksumCoversPackagePayload = $true
        installedVersionWritesInstallManifest = $true
    }
    codexMcpConfiguration = [ordered]@{
        transactionScript = "Configure-CodexMcp.ps1"
        requiresExplicitConfigPath = $true
        backupBeforeWrite = $true
        rollbackRestoresBeforeSnapshot = $true
        installerRunsByDefault = $false
        installerSupportsExplicitApplyRepair = $true
        uninstallerSupportsExplicitRemove = $true
    }
    coordinatorLifecycle = [ordered]@{
        entryScript = "Manage-Coordinator.ps1"
        installerRunsByDefault = $false
        startsBackgroundCoordinator = $false
        supportsDescribe = $true
        supportsVerify = $true
        supportsHealth = $true
        supportsStartPlan = $true
        healthRunsForeground = $true
        defaultDataDirUnderInstallRoot = $true
        defaultDatabaseUnderDataDir = $true
    }
    externalWindowsAcceptance = [ordered]@{
        contractVersion = "external-windows-acceptance/v1"
        packagePath = "EXTERNAL_WINDOWS_ACCEPTANCE.md"
        docsPath = "docs/EXTERNAL_WINDOWS_ACCEPTANCE.md"
        acceptanceRunner = "Test-ExternalWindowsAcceptance.ps1"
        requiresWindowsX64 = $true
        requiresExplicitInstallRoot = $true
        usesSyntheticCodexConfigOnly = $true
        allowsReadOnlyProbeDoctor = $true
        realDispatchEnabled = $false
        startsBackgroundCoordinator = $false
        runnerUsesWindowsPowerShell = $true
    }
    betaReleaseNotes = [ordered]@{
        contractVersion = "beta-release-notes/v1"
        targetVersion = "0.2.0-beta.1"
        packagePath = "RELEASE_NOTES_0.2.0-beta.1.md"
        docsPath = "docs/RELEASE_NOTES_0.2.0-beta.1.md"
        packageVersion = $Version
        statesNoDefaultMcpRegistration = $true
        statesNoBackgroundCoordinator = $true
        statesNoRealDispatch = $true
        statesUnsignedLocalBuildAllowed = $true
        statesProductionSignatureRequired = $true
    }
}
$artifactManifestJson = $artifactManifest | ConvertTo-Json -Depth 8
Write-Utf8NoBomText -Path (Join-Path $PackageRoot "ARTIFACT_MANIFEST.json") -Text $artifactManifestJson

$dependencies = @(
    $metadata.packages |
        Sort-Object name, version |
        ForEach-Object {
            [ordered]@{
                name = $_.name
                version = $_.version
                license = $_.license
                source = $_.source
            }
        }
)
$dependenciesManifest = [ordered]@{
    schemaVersion = 1
    generatedFor = "CodexCCPanesCompanion"
    productVersion = $Version
    dependencies = $dependencies
}
$dependenciesManifestJson = $dependenciesManifest | ConvertTo-Json -Depth 6
Write-Utf8NoBomText -Path (Join-Path $PackageRoot "DEPENDENCIES.json") -Text $dependenciesManifestJson

# Keep the actual license texts beside the dependency inventory.  A SPDX
# identifier alone is not sufficient for a redistributable package: the
# corresponding crate text must be available to recipients of the archive.
$licenseRoot = Join-Path $PackageRoot "THIRD_PARTY_LICENSES"
New-Item -ItemType Directory -Force -Path $licenseRoot | Out-Null
$licensePackages = @()
$externalPackages = @(
    $metadata.packages |
        Where-Object { $null -ne $_.source } |
        Sort-Object name, version
)
foreach ($dependency in $externalPackages) {
    $dependencyLicense = [string]$dependency.license
    if ([string]::IsNullOrWhiteSpace($dependencyLicense)) {
        throw "Dependency has no declared license: $($dependency.name) $($dependency.version)"
    }
    $manifestPath = [string]$dependency.manifest_path
    if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
        throw "Dependency manifest is missing: $manifestPath"
    }
    $sourceRoot = Split-Path -Parent $manifestPath
    $licenseFiles = @(
        Get-ChildItem -LiteralPath $sourceRoot -File -Force |
            Where-Object {
                $_.Name -match '^(?i:LICENSE|COPYING|NOTICE|UNLICENSE)([-_.].*)?$'
            } |
            Sort-Object Name
    )
    if ($licenseFiles.Count -lt 1) {
        throw "No license text found for dependency: $($dependency.name) $($dependency.version)"
    }
    $safeName = (($dependency.name -replace '[^A-Za-z0-9._-]', '_') + "-" +
        ($dependency.version -replace '[^A-Za-z0-9._-]', '_'))
    $destinationRoot = Join-Path $licenseRoot $safeName
    New-Item -ItemType Directory -Force -Path $destinationRoot | Out-Null
    $relativeFiles = @()
    foreach ($licenseFile in $licenseFiles) {
        $destination = Join-Path $destinationRoot $licenseFile.Name
        Copy-Item -LiteralPath $licenseFile.FullName -Destination $destination
        $relativeFiles += $licenseFile.Name
    }
    $licensePackages += [ordered]@{
        name = $dependency.name
        version = $dependency.version
        license = $dependencyLicense
        source = $dependency.source
        files = $relativeFiles
        directory = $safeName
    }
}
$licenseManifest = [ordered]@{
    schemaVersion = 1
    generatedFor = "CodexCCPanesCompanion"
    productVersion = $Version
    packages = $licensePackages
}
Write-Utf8NoBomText -Path (Join-Path $licenseRoot "MANIFEST.json") `
    -Text ($licenseManifest | ConvertTo-Json -Depth 8)

$checksumLines = foreach ($file in Get-ChildItem -LiteralPath $PackageRoot -Recurse -File |
    Where-Object { $_.Name -ne "SHA256SUMS" } |
    Sort-Object FullName) {
    $relative = (Convert-ToRelativePath -Root $PackageRoot -Path $file.FullName).Replace('\', '/')
    $hash = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash
    "$hash *$relative"
}
$checksumLines | Set-Content -LiteralPath (Join-Path $PackageRoot "SHA256SUMS") -Encoding ascii

Compress-Archive -LiteralPath $PackageRoot -DestinationPath $ZipPath -CompressionLevel Optimal
$artifact = [ordered]@{
    schemaVersion = 1
    product = "CodexCCPanesCompanion"
    version = $Version
    packageDirectory = $PackageRoot
    zip = $ZipPath
    zipBytes = (Get-Item -LiteralPath $ZipPath).Length
    zipSha256 = (Get-FileHash -LiteralPath $ZipPath -Algorithm SHA256).Hash
    cargoTargetDir = $CargoTargetDir
    signatureStatus = $signatureStatus
    dependencyCount = $dependencies.Count
    componentExecutables = $ComponentBinaries
    supportExecutables = $SupportBinaries
    externalWindowsAcceptance = "EXTERNAL_WINDOWS_ACCEPTANCE.md"
    externalWindowsAcceptanceRunner = "Test-ExternalWindowsAcceptance.ps1"
    betaReleaseNotes = "RELEASE_NOTES_0.2.0-beta.1.md"
}
$artifactJson = $artifact | ConvertTo-Json -Depth 4
Write-Utf8NoBomText -Path (Join-Path $OutputRoot "$PackageName.artifact.json") -Text $artifactJson
$artifact
