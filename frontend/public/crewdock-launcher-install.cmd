@echo off
setlocal
set "CREWDOCK_INSTALL_DIR=%LOCALAPPDATA%\crewdock-launcher"
set "CREWDOCK_SCRIPT=%CREWDOCK_INSTALL_DIR%\crewdock-launcher.ps1"
set "CREWDOCK_SELF=%~f0"

if not exist "%CREWDOCK_INSTALL_DIR%" mkdir "%CREWDOCK_INSTALL_DIR%"

powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "$raw=[IO.File]::ReadAllText($env:CREWDOCK_SELF); $marker=('###'+'CREWDOCK_POWERSHELL###'); $index=$raw.LastIndexOf($marker); if($index -lt 0){throw 'Launcher payload is missing'}; $payload=$raw.Substring($index+$marker.Length).TrimStart([char]13,[char]10); [IO.File]::WriteAllText($env:CREWDOCK_SCRIPT,$payload,[Text.UTF8Encoding]::new($false)); $pwsh=Get-Command pwsh.exe -All -ErrorAction SilentlyContinue | Sort-Object Version -Descending | Select-Object -First 1 -ExpandProperty Source; if(-not $pwsh){throw 'PowerShell 7 is required'}; $base='HKCU:\Software\Classes\crewdock'; New-Item -Path ($base+'\shell\open\command') -Force | Out-Null; Set-Item -Path $base -Value 'URL:crewdock Connection Launcher'; New-ItemProperty -Path $base -Name 'URL Protocol' -Value '' -PropertyType String -Force | Out-Null; $q=[char]34; $command=$q+$pwsh+$q+' -NoExit -NoProfile -ExecutionPolicy Bypass -File '+$q+$env:CREWDOCK_SCRIPT+$q+' '+$q+'%%1'+$q; Set-Item -Path ($base+'\shell\open\command') -Value $command"

if errorlevel 1 (
  echo Installation failed.
) else (
  echo crewdock connection launcher installed for the current Windows user.
)
pause
exit /b

###CREWDOCK_POWERSHELL###
param([Parameter(Mandatory = $true)][string]$Uri)

$ErrorActionPreference = 'Stop'

function Get-QueryValues([Uri]$ParsedUri) {
    $values = @{}
    foreach ($pair in $ParsedUri.Query.TrimStart('?').Split('&', [StringSplitOptions]::RemoveEmptyEntries)) {
        $parts = $pair.Split('=', 2)
        $name = [Uri]::UnescapeDataString($parts[0])
        $value = if ($parts.Count -gt 1) { [Uri]::UnescapeDataString($parts[1]) } else { '' }
        $values[$name] = $value
    }
    return $values
}

function Require-Value($Values, [string]$Name, [string]$Pattern) {
    $value = [string]$Values[$Name]
    if ([string]::IsNullOrWhiteSpace($value) -or $value -notmatch $Pattern) {
        throw "Invalid $Name"
    }
    return $value
}

function Quote-ProcessArgument([string]$Value) {
    return '"' + $Value.Replace('"', '\"') + '"'
}

try {
    $parsed = [Uri]$Uri
    if ($parsed.Scheme -ne 'crewdock') { throw 'Unsupported connection protocol' }
    $kind = $parsed.Host.ToLowerInvariant()
    $values = Get-QueryValues $parsed
    $hostName = Require-Value $values 'host' '^[A-Za-z0-9._:-]+$'
    $username = Require-Value $values 'username' '^[A-Za-z0-9._@\\+\$-]+$'
    $port = 0
    if (-not [int]::TryParse([string]$values['port'], [ref]$port) -or $port -lt 1 -or $port -gt 65535) {
        throw 'Invalid port'
    }

    if ($kind -eq 'ssh') {
        $sshArguments = @('-p', [string]$port)
        if ([string]$values['auth'] -eq 'key') {
            $identity = [string]$values['identity']
            if ($identity.StartsWith('~/') -or $identity.StartsWith('~\')) {
                $identity = Join-Path $env:USERPROFILE $identity.Substring(2)
            }
            if ([string]::IsNullOrWhiteSpace($identity) -or -not (Test-Path -LiteralPath $identity -PathType Leaf)) {
                throw 'The configured SSH identity file does not exist on this computer.'
            }
            $sshArguments += @('-i', $identity)
        }
        $sshArguments += "$username@$hostName"

        & ssh.exe @sshArguments
        if ($LASTEXITCODE -ne 0) {
            Read-Host 'SSH connection failed. Press Enter to close' | Out-Null
        }
        return
    }

    if ($kind -eq 'rdp') {
        $clipboard = if ([string]$values['clipboard'] -eq '0') { 0 } else { 1 }
        $drives = if ([string]$values['drives'] -eq '1') { 1 } else { 0 }
        $driveStore = if ($drives -eq 1) { '*' } else { '' }
        $rdpPath = Join-Path $env:TEMP ("crewdock-{0}.rdp" -f [Guid]::NewGuid().ToString('N'))
        $rdp = @(
            "full address:s:${hostName}:${port}",
            "username:s:${username}",
            "prompt for credentials:i:1",
            "authentication level:i:2",
            "redirectclipboard:i:${clipboard}",
            "redirectdrives:i:${drives}",
            "drivestoredirect:s:${driveStore}",
            "redirectprinters:i:0",
            "redirectcomports:i:0"
        ) -join "`r`n"
        Set-Content -LiteralPath $rdpPath -Value $rdp -Encoding Unicode
        Start-Process -FilePath 'mstsc.exe' -ArgumentList (Quote-ProcessArgument $rdpPath)
        exit
    }

    throw 'Unsupported connection type'
} catch {
    Add-Type -AssemblyName PresentationFramework
    [System.Windows.MessageBox]::Show($_.Exception.Message, 'crewdock connection failed', 'OK', 'Error') | Out-Null
}
