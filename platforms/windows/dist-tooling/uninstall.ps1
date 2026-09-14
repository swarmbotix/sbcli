<#
.SYNOPSIS
    swarmbotix sb CLI uninstaller (Windows).

.DESCRIPTION
    Removes swarmbotix from this machine, in this order:

      1. the $SB_HOME\bin entry install.ps1 added to the USER PATH
      2. $SB_HOME itself (default: %USERPROFILE%\.swarmbotix) - binary,
         config, message vault and reference docs

    It ships inside swarmbotix-<version>-windows-x86_64.zip beside
    install.ps1, and install.ps1 also places a copy at $SB_HOME\uninstall.ps1.
    Either copy works; they are the same file.

    RUNS ON ANY POWERSHELL: Windows PowerShell 5.1 as well as 7+, under the
    same two rules as install.ps1 - ASCII ONLY (a non-ASCII byte decodes to a
    smart quote under 5.1's Windows-1252 assumption and can break the parse),
    and no 6+/7+ syntax: no `??`, no ternary, no `&&`/`||` chains.

.PARAMETER Yes
    Non-interactive; delete $SB_HOME without confirming.

.PARAMETER KeepHome
    Remove the PATH entry and the binary only. Config and edited messages stay.

.EXAMPLE
    .\uninstall.ps1
    powershell -ExecutionPolicy Bypass -File .\uninstall.ps1 -Yes
    .\uninstall.ps1 -KeepHome
#>
[CmdletBinding()]
param([switch]$Yes, [switch]$KeepHome)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($env:SB_HOME) { $SbHome = $env:SB_HOME }
else { $SbHome = Join-Path $env:USERPROFILE '.swarmbotix' }
$SbHome = [System.IO.Path]::GetFullPath($SbHome)
$BinDir = Join-Path $SbHome 'bin'

# Never let a stray SB_HOME turn this into a much bigger delete.
$profileDir = [System.IO.Path]::GetFullPath($env:USERPROFILE)
if ($SbHome -eq $profileDir -or $SbHome.Length -le 3) {
    Write-Error "SB_HOME is $SbHome - refusing to delete it."
    exit 1
}

# The installed copy of this script sits inside the tree it is about to
# delete. PowerShell reads a .ps1 fully before running it, so the script file
# itself is safe to remove mid-run - but a directory cannot be deleted while
# it is the current location, and the user very likely cd'd into $SB_HOME to
# get here. Step out first.
Set-Location $profileDir

Write-Host "Uninstalling swarmbotix from $SbHome"

# --- 1. PATH ---------------------------------------------------------
# Read 'User', write 'User'. NEVER rebuild this from $env:Path - that is the
# merged machine+user+session string, and writing it back into the User scope
# permanently duplicates every machine entry.
$user = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($user) {
    $parts = @($user -split ';' | Where-Object { $_ })
    $kept  = @($parts | Where-Object { $_ -ne $BinDir })
    if ($kept.Count -ne $parts.Count) {
        [Environment]::SetEnvironmentVariable('Path', ($kept -join ';'), 'User')
        Write-Host "  removed $BinDir from the user PATH"
    } else {
        Write-Host "  user PATH has no $BinDir entry"
    }
} else {
    Write-Host "  user PATH is empty"
}

# --- 2. $SB_HOME -----------------------------------------------------
if ($KeepHome) {
    Remove-Item "$BinDir\sb.exe" -Force -ErrorAction SilentlyContinue
    Write-Host "  removed $BinDir\sb.exe"
    Write-Host "  kept $SbHome (config, messages, documents)"
}
elseif (-not (Test-Path $SbHome)) {
    Write-Host "  $SbHome does not exist - nothing to remove"
}
else {
    $doDelete = $false
    if ($Yes) {
        $doDelete = $true
    } else {
        $ans = Read-Host "Delete $SbHome (config, messages, documents)? [Y/n]"
        if ($ans -eq '' -or $ans -in @('y','Y','yes','YES')) { $doDelete = $true }
    }
    if ($doDelete) {
        Remove-Item $SbHome -Recurse -Force
        Write-Host "  removed $SbHome"
    } else {
        Write-Host "  kept $SbHome"
    }
}

Write-Host ""
Write-Host "Done. Open a new terminal so the PATH change takes effect."
