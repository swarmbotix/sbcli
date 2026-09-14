#requires -Version 7
<#
.SYNOPSIS
    Stage a swarmbotix Windows release into platforms/windows/dist/<version>/.

.DESCRIPTION
    Windows counterpart of the staging steps in installguide_ubuntu.md Part 2.
    Reads the target platform from .env at the repo root (never from $IsWindows)
    and every build fact from platforms/<os>/version.json.

    Produces exactly two files:
        <package_stem>.zip
        <package_stem>.zip.sha256

    install.ps1 and uninstall.ps1 go INSIDE the zip, at the top of the
    package. Nothing loose is staged beside the archive: one download is the
    whole product, and there is no second copy of the installer to drift.

.PARAMETER SkipBuild
    Reuse whatever is already at target/<triple>/<profile>/; do not run cargo.

.PARAMETER NoZip
    Stop after staging the payload directory. Useful for inspecting the tree.

.EXAMPLE
    pwsh -File platforms\windows\dist-tooling\build.ps1
#>
[CmdletBinding()]
param(
    [switch]$SkipBuild,
    [switch]$NoZip
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# ─────────────────────────────────────────────────────────────────────
# .env reader — the ONLY platform switch. See installguide_windows.md §2.
# ─────────────────────────────────────────────────────────────────────
function Read-DotEnv {
    param([string]$Path)
    $map = @{}
    if (-not (Test-Path $Path)) { throw "no .env at $Path (copy .env.example)" }
    foreach ($line in Get-Content $Path) {
        if ($line -match '^\s*#') { continue }
        if ($line -match '^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$') {
            $val = $Matches[2] -replace '\s+#.*$', ''
            $map[$Matches[1]] = $val.Trim().Trim('"').Trim("'")
        }
    }
    return $map
}

# platforms/windows/dist-tooling/ → repo root is three levels up.
$Root   = (Resolve-Path "$PSScriptRoot\..\..\..").Path
$DotEnv = Read-DotEnv "$Root\.env"

if (-not $DotEnv.ContainsKey('OS')) { throw "OS not set in $Root\.env" }
$OS = $DotEnv['OS']
if ($OS -ne 'windows') {
    throw "build.ps1 is the windows staging script, but .env has OS=$OS. " +
          "Set OS=`"windows`" or use the linux flow in installguide_ubuntu.md."
}

# ─────────────────────────────────────────────────────────────────────
# 0. Resolve everything from .env + this platform's descriptor
# ─────────────────────────────────────────────────────────────────────
$DistRoot = if ($DotEnv.ContainsKey('DIST_ROOT')) { $DotEnv['DIST_ROOT'] } else { 'platforms' }
$Tooling  = Join-Path $Root "$DistRoot\$OS\dist-tooling"
$DescPath = Join-Path $Root "$DistRoot\$OS\version.json"
if (-not (Test-Path $DescPath)) { throw "missing platform descriptor $DescPath" }
$desc = Get-Content $DescPath -Raw | ConvertFrom-Json

$Version = $desc.version
$Triple  = if ($DotEnv.ContainsKey('TARGET_TRIPLE')) { $DotEnv['TARGET_TRIPLE'] } else { $desc.triple }
$Profile = if ($DotEnv.ContainsKey('PROFILE'))       { $DotEnv['PROFILE'] }       else { 'release' }
$Exe     = $desc.binary
$Pkg     = $desc.package_stem
$Dist    = Join-Path $Root "$DistRoot\$OS\dist\$Version"
$Stage   = Join-Path $Dist $Pkg

# Drift check. The descriptor MIRRORS the version; Cargo.toml owns it.
# A bundle whose zip name disagrees with the binary inside it is worse than
# no bundle, so this is fatal rather than a warning. See /sb-release.
$meta   = cargo metadata --format-version 1 --no-deps --manifest-path "$Root\Cargo.toml" | ConvertFrom-Json
$CargoV = ($meta.packages | Where-Object name -eq 'sb-cli').version
if ($Version -ne $CargoV) {
    throw "version drift: $DescPath says $Version, Cargo.toml says $CargoV — run /sb-release"
}
$wantStem = "$($desc.product)-$Version-$($desc.arch)"
if ($Pkg -ne $wantStem) {
    throw "package_stem drift: descriptor says $Pkg, expected $wantStem — run /sb-release"
}

Write-Host "swarmbotix $Version  ($($desc.arch), $Triple, $Profile)"

# ─────────────────────────────────────────────────────────────────────
# 1. Build
# ─────────────────────────────────────────────────────────────────────
if (-not $SkipBuild) {
    # Always with an explicit --target, so the artifact lands in the
    # per-triple dir and a bare `cargo build --release` can never be
    # mistaken for a staged one. installguide_windows.md §5.
    cargo build --profile $Profile --target $Triple --bin sb --manifest-path "$Root\Cargo.toml"
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
}
$BinSrc = Join-Path $Root "target\$Triple\$Profile\$Exe"
if (-not (Test-Path $BinSrc)) {
    throw "missing $BinSrc — drop -SkipBuild, or check the profile/triple"
}

# ─────────────────────────────────────────────────────────────────────
# 2. Clean ONLY this version slot (never the sibling platform)
# ─────────────────────────────────────────────────────────────────────
if (Test-Path $Dist) { Remove-Item $Dist -Recurse -Force }
New-Item -ItemType Directory -Force -Path `
    "$Stage\bin", "$Stage\messages", "$Stage\documents" | Out-Null

# ─────────────────────────────────────────────────────────────────────
# 3. Binary
# ─────────────────────────────────────────────────────────────────────
Copy-Item $BinSrc "$Stage\bin\$Exe"
# sb.pdb is deliberately NOT shipped — archive it out of band if you want
# to symbolicate crash dumps. installguide_windows.md §5.

# ─────────────────────────────────────────────────────────────────────
# 4. Message styles — the whole messages/ tree, one subdir per style,
#    each with its own message_definitions/. Same exclusions as the Linux
#    rsync call. `custom/` IS staged now — it is part of the swarmbotix
#    style's scaffold rather than a user artifact, and ships empty.
#    message_targets/ is generated output and is never staged.
# ─────────────────────────────────────────────────────────────────────
robocopy "$Root\messages" "$Stage\messages" /E /NJH /NJS /NFL /NDL `
    /XD '.cache' 'targets' 'message_targets' | Out-Null
# robocopy exit codes 0-7 are success; >= 8 is a real failure.
if ($LASTEXITCODE -ge 8) { throw "robocopy failed ($LASTEXITCODE)" }
$global:LASTEXITCODE = 0

# Every shipped style must carry a message_definitions/ or the installer
# has nothing to place. Catches a style added to the repo without one.
foreach ($s in Get-ChildItem "$Stage\messages" -Directory) {
    if (-not (Test-Path "$($s.FullName)\message_definitions")) {
        throw "style '$($s.Name)' staged without a message_definitions/ directory"
    }
}

# ─────────────────────────────────────────────────────────────────────
# 5. Reference docs
# ─────────────────────────────────────────────────────────────────────
Copy-Item "$Root\documents\*" "$Stage\documents\" -Recurse -Force

# ─────────────────────────────────────────────────────────────────────
# 6. Config template — the Windows flavor, from this platform's slot
# ─────────────────────────────────────────────────────────────────────
Copy-Item "$Tooling\sb.config.win.yml.template" "$Stage\sb.config.yml.template"

# ─────────────────────────────────────────────────────────────────────
# 7. Installer + uninstaller, verbatim from this platform's dist-tooling/.
#    They ship INSIDE the package: the user extracts one zip and runs the
#    install.ps1 that comes out of it, and $SB_HOME gets that same
#    uninstall.ps1 placed in it.
# ─────────────────────────────────────────────────────────────────────
Copy-Item "$Tooling\install.ps1"   "$Stage\install.ps1"
Copy-Item "$Tooling\uninstall.ps1" "$Stage\uninstall.ps1"

# ─────────────────────────────────────────────────────────────────────
# 8. VERSION stamp
# ─────────────────────────────────────────────────────────────────────
# utf8NoBOM: PowerShell's default UTF-8-with-BOM would put \xEF\xBB\xBF in
# front of `version:` and break anything that matches on the first line.
@"
version:    $Version
built:      $(Get-Date -Format 'yyyy-MM-dd')
platform:   $($desc.arch)
binary:     sb $Version
"@ | Set-Content "$Stage\VERSION" -Encoding utf8NoBOM

if ($NoZip) {
    Write-Host "staged (not zipped): $Stage"
    return
}

# ─────────────────────────────────────────────────────────────────────
# 9. Zip, drop the staging dir
# ─────────────────────────────────────────────────────────────────────
# -Path $Stage (not $Stage\*) so the package dir is the zip's single
# top-level entry — matches `zip -rq $PKG.zip $PKG` on Linux, and is what
# install.ps1 asserts on. Passing $Stage\* flattens the payload and
# silently breaks the installer's layout check.
Compress-Archive -Path $Stage -DestinationPath "$Dist\$Pkg.zip" -Force
Remove-Item $Stage -Recurse -Force

# ─────────────────────────────────────────────────────────────────────
# 10. Checksum next to the zip
# ─────────────────────────────────────────────────────────────────────
(Get-FileHash "$Dist\$Pkg.zip" -Algorithm SHA256).Hash.ToLower() |
    Set-Content "$Dist\$Pkg.zip.sha256" -Encoding ascii

Write-Host ""
Write-Host "staged → $Dist"
Get-ChildItem $Dist | Format-Table Name, @{ n = 'MB'; e = { [math]::Round($_.Length / 1MB, 2) } }
