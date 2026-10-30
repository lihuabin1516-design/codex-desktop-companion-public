[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$PackageRoot
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PackageRoot = [System.IO.Path]::GetFullPath($PackageRoot)
$TestRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("codex-ccpanes-installer-test-" + [guid]::NewGuid().ToString("N"))
$InstallRoot = Join-Path $TestRoot "install"

function Assert-TestPath {
    param([string]$Path)
    $full = [System.IO.Path]::GetFullPath($Path)
    $temp = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (-not $full.StartsWith($temp, [System.StringComparison]::OrdinalIgnoreCase) -or
        (Split-Path -Leaf $TestRoot) -notlike "codex-ccpanes-installer-test-*") {
        throw "Unsafe installer test path: $full"
    }
}

function Write-Utf8NoBomText {
    param(
        [string]$Path,
        [string]$Text
    )
    $encoding = New-Object System.Text.UTF8Encoding -ArgumentList $false
    [System.IO.File]::WriteAllText($Path, $Text, $encoding)
}

Assert-TestPath -Path $TestRoot
New-Item -ItemType Directory -Path $TestRoot | Out-Null
try {
    $installer = Join-Path $PackageRoot "Install-CodexCCPanes.ps1"
    $uninstaller = Join-Path $PackageRoot "Uninstall-CodexCCPanes.ps1"

    & $installer -InstallRoot $InstallRoot -SkipShortcuts -SkipRegistry -Quiet | Out-Null
    $statePath = Join-Path $InstallRoot "install-state.json"
    if (-not (Test-Path -LiteralPath $statePath -PathType Leaf)) {
        throw "Install state missing after first install."
    }
    $state = Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json
    $descriptorJson = & $state.cliExecutable describe --json
    if ($LASTEXITCODE -ne 0) {
        throw "Installed descriptor command failed with exit code $LASTEXITCODE"
    }
    $descriptor = $descriptorJson | ConvertFrom-Json
    if ($descriptor.liveagentRuntimeDependency -ne $false) {
        throw "Installed descriptor contract failed."
    }
    foreach ($installedExecutable in @(
        $state.cliExecutable,
        $state.guiExecutable,
        $state.coordinatorExecutable,
        $state.mcpExecutable
    )) {
        if ([string]::IsNullOrWhiteSpace($installedExecutable) -or
            -not (Test-Path -LiteralPath $installedExecutable -PathType Leaf)) {
            throw "Installed executable missing from state: $installedExecutable"
        }
    }
    $current = Get-Content -LiteralPath (Join-Path $InstallRoot "current.json") -Raw | ConvertFrom-Json
    if ($current.coordinatorExecutable -ne $state.coordinatorExecutable -or
        $current.mcpExecutable -ne $state.mcpExecutable -or
        [string]::IsNullOrWhiteSpace([string]$current.coordinatorLifecycleScript)) {
        throw "Current install descriptor does not expose Coordination Plane executables."
    }
    $coordinatorLifecycle = Join-Path $InstallRoot "Manage-Coordinator.ps1"
    $versionCoordinatorLifecycle = Join-Path $state.currentVersionRoot "Manage-Coordinator.ps1"
    foreach ($lifecycleScript in @($coordinatorLifecycle, $versionCoordinatorLifecycle, $state.coordinatorLifecycleScript)) {
        if ([string]::IsNullOrWhiteSpace([string]$lifecycleScript) -or
            -not (Test-Path -LiteralPath $lifecycleScript -PathType Leaf)) {
            throw "Coordinator lifecycle script missing: $lifecycleScript"
        }
    }
    foreach ($contractFile in @(
        "EXTERNAL_WINDOWS_ACCEPTANCE.md",
        "RELEASE_NOTES_0.2.0-beta.1.md",
        "Test-ExternalWindowsAcceptance.ps1",
        "docs\EXTERNAL_WINDOWS_ACCEPTANCE.md",
        "docs\RELEASE_NOTES_0.2.0-beta.1.md"
    )) {
        $contractRoot = if ($contractFile -eq "Test-ExternalWindowsAcceptance.ps1") {
            $PackageRoot
        }
        else {
            $state.currentVersionRoot
        }
        if (-not (Test-Path -LiteralPath (Join-Path $contractRoot $contractFile) -PathType Leaf)) {
            throw "Installed Phase 8 package contract file missing: $contractFile"
        }
    }
    $installedArtifactManifest = Get-Content -LiteralPath (Join-Path $state.currentVersionRoot "ARTIFACT_MANIFEST.json") -Raw |
        ConvertFrom-Json
    if ($installedArtifactManifest.externalWindowsAcceptance.contractVersion -ne "external-windows-acceptance/v1" -or
        $installedArtifactManifest.externalWindowsAcceptance.acceptanceRunner -ne "Test-ExternalWindowsAcceptance.ps1" -or
        $installedArtifactManifest.externalWindowsAcceptance.usesSyntheticCodexConfigOnly -ne $true -or
        $installedArtifactManifest.externalWindowsAcceptance.realDispatchEnabled -ne $false -or
        $installedArtifactManifest.betaReleaseNotes.contractVersion -ne "beta-release-notes/v1" -or
        $installedArtifactManifest.betaReleaseNotes.targetVersion -ne "0.2.0-beta.1" -or
        $installedArtifactManifest.betaReleaseNotes.statesNoRealDispatch -ne $true) {
        throw "Installed Phase 8 acceptance/release notes manifest contract is invalid."
    }
    $contractCopyPairs = @(
        [pscustomobject]@{ Root = "EXTERNAL_WINDOWS_ACCEPTANCE.md"; Docs = "docs\EXTERNAL_WINDOWS_ACCEPTANCE.md" },
        [pscustomobject]@{ Root = "RELEASE_NOTES_0.2.0-beta.1.md"; Docs = "docs\RELEASE_NOTES_0.2.0-beta.1.md" }
    )
    foreach ($copyPair in $contractCopyPairs) {
        $rootHash = (Get-FileHash -LiteralPath (Join-Path $state.currentVersionRoot $copyPair.Root) -Algorithm SHA256).Hash
        $docsHash = (Get-FileHash -LiteralPath (Join-Path $state.currentVersionRoot $copyPair.Docs) -Algorithm SHA256).Hash
        if ($rootHash -ne $docsHash) {
            throw "Installed package contract root/docs copy mismatch: $($copyPair.Root)"
        }
    }
    $coordinatorVersion = & $state.coordinatorExecutable --version
    if ($LASTEXITCODE -ne 0 -or $coordinatorVersion -notmatch '^codex-ccpanes-coordinator\s+') {
        throw "Installed coordinator version contract failed."
    }
    $lifecycleDescribeJson = & $coordinatorLifecycle -Action Describe -InstallRoot $InstallRoot -Json -Quiet
    $lifecycleDescribe = $lifecycleDescribeJson | ConvertFrom-Json
    if ($lifecycleDescribe.ok -ne $true -or
        $lifecycleDescribe.defaultStartByInstaller -ne $false -or
        $lifecycleDescribe.startsBackgroundCoordinator -ne $false -or
        $lifecycleDescribe.startPlan.startsProcess -ne $false -or
        $lifecycleDescribe.startPlan.mode -ne "foreground-stdio-ipc") {
        throw "Coordinator lifecycle describe contract failed."
    }
    $lifecycleVerifyJson = & $coordinatorLifecycle -Action Verify -InstallRoot $InstallRoot -Json -Quiet
    $lifecycleVerify = $lifecycleVerifyJson | ConvertFrom-Json
    if ($lifecycleVerify.ok -ne $true -or
        $lifecycleVerify.startsBackgroundCoordinator -ne $false -or
        $lifecycleVerify.version -notmatch '^codex-ccpanes-coordinator\s+') {
        throw "Coordinator lifecycle verify contract failed."
    }
    $coordinatorDataDir = Join-Path $InstallRoot "data\coordinator"
    $coordinatorHealthJson = & $coordinatorLifecycle -Action Health -InstallRoot $InstallRoot `
        -DataDir $coordinatorDataDir -Json -Quiet
    $coordinatorHealth = $coordinatorHealthJson | ConvertFrom-Json
    if ($coordinatorHealth.ok -ne $true -or
        $coordinatorHealth.startsBackgroundCoordinator -ne $false -or
        $coordinatorHealth.health.status -ne "FROZEN" -or
        $coordinatorHealth.health.dispatchReady -ne $false -or
        $coordinatorHealth.database.exists -ne $true) {
        throw "Coordinator lifecycle foreground health contract failed."
    }
    $mcpDescriptorJson = & $state.mcpExecutable --describe
    if ($LASTEXITCODE -ne 0) {
        throw "Installed MCP descriptor command failed with exit code $LASTEXITCODE"
    }
    $mcpDescriptor = $mcpDescriptorJson | ConvertFrom-Json
    if ($mcpDescriptor.name -ne "codex-ccpanes-mcp" -or
        [string]::IsNullOrWhiteSpace($mcpDescriptor.mcpSchemaVersion) -or
        [string]::IsNullOrWhiteSpace($mcpDescriptor.ipcSchemaVersion)) {
        throw "Installed MCP descriptor contract failed."
    }
    $configManager = Join-Path $state.currentVersionRoot "Configure-CodexMcp.ps1"
    if (-not (Test-Path -LiteralPath $configManager -PathType Leaf)) {
        throw "Installed Codex MCP configuration manager missing."
    }
    $codexConfig = Join-Path $TestRoot "codex-home\config.toml"
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $codexConfig) | Out-Null
    $codexConfigText = @"
# synthetic Codex config
[mcp_servers.other]
command = "other"
"@
    Write-Utf8NoBomText -Path $codexConfig -Text $codexConfigText
    $syntheticDiscovery = [System.IO.Path]::Combine($TestRoot, "cc-panes", "runtime", "daemon-manifest.json")
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $syntheticDiscovery) | Out-Null
    Write-Utf8NoBomText -Path $syntheticDiscovery -Text '{"addr":"127.0.0.1:62674","token":"TOKEN_PLACEHOLDER","pid":1,"startedAt":1}'
    $beforeConfigHash = (Get-FileHash -LiteralPath $codexConfig -Algorithm SHA256).Hash
    $applyJson = & $configManager -Action Apply -ConfigPath $codexConfig -InstallRoot $InstallRoot `
        -McpExecutable $state.mcpExecutable -DiscoveryFile $syntheticDiscovery -Json -Quiet
    $apply = $applyJson | ConvertFrom-Json
    if ($apply.ok -ne $true -or -not (Test-Path -LiteralPath $apply.transactionPath -PathType Leaf)) {
        throw "Codex MCP synthetic apply transaction failed."
    }
    & $configManager -Action Verify -ConfigPath $codexConfig -InstallRoot $InstallRoot `
        -McpExecutable $state.mcpExecutable -DiscoveryFile $syntheticDiscovery `
        -RequireRegistered -Json -Quiet | Out-Null
    & $configManager -Action Repair -ConfigPath $codexConfig -InstallRoot $InstallRoot `
        -McpExecutable $state.mcpExecutable -DiscoveryFile $syntheticDiscovery -Json -Quiet | Out-Null
    $managedMatches = Select-String -LiteralPath $codexConfig -Pattern "# BEGIN CodexCCPanesCompanion MCP codex_ccpanes" |
        Measure-Object
    if ($managedMatches.Count -ne 1) {
        throw "Codex MCP repair duplicated managed configuration."
    }
    $managedConfig = Get-Content -LiteralPath $codexConfig -Raw
    $expectedDiscoveryPath = [System.IO.Path]::GetFullPath($syntheticDiscovery)
    $expectedDiscoveryLine = 'CODEX_CCPANES_DISCOVERY_FILE = "' +
        $expectedDiscoveryPath.Replace('\', '\\') + '"'
    if (-not $managedConfig.Contains($expectedDiscoveryLine)) {
        throw "Codex MCP managed configuration is missing the discovery file."
    }
    & $configManager -Action Rollback -TransactionPath $apply.transactionPath -Json -Quiet | Out-Null
    $afterConfigHash = (Get-FileHash -LiteralPath $codexConfig -Algorithm SHA256).Hash
    if ($afterConfigHash -ne $beforeConfigHash) {
        throw "Codex MCP rollback did not restore the original configuration."
    }
    $absentConfig = Join-Path $TestRoot "absent-codex-home\config.toml"
    $absentApplyJson = & $configManager -Action Apply -ConfigPath $absentConfig -InstallRoot $InstallRoot `
        -McpExecutable $state.mcpExecutable -Json -Quiet
    $absentApply = $absentApplyJson | ConvertFrom-Json
    & $configManager -Action Rollback -TransactionPath $absentApply.transactionPath -Json -Quiet | Out-Null
    if (Test-Path -LiteralPath $absentConfig) {
        throw "Codex MCP rollback did not restore absent config state."
    }

    $installerCodexConfig = Join-Path $TestRoot "installer-codex-home\config.toml"
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $installerCodexConfig) | Out-Null
    $installerCodexConfigText = @"
# installer synthetic Codex config
[mcp_servers.installer_other]
command = "installer-other"
"@
    Write-Utf8NoBomText -Path $installerCodexConfig -Text $installerCodexConfigText
    & $installer -InstallRoot $InstallRoot -SkipShortcuts -SkipRegistry -Quiet `
        -ConfigureCodexMcp -CodexConfigPath $installerCodexConfig | Out-Null
    $configuredState = Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json
    if ($configuredState.codexMcpConfiguration.configuredThisRun -ne $true -or
        $configuredState.codexMcpConfiguration.action -ne "repair" -or
        $configuredState.codexMcpConfiguration.configPath -ne [System.IO.Path]::GetFullPath($installerCodexConfig) -or
        -not (Test-Path -LiteralPath $configuredState.codexMcpConfiguration.transactionPath -PathType Leaf)) {
        throw "Installer-driven Codex MCP configuration state is invalid."
    }
    & $configManager -Action Verify -ConfigPath $installerCodexConfig -InstallRoot $InstallRoot `
        -McpExecutable $configuredState.mcpExecutable -RequireRegistered -Json -Quiet | Out-Null
    & $installer -InstallRoot $InstallRoot -SkipShortcuts -SkipRegistry -Quiet `
        -ConfigureCodexMcp -CodexConfigPath $installerCodexConfig | Out-Null
    $installerManagedMatches = Select-String -LiteralPath $installerCodexConfig `
        -Pattern "# BEGIN CodexCCPanesCompanion MCP codex_ccpanes" | Measure-Object
    if ($installerManagedMatches.Count -ne 1) {
        throw "Installer-driven Codex MCP repair duplicated managed configuration."
    }

    $fixture = Join-Path $TestRoot "fixture\cc-panes.exe"
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $fixture) | Out-Null
    Set-Content -LiteralPath $fixture -Value "fixture" -Encoding ascii -NoNewline
    $previousHome = $env:CODEX_CCPANES_HOME
    $env:CODEX_CCPANES_HOME = Join-Path $TestRoot "companion-home"
    try {
        $migrationJson = & $state.cliExecutable migrate-config --ccpanes-bin $fixture --apply --json
        if ($LASTEXITCODE -ne 0) {
            throw "Configuration migration command failed with exit code $LASTEXITCODE"
        }
        $migration = $migrationJson | ConvertFrom-Json
        if ($migration.applied -ne $true -or $migration.after.exists -ne $true) {
            throw "Configuration migration did not apply."
        }
        $probeJson = & $state.cliExecutable probe --json
        if ($LASTEXITCODE -ne 0) {
            throw "Installed probe command failed with exit code $LASTEXITCODE"
        }
        $probe = $probeJson | ConvertFrom-Json
        if ($probe.source -ne "environment" -or $probe.available -ne $true) {
            throw "Migrated configuration was not used for discovery."
        }
    }
    finally {
        if ($null -eq $previousHome) {
            Remove-Item Env:CODEX_CCPANES_HOME -ErrorAction SilentlyContinue
        }
        else {
            $env:CODEX_CCPANES_HOME = $previousHome
        }
    }

    & $installer -InstallRoot $InstallRoot -SkipShortcuts -SkipRegistry -Quiet | Out-Null
    $stateAfterUpgrade = Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json
    if ($stateAfterUpgrade.currentVersion -ne $state.currentVersion) {
        throw "Idempotent reinstall changed version unexpectedly."
    }

    & $uninstaller -InstallRoot $InstallRoot -SkipShortcuts -SkipRegistry -Quiet `
        -RemoveCodexMcp -CodexConfigPath $installerCodexConfig
    $installerConfigAfterUninstall = Get-Content -LiteralPath $installerCodexConfig -Raw
    if ($installerConfigAfterUninstall -match "# BEGIN CodexCCPanesCompanion MCP codex_ccpanes" -or
        $installerConfigAfterUninstall -notmatch "\[mcp_servers\.installer_other\]") {
        throw "Uninstaller-driven Codex MCP removal did not preserve unmanaged config content."
    }
    if (Test-Path -LiteralPath (Join-Path $InstallRoot "versions")) {
        throw "Managed version directories remain after uninstall."
    }
    if (Test-Path -LiteralPath $statePath) {
        throw "Install state remains after uninstall."
    }
    if (Test-Path -LiteralPath (Join-Path $InstallRoot "Manage-Coordinator.ps1") -PathType Leaf) {
        throw "Coordinator lifecycle root script remains after uninstall."
    }

    $freshInstallRoot = Join-Path $TestRoot "fresh-install-with-config"
    $freshCodexConfig = Join-Path $TestRoot "fresh-codex-home\config.toml"
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $freshCodexConfig) | Out-Null
    $freshCodexConfigText = @"
# fresh installer synthetic Codex config
[mcp_servers.fresh_other]
command = "fresh-other"
"@
    Write-Utf8NoBomText -Path $freshCodexConfig -Text $freshCodexConfigText
    & $installer -InstallRoot $freshInstallRoot -SkipShortcuts -SkipRegistry -Quiet `
        -ConfigureCodexMcp -CodexConfigPath $freshCodexConfig | Out-Null
    $freshStatePath = Join-Path $freshInstallRoot "install-state.json"
    $freshState = Get-Content -LiteralPath $freshStatePath -Raw | ConvertFrom-Json
    if ($freshState.codexMcpConfiguration.configuredThisRun -ne $true -or
        $freshState.codexMcpConfiguration.action -ne "apply") {
        throw "Fresh installer-driven Codex MCP apply state is invalid."
    }
    $freshConfigManager = Join-Path $freshState.currentVersionRoot "Configure-CodexMcp.ps1"
    & $freshConfigManager -Action Verify -ConfigPath $freshCodexConfig -InstallRoot $freshInstallRoot `
        -McpExecutable $freshState.mcpExecutable -RequireRegistered -Json -Quiet | Out-Null
    & $uninstaller -InstallRoot $freshInstallRoot -SkipShortcuts -SkipRegistry -Quiet `
        -RemoveCodexMcp -CodexConfigPath $freshCodexConfig
    $freshConfigAfterUninstall = Get-Content -LiteralPath $freshCodexConfig -Raw
    if ($freshConfigAfterUninstall -match "# BEGIN CodexCCPanesCompanion MCP codex_ccpanes" -or
        $freshConfigAfterUninstall -notmatch "\[mcp_servers\.fresh_other\]") {
        throw "Fresh uninstaller-driven Codex MCP removal did not preserve unmanaged config content."
    }

    $tamperedPackage = Join-Path $TestRoot "tampered-package"
    New-Item -ItemType Directory -Path $tamperedPackage | Out-Null
    Get-ChildItem -LiteralPath $PackageRoot -Force |
        Copy-Item -Destination $tamperedPackage -Recurse
    Add-Content -LiteralPath (Join-Path $tamperedPackage "README.md") -Value "`nTAMPERED"
    $tamperRejected = $false
    try {
        & (Join-Path $tamperedPackage "Install-CodexCCPanes.ps1") `
            -InstallRoot (Join-Path $TestRoot "tampered-install") `
            -SkipShortcuts -SkipRegistry -Quiet | Out-Null
    }
    catch {
        $tamperRejected = $_.Exception.Message -like "Checksum mismatch:*"
    }
    if (-not $tamperRejected) {
        throw "Tampered package was not rejected by the installer."
    }

    [pscustomobject]@{
        Install = "pass"
        ConfigMigration = "pass"
        ComponentInventory = "pass"
        CoordinatorLifecycle = "pass"
        ExternalWindowsAcceptance = "pass"
        BetaReleaseNotes = "pass"
        McpConfigTransaction = "pass"
        InstallerMcpConfig = "pass"
        Reinstall = "pass"
        Uninstall = "pass"
        TamperRejection = "pass"
        IsolatedRoot = $TestRoot
    }
}
finally {
    if (Test-Path -LiteralPath $TestRoot) {
        Assert-TestPath -Path $TestRoot
        Remove-Item -LiteralPath $TestRoot -Recurse -Force
    }
}
