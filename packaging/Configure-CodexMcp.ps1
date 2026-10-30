[CmdletBinding()]
param(
    [ValidateSet("Apply", "Repair", "Verify", "Rollback", "Remove")]
    [string]$Action = "Verify",
    [string]$ConfigPath,
    [string]$InstallRoot = (Join-Path $env:LOCALAPPDATA "CodexPaneForge"),
    [string]$McpExecutable,
    [string]$DiscoveryFile,
    [string]$RegistrationName = "codex_paneforge",
    [string]$TransactionPath,
    [switch]$RequireRegistered,
    [switch]$Json,
    [switch]$Quiet
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ProductId = "CodexPaneForgeCompanion"
$BeginMarkerPrefix = "# BEGIN CodexPaneForgeCompanion MCP"
$EndMarkerPrefix = "# END CodexPaneForgeCompanion MCP"

function Write-Step {
    param([string]$Message)
    if (-not $Quiet) {
        Write-Host "[Codex + PaneForge] $Message"
    }
}

function Assert-RegistrationName {
    param([string]$Name)
    if ($Name -notmatch '^[A-Za-z0-9_-]+$') {
        throw "Invalid MCP registration name: $Name"
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

function ConvertTo-TomlString {
    param([string]$Value)
    $escaped = $Value.Replace('\', '\\').Replace('"', '\"')
    return '"' + $escaped + '"'
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

function Write-TextAtomic {
    param(
        [string]$Path,
        [string]$Text
    )
    $parent = Split-Path -Parent $Path
    if (-not [string]::IsNullOrWhiteSpace($parent)) {
        New-Item -ItemType Directory -Force -Path $parent | Out-Null
    }
    $temporary = Join-Path $parent (".{0}.tmp-{1}" -f (Split-Path -Leaf $Path), [guid]::NewGuid().ToString("N"))
    $encoding = New-Object System.Text.UTF8Encoding -ArgumentList $false
    [System.IO.File]::WriteAllText($temporary, $Text, $encoding)
    Move-Item -LiteralPath $temporary -Destination $Path -Force
}

function Write-JsonAtomic {
    param(
        [string]$Path,
        [object]$Value
    )
    $json = $Value | ConvertTo-Json -Depth 8
    Write-TextAtomic -Path $Path -Text $json
}

function Copy-FileAtomic {
    param(
        [string]$Source,
        [string]$Destination
    )
    $parent = Split-Path -Parent $Destination
    if (-not [string]::IsNullOrWhiteSpace($parent)) {
        New-Item -ItemType Directory -Force -Path $parent | Out-Null
    }
    $temporary = Join-Path $parent (".{0}.tmp-{1}" -f (Split-Path -Leaf $Destination), [guid]::NewGuid().ToString("N"))
    Copy-Item -LiteralPath $Source -Destination $temporary -Force
    Move-Item -LiteralPath $temporary -Destination $Destination -Force
}

function Get-ManagedMarkers {
    param([string]$Name)
    return @{
        Begin = "$BeginMarkerPrefix $Name"
        End = "$EndMarkerPrefix $Name"
    }
}

function Remove-ManagedBlock {
    param(
        [string]$Text,
        [string]$Name
    )
    $markers = Get-ManagedMarkers -Name $Name
    $pattern = "(?ms)^\s*" + [regex]::Escape($markers.Begin) + ".*?^\s*" + [regex]::Escape($markers.End) + "\s*(?:\r?\n)?"
    return [regex]::Replace($Text, $pattern, "")
}

function Test-UnmanagedRegistration {
    param(
        [string]$Text,
        [string]$Name
    )
    $unquoted = "(?m)^\s*\[mcp_servers\." + [regex]::Escape($Name) + "\]\s*$"
    $quoted = "(?m)^\s*\[mcp_servers\.`"" + [regex]::Escape($Name) + "`"\]\s*$"
    return ($Text -match $unquoted -or $Text -match $quoted)
}

function New-ManagedBlock {
    param(
        [string]$Name,
        [string]$Executable,
        [string]$Root,
        [string]$Discovery
    )
    $lines = @(
        "$BeginMarkerPrefix $Name",
        "[mcp_servers.$Name]",
        "command = $(ConvertTo-TomlString $Executable)",
        "args = []",
        "[mcp_servers.$Name.env]",
        "CODEX_PANEFORGE_INSTALL_ROOT = $(ConvertTo-TomlString $Root)",
        "CODEX_PANEFORGE_MANAGED_BY = ""$ProductId"""
    )
    if (-not [string]::IsNullOrWhiteSpace($Discovery)) {
        $lines += "CODEX_PANEFORGE_DISCOVERY_FILE = $(ConvertTo-TomlString $Discovery)"
    }
    $lines += @("$EndMarkerPrefix $Name", "")
    return $lines -join [Environment]::NewLine
}

function Get-RegistrationStatus {
    param(
        [string]$Path,
        [string]$Name,
        [string]$ExpectedExecutable,
        [string]$ExpectedDiscovery
    )
    $snapshot = Get-FileSnapshot -Path $Path
    if (-not $snapshot.exists) {
        return [ordered]@{
            registered = $false
            managed = $false
            commandMatches = $false
            discoveryMatches = $false
            config = $snapshot
        }
    }
    $text = Get-Content -LiteralPath $Path -Raw
    $markers = Get-ManagedMarkers -Name $Name
    $managed = $text.Contains($markers.Begin) -and $text.Contains($markers.End)
    $commandMatches = $false
    $discoveryMatches = $false
    if ($managed -and -not [string]::IsNullOrWhiteSpace($ExpectedExecutable)) {
        $expectedLine = "command = $(ConvertTo-TomlString $ExpectedExecutable)"
        $commandMatches = $text.Contains($expectedLine)
    }
    if ($managed -and -not [string]::IsNullOrWhiteSpace($ExpectedDiscovery)) {
        $expectedDiscoveryLine = "CODEX_PANEFORGE_DISCOVERY_FILE = $(ConvertTo-TomlString $ExpectedDiscovery)"
        $discoveryMatches = $text.Contains($expectedDiscoveryLine)
    }
    return [ordered]@{
        registered = $managed
        managed = $managed
        commandMatches = $commandMatches
        discoveryMatches = $discoveryMatches
        config = $snapshot
    }
}

function Backup-Config {
    param(
        [string]$Root,
        [string]$Path,
        [string]$TransactionId,
        [object]$Before
    )
    if (-not $Before.exists) {
        return $null
    }
    $backupRoot = Join-Path $Root "backups\codex-mcp"
    $backupPath = Join-Path $backupRoot "$TransactionId-config.toml"
    Assert-ChildPath -Parent $Root -Child $backupPath
    New-Item -ItemType Directory -Force -Path $backupRoot | Out-Null
    Copy-Item -LiteralPath $Path -Destination $backupPath -Force
    $backupHash = (Get-FileHash -LiteralPath $backupPath -Algorithm SHA256).Hash
    if ($backupHash -ne $Before.sha256) {
        throw "Codex MCP configuration backup hash mismatch."
    }
    return $backupPath
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

Assert-RegistrationName -Name $RegistrationName
$InstallRoot = [System.IO.Path]::GetFullPath($InstallRoot)

if ($Action -in @("Apply", "Repair", "Verify", "Remove")) {
    if ([string]::IsNullOrWhiteSpace($ConfigPath)) {
        throw "ConfigPath is required for $Action."
    }
    $ConfigPath = [System.IO.Path]::GetFullPath($ConfigPath)
}

if ($Action -in @("Apply", "Repair", "Verify")) {
    if ([string]::IsNullOrWhiteSpace($McpExecutable)) {
        throw "McpExecutable is required for $Action."
    }
    $McpExecutable = [System.IO.Path]::GetFullPath($McpExecutable)
    if (-not (Test-Path -LiteralPath $McpExecutable -PathType Leaf)) {
        throw "MCP executable is missing: $McpExecutable"
    }
    if (-not [string]::IsNullOrWhiteSpace($DiscoveryFile)) {
        $DiscoveryFile = [System.IO.Path]::GetFullPath($DiscoveryFile)
        if (-not (Test-Path -LiteralPath $DiscoveryFile -PathType Leaf)) {
            throw "PaneForge discovery file is missing: $DiscoveryFile"
        }
    }
}

if ($Action -in @("Apply", "Repair", "Remove")) {
    New-Item -ItemType Directory -Force -Path $InstallRoot | Out-Null
}

if ($Action -eq "Verify") {
    $status = Get-RegistrationStatus -Path $ConfigPath -Name $RegistrationName `
        -ExpectedExecutable $McpExecutable -ExpectedDiscovery $DiscoveryFile
    if ($RequireRegistered -and (
            -not $status.registered -or
            -not $status.commandMatches -or
            (-not [string]::IsNullOrWhiteSpace($DiscoveryFile) -and -not $status.discoveryMatches)
        )) {
        throw "Codex MCP registration contract is not satisfied."
    }
    Complete-Report -Report ([ordered]@{
        schemaVersion = 1
        productId = $ProductId
        action = "verify"
        ok = $true
        registrationName = $RegistrationName
        status = $status
    })
    return
}

if ($Action -in @("Apply", "Repair")) {
    $before = Get-FileSnapshot -Path $ConfigPath
    $transactionId = [guid]::NewGuid().ToString("N")
    $backupPath = Backup-Config -Root $InstallRoot -Path $ConfigPath -TransactionId $transactionId -Before $before
    $existing = if ($before.exists) { Get-Content -LiteralPath $ConfigPath -Raw } else { "" }
    $withoutManaged = Remove-ManagedBlock -Text $existing -Name $RegistrationName
    if (Test-UnmanagedRegistration -Text $withoutManaged -Name $RegistrationName) {
        throw "An unmanaged Codex MCP registration already exists for $RegistrationName."
    }
    $block = New-ManagedBlock -Name $RegistrationName -Executable $McpExecutable `
        -Root $InstallRoot -Discovery $DiscoveryFile
    $prefix = $withoutManaged.TrimEnd()
    $next = if ([string]::IsNullOrWhiteSpace($prefix)) {
        $block
    }
    else {
        $prefix + [Environment]::NewLine + [Environment]::NewLine + $block
    }
    Write-TextAtomic -Path $ConfigPath -Text $next
    $after = Get-FileSnapshot -Path $ConfigPath
    $transaction = [ordered]@{
        schemaVersion = 1
        productId = $ProductId
        operation = "codex-mcp-$($Action.ToLowerInvariant())"
        transactionId = $transactionId
        registrationName = $RegistrationName
        configPath = $ConfigPath
        installRoot = $InstallRoot
        mcpExecutable = $McpExecutable
        discoveryFile = $DiscoveryFile
        backupPath = $backupPath
        before = $before
        after = $after
        createdAtUtc = [DateTime]::UtcNow.ToString("o")
    }
    $transactionRoot = Join-Path $InstallRoot "transactions"
    New-Item -ItemType Directory -Force -Path $transactionRoot | Out-Null
    $transactionFile = Join-Path $transactionRoot "$transactionId-codex-mcp.json"
    Assert-ChildPath -Parent $InstallRoot -Child $transactionFile
    Write-JsonAtomic -Path $transactionFile -Value $transaction
    $status = Get-RegistrationStatus -Path $ConfigPath -Name $RegistrationName `
        -ExpectedExecutable $McpExecutable -ExpectedDiscovery $DiscoveryFile
    if (-not $status.registered -or -not $status.commandMatches -or
        (-not [string]::IsNullOrWhiteSpace($DiscoveryFile) -and -not $status.discoveryMatches)) {
        throw "Codex MCP registration verification failed after $Action."
    }
    Complete-Report -Report ([ordered]@{
        schemaVersion = 1
        productId = $ProductId
        action = $Action.ToLowerInvariant()
        ok = $true
        registrationName = $RegistrationName
        transactionPath = $transactionFile
        before = $before
        after = $after
        status = $status
    })
    return
}

if ($Action -eq "Remove") {
    $before = Get-FileSnapshot -Path $ConfigPath
    $transactionId = [guid]::NewGuid().ToString("N")
    $backupPath = Backup-Config -Root $InstallRoot -Path $ConfigPath -TransactionId $transactionId -Before $before
    if ($before.exists) {
        $existing = Get-Content -LiteralPath $ConfigPath -Raw
        $next = Remove-ManagedBlock -Text $existing -Name $RegistrationName
        if ([string]::IsNullOrWhiteSpace($next)) {
            Remove-Item -LiteralPath $ConfigPath -Force
        }
        else {
            Write-TextAtomic -Path $ConfigPath -Text $next
        }
    }
    $after = Get-FileSnapshot -Path $ConfigPath
    $transaction = [ordered]@{
        schemaVersion = 1
        productId = $ProductId
        operation = "codex-mcp-remove"
        transactionId = $transactionId
        registrationName = $RegistrationName
        configPath = $ConfigPath
        installRoot = $InstallRoot
        backupPath = $backupPath
        before = $before
        after = $after
        createdAtUtc = [DateTime]::UtcNow.ToString("o")
    }
    $transactionRoot = Join-Path $InstallRoot "transactions"
    New-Item -ItemType Directory -Force -Path $transactionRoot | Out-Null
    $transactionFile = Join-Path $transactionRoot "$transactionId-codex-mcp-remove.json"
    Write-JsonAtomic -Path $transactionFile -Value $transaction
    Complete-Report -Report ([ordered]@{
        schemaVersion = 1
        productId = $ProductId
        action = "remove"
        ok = $true
        registrationName = $RegistrationName
        transactionPath = $transactionFile
        before = $before
        after = $after
    })
    return
}

if ($Action -eq "Rollback") {
    if ([string]::IsNullOrWhiteSpace($TransactionPath)) {
        throw "TransactionPath is required for Rollback."
    }
    $TransactionPath = [System.IO.Path]::GetFullPath($TransactionPath)
    $transaction = Get-Content -LiteralPath $TransactionPath -Raw | ConvertFrom-Json
    if ($transaction.schemaVersion -ne 1 -or $transaction.productId -ne $ProductId) {
        throw "Unsupported Codex MCP transaction record."
    }
    $config = [string]$transaction.configPath
    if ([string]::IsNullOrWhiteSpace($config)) {
        throw "Transaction record is missing configPath."
    }
    $config = [System.IO.Path]::GetFullPath($config)
    $before = $transaction.before
    if ($before.exists -eq $true) {
        $backup = [string]$transaction.backupPath
        if ([string]::IsNullOrWhiteSpace($backup) -or -not (Test-Path -LiteralPath $backup -PathType Leaf)) {
            throw "Codex MCP transaction backup is missing."
        }
        Copy-FileAtomic -Source $backup -Destination $config
        $restored = Get-FileSnapshot -Path $config
        if ($restored.exists -ne $true -or $restored.bytes -ne $before.bytes -or $restored.sha256 -ne $before.sha256) {
            throw "Restored Codex MCP configuration does not match transaction snapshot."
        }
    }
    else {
        if (Test-Path -LiteralPath $config -PathType Leaf) {
            Remove-Item -LiteralPath $config -Force
        }
    }
    $after = Get-FileSnapshot -Path $config
    Complete-Report -Report ([ordered]@{
        schemaVersion = 1
        productId = $ProductId
        action = "rollback"
        ok = $true
        registrationName = [string]$transaction.registrationName
        config = $after
        transactionPath = $TransactionPath
    })
    return
}
