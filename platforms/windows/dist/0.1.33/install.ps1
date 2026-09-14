<#
.SYNOPSIS
    swarmbotix sb CLI installer (Windows).

.DESCRIPTION
    Layout: this script lives next to a swarmbotix-<version>-windows-x86_64.zip
    in platforms/windows/dist/<version>/. It auto-detects the sibling zip,
    extracts to a temp dir, and copies the payload into $SB_HOME
    (default: %USERPROFILE%\.swarmbotix).

    Version-agnostic - it never hardcodes a version, so the same script ships
    with every cut.

    RUNS ON ANY POWERSHELL: Windows PowerShell 5.1 (the powershell.exe that
    ships with Windows) as well as PowerShell 7+. End users install with
    whatever their box already has, so this script carries no #requires and
    must stay free of pwsh-7-only syntax. Two rules keep that true:

      1. ASCII ONLY. A .ps1 with no BOM is read as UTF-8 by pwsh 7 but as
         Windows-1252 by 5.1. A UTF-8 box-drawing dash (E2 94 80) then decodes
         to three chars, the middle one being 0x94 = a smart double quote,
         which the 5.1 parser treats as a string delimiter - the whole file
         fails with "string is missing the terminator". Keep every byte < 0x80
         and the file parses identically under either assumption.
      2. No `??`, no `?:` ternary, no `&&`/`||` chains, no -Parallel, and no
         `-Encoding utf8NoBOM` (see Write-Utf8NoBom below). Those are 6+/7+.

    build.ps1 is the maintainer-side script and may require 7; this one may not.

.PARAMETER Yes
    Non-interactive; auto-accept the PATH edit.

.EXAMPLE
    .\install.ps1                                      # Windows PowerShell or pwsh
    powershell -ExecutionPolicy Bypass -File .\install.ps1 -Yes
    $env:SB_HOME='D:\sb'; .\install.ps1                # custom prefix
#>
[CmdletBinding()]
param([switch]$Yes)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# Write UTF-8 with no BOM, on 5.1 as well as 7+.
#
# `Set-Content -Encoding utf8NoBOM` is PowerShell 6+ only; on 5.1 that value
# does not exist and plain `utf8` emits a BOM. A BOM matters here: it would put
# EF BB BF in front of the first key of sb.config.yml, and in front of the
# first line of the generated uninstall.ps1. So go through .NET, which behaves
# identically on both editions.
#
# WriteAllText resolves a relative path against the *process* working
# directory, which is not necessarily PowerShell's current location - always
# hand it an absolute path. Combine() returns $Path unchanged when it is
# already rooted, so this is correct either way.
function Write-Utf8NoBom {
    param([Parameter(Mandatory = $true)][string]$Path,
          [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Text)
    $cwd  = (Get-Location -PSProvider FileSystem).ProviderPath
    $full = [System.IO.Path]::GetFullPath([System.IO.Path]::Combine($cwd, $Path))
    # Set-Content terminated its output with a newline; keep that.
    if (-not $Text.EndsWith("`n")) { $Text += "`r`n" }
    [System.IO.File]::WriteAllText($full, $Text, (New-Object System.Text.UTF8Encoding($false)))
}

# Read a UTF-8 file, on 5.1 as well as 7+.
#
# `Get-Content -Raw` is NOT encoding-neutral across editions: 7+ assumes UTF-8,
# but 5.1 assumes the box's ANSI codepage. sb.config.yml.template is UTF-8 and
# contains non-ASCII (the `->` arrows in its guidance comments), so on 5.1 a
# bare Get-Content decodes those bytes into Windows-1252 - and slots 0x81 0x8D
# 0x8F 0x90 0x9D are undefined there, arriving as C1 CONTROL characters. Those
# then get written straight into the seeded sb.config.yml, and sb rejects it
# with "control characters are not allowed at position N". ReadAllText is
# UTF-8-with-BOM-detection on both editions, and is the exact inverse of
# Write-Utf8NoBom above.
function Read-Utf8 {
    param([Parameter(Mandatory = $true)][string]$Path)
    $cwd  = (Get-Location -PSProvider FileSystem).ProviderPath
    $full = [System.IO.Path]::GetFullPath([System.IO.Path]::Combine($cwd, $Path))
    return [System.IO.File]::ReadAllText($full)
}

$ScriptDir = $PSScriptRoot

# %USERPROFILE% is what dirs::home_dir() resolves to on Windows, so this
# matches where sb itself will look for its home.
$UsingDefaultHome = $true
if ($env:SB_HOME) {
    $SbHome = $env:SB_HOME
    $UsingDefaultHome = $false
} else {
    $SbHome = Join-Path $env:USERPROFILE '.swarmbotix'
}

# --- Locate the payload ----------------------------------------------
$zips = @(Get-ChildItem -Path $ScriptDir -Filter 'swarmbotix-*-windows-x86_64.zip' -File)
if ($zips.Count -eq 0) {
    Write-Error "no swarmbotix-*-windows-x86_64.zip found next to install.ps1`n       (looked in $ScriptDir)"
    exit 1
}
if ($zips.Count -gt 1) {
    Write-Error ("multiple swarmbotix zips found; keep only one alongside install.ps1:`n  " +
                 ($zips.Name -join "`n  "))
    exit 1
}
$Zip = $zips[0]

# --- Extract ---------------------------------------------------------
$Tmp = Join-Path ([System.IO.Path]::GetTempPath()) "sb-install-$([System.Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Force -Path $Tmp | Out-Null
try {
    # Expand-Archive ships with PowerShell 5.0+ (Microsoft.PowerShell.Archive),
    # so it is available on a bare Windows box.
    Expand-Archive -Path $Zip.FullName -DestinationPath $Tmp -Force

    # The zip has exactly one top-level dir (the package stem) containing bin\.
    $payload = @(Get-ChildItem -Path $Tmp -Directory)
    if ($payload.Count -ne 1 -or -not (Test-Path (Join-Path $payload[0].FullName 'bin'))) {
        Write-Error "unexpected zip layout in $($Zip.Name)"
        exit 1
    }
    $Payload = $payload[0].FullName

    Write-Host "Installing $($Zip.BaseName) -> $SbHome"

    New-Item -ItemType Directory -Force -Path `
        "$SbHome\bin", "$SbHome\messages", "$SbHome\documents" | Out-Null

    # --- Binary ------------------------------------------------------
    Copy-Item "$Payload\bin\sb.exe" "$SbHome\bin\sb.exe" -Force

    # --- Migrate pre-0.1.31 flat layout into the style tree -----------
    # Before styles, everything lived in <SbHome>\message_definitions.
    # Those namespaces are all ROS2-side (std/ included), so they move to
    # messages\ros2\. Runs before the payload copy so user files are the
    # ones preserved when both exist. message_targets/ is regenerated by
    # `sb message compile`, so it is dropped rather than moved.
    $legacyDefs = Join-Path $SbHome 'message_definitions'
    $legacyTgts = Join-Path $SbHome 'message_targets'
    if (Test-Path $legacyDefs) {
        $ros2Defs = Join-Path $SbHome 'messages\ros2\message_definitions'
        New-Item -ItemType Directory -Force -Path $ros2Defs | Out-Null
        $moved = 0
        foreach ($f in Get-ChildItem -Path $legacyDefs -Recurse -File) {
            $rel  = $f.FullName.Substring($legacyDefs.Length).TrimStart('\')
            # .cache/ holds the generated descriptor set - regenerated, not moved.
            if ($rel -like '.cache\*') { continue }
            $dest = Join-Path $ros2Defs $rel
            if (Test-Path $dest) { continue }
            $parent = Split-Path $dest -Parent
            if (-not (Test-Path $parent)) { New-Item -ItemType Directory -Force -Path $parent | Out-Null }
            Copy-Item $f.FullName $dest
            $moved++
        }
        Remove-Item $legacyDefs -Recurse -Force
        Write-Host "  migrated: $moved file(s) message_definitions\ -> messages\ros2\message_definitions\"
    }
    if (Test-Path $legacyTgts) {
        Remove-Item $legacyTgts -Recurse -Force
        Write-Host "  migrated: dropped stale message_targets\ (regenerated by 'sb message compile')"
    }

    # --- Message styles - NEVER overwrite user edits -----------------
    $src = Join-Path $Payload 'messages'
    $kept = 0; $copied = 0
    foreach ($f in Get-ChildItem -Path $src -Recurse -File) {
        $rel  = $f.FullName.Substring($src.Length).TrimStart('\')
        $dest = Join-Path "$SbHome\messages" $rel
        if (Test-Path $dest) { $kept++; continue }
        $parent = Split-Path $dest -Parent
        if (-not (Test-Path $parent)) { New-Item -ItemType Directory -Force -Path $parent | Out-Null }
        Copy-Item $f.FullName $dest
        $copied++
    }
    $styles = (Get-ChildItem "$SbHome\messages" -Directory | ForEach-Object { $_.Name }) -join ', '
    Write-Host "  messages: $copied new, $kept preserved  (styles: $styles)"

    # --- Documents - replace wholesale (reference docs ship verbatim) --
    if (Test-Path "$SbHome\documents") { Remove-Item "$SbHome\documents" -Recurse -Force }
    Copy-Item "$Payload\documents" "$SbHome\documents" -Recurse

    Copy-Item "$Payload\VERSION" "$SbHome\VERSION" -Force

    # --- Seed config only if absent ----------------------------------
    $cfg = Join-Path $SbHome 'sb.config.yml'
    $tpl = Join-Path $Payload 'sb.config.yml.template'
    if (-not (Test-Path $cfg) -and (Test-Path $tpl)) {
        # hostname.exe exists on Windows but rejects -s; COMPUTERNAME is the
        # short name we actually want.
        $hostShort = $env:COMPUTERNAME
        # Forward slashes: backslashes are literal in a plain YAML scalar, but
        # this dodges the question entirely and Windows accepts them.
        $homeYaml = $SbHome -replace '\\', '/'
        $seeded = (Read-Utf8 -Path $tpl).
            Replace('__HOSTNAME__', $hostShort).
            Replace('__SB_HOME__', $homeYaml)
        Write-Utf8NoBom -Path $cfg -Text $seeded
        Write-Host "  seeded $cfg (device: $hostShort)"
    }
    elseif (Test-Path $cfg) {
        # --- Migrate an existing pre-0.1.31 config -------------------
        # A config written before styles pins the flat vault paths
        # explicitly, and an explicit value BEATS the derived style path.
        # The tree migration above just deleted those directories, so
        # leaving the pins in place would point sb at nothing.
        #
        # Only the legacy self-referential values are dropped. A path
        # pointing anywhere else is a deliberate external pin and is left
        # exactly as the user wrote it.
        $homeYaml  = $SbHome -replace '\\', '/'
        $lines     = @((Read-Utf8 -Path $cfg) -split "`r?`n")
        $legacyRe  = '^\s*message_(definitions|targets)\s*:\s*(.+?)\s*$'
        $dropped   = 0
        $kept      = New-Object System.Collections.Generic.List[string]
        foreach ($line in $lines) {
            if ($line -match $legacyRe) {
                $val = $Matches[2].Trim().Trim('"').Trim("'") -replace '\\', '/'
                $legacy = "$homeYaml/message_$($Matches[1])"
                if ($val -eq $legacy) { $dropped++; continue }
            }
            $kept.Add($line)
        }
        $body = ($kept -join "`r`n")
        $added = @()
        if ($body -notmatch '(?m)^\s*messages_root\s*:') {
            $added += "messages_root: $homeYaml/messages"
        }
        if ($body -notmatch '(?m)^\s*message_style\s*:') {
            # ros2 rather than the built-in default swarmbotix: swarmbotix
            # ships empty, and this box's messages just migrated into ros2.
            $added += 'message_style: ros2'
        }
        if ($dropped -gt 0 -or $added.Count -gt 0) {
            Copy-Item $cfg "$cfg.bak" -Force
            if ($added.Count -gt 0) {
                $body = $body.TrimEnd() + "`r`n`r`n# --- Message styles (added by the 0.1.31 installer) ---`r`n" +
                        ($added -join "`r`n") + "`r`n"
            }
            Write-Utf8NoBom -Path $cfg -Text $body
            Write-Host "  config: dropped $dropped stale vault pin(s), added $($added.Count) style key(s); backup at $cfg.bak"
        }
    }

    # --- Uninstaller -------------------------------------------------
    # Same portability contract as this script: ASCII only, no #requires.
    $uninstall = @'
# Remove the sb binary and the PATH entry.
# -Purge also deletes $SB_HOME (config + messages); prompts first.
[CmdletBinding()] param([switch]$Purge)
$ErrorActionPreference = 'Stop'
$SbHome = if ($env:SB_HOME) { $env:SB_HOME } else { Join-Path $env:USERPROFILE '.swarmbotix' }
Remove-Item "$SbHome\bin\sb.exe" -Force -ErrorAction SilentlyContinue
$binDir = Join-Path $SbHome 'bin'
$user   = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($user) {
    $parts = @($user -split ';' | Where-Object { $_ -and $_ -ne $binDir })
    [Environment]::SetEnvironmentVariable('Path', ($parts -join ';'), 'User')
    Write-Host "removed $binDir from user PATH"
}
if ($Purge) {
    $ans = Read-Host "Delete $SbHome entirely (config + messages)? [y/N]"
    if ($ans -in @('y','Y','yes','YES')) {
        Remove-Item $SbHome -Recurse -Force; Write-Host "removed $SbHome"
    } else { Write-Host "kept $SbHome" }
}
'@
    Write-Utf8NoBom -Path "$SbHome\uninstall.ps1" -Text $uninstall

    # --- PATH wiring (default prefix only) ---------------------------
    $binDir = Join-Path $SbHome 'bin'
    if (-not $UsingDefaultHome) {
        Write-Host "Custom SB_HOME - skipping PATH edit. Add manually:"
        Write-Host "  $binDir"
    } else {
        # Read 'User', write 'User'. NEVER build the new value from $env:Path -
        # that is the merged machine+user+session string, and writing it back
        # into the User scope permanently duplicates every machine entry.
        $user  = [Environment]::GetEnvironmentVariable('Path', 'User')
        $parts = @($user -split ';' | Where-Object { $_ })
        if ($parts -contains $binDir) {
            Write-Host "  PATH already configured"
        } else {
            $doEdit = $false
            if ($Yes) {
                $doEdit = $true
            } else {
                $ans = Read-Host "Add $binDir to your user PATH? [Y/n]"
                if ($ans -eq '' -or $ans -in @('y','Y','yes','YES')) { $doEdit = $true }
            }
            if ($doEdit) {
                [Environment]::SetEnvironmentVariable('Path', (($parts + $binDir) -join ';'), 'User')
                Write-Host "  added $binDir to your user PATH (open a new terminal to pick it up)"
            } else {
                Write-Host "Add this to your PATH manually:"
                Write-Host "  $binDir"
            }
        }
    }
    # Make sb resolvable for the compile step below regardless of the above.
    $env:Path = "$env:Path;$binDir"

    # --- Compile the message vault -----------------------------------
    # Non-fatal: if protoc/flatc are missing or a message fails, print the
    # error but let the install succeed - `sb doctor` says what's missing.
    Write-Host ""
    # Compile EVERY installed style, not just the active one. The payload
    # ships definitions for all of them, and a style whose bindings were
    # never generated looks broken the moment someone switches to it.
    Write-Host "Compiling message vault..."
    Push-Location $SbHome
    try {
        $env:SB_HOME   = $SbHome
        $env:SB_CONFIG = Join-Path $SbHome 'sb.config.yml'
        $styles = @(Get-ChildItem "$SbHome\messages" -Directory -ErrorAction SilentlyContinue |
                    Where-Object { Test-Path "$($_.FullName)\message_definitions" } |
                    ForEach-Object { $_.Name })
        if (-not $styles) { $styles = @($null) }   # no style tree: one default pass
        $failed = @()
        foreach ($s in $styles) {
            if ($s) {
                Write-Host "  style $s"
                & "$SbHome\bin\sb.exe" --style $s message compile
            } else {
                & "$SbHome\bin\sb.exe" message compile
            }
            if ($LASTEXITCODE -ne 0) { $failed += $s }
        }
        if ($failed.Count -eq 0) {
            Write-Host "  message vault compiled"
        } else {
            Write-Warning "message vault did not fully compile for: $($failed -join ', ')"
            Write-Warning "         run 'sb doctor' then re-run 'sb message compile'."
        }
    } finally {
        Pop-Location
        $global:LASTEXITCODE = 0
    }

    Write-Host ""
    Write-Host "Installed. Try:"
    Write-Host "  sb --version"
    Write-Host "  sb doctor"
}
finally {
    # Best-effort. A recursive delete right after Expand-Archive can lose a
    # race with the indexer / AV and return "Access is denied". Note that
    # -ErrorAction SilentlyContinue is NOT enough here: it suppresses the
    # message but the failure still propagates under $ErrorActionPreference
    # = 'Stop'. Promote to terminating and swallow.
    try {
        Remove-Item $Tmp -Recurse -Force -ErrorAction Stop
    } catch {
        Write-Host "  note: could not remove temp dir $Tmp (safe to delete later)"
    }
}
