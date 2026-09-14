# swarmbotix — Windows build & distribution pattern

**Status: landed and verified end to end.** The layout in §6 exists, the scripts
in §8 are written, and the whole path has been exercised on Windows 11 /
`x86_64-pc-windows-msvc`: `cargo test --workspace` → **281 passed, 0 failed**;
`build.ps1` produces a 9.5 MB signed-off zip; `install.ps1` installs it into a
sandbox `SB_HOME` and `sb.exe --version` / `sb.exe doctor` run from it. See §9
for what that took and §12 for what is left.

Companion to [installguide_ubuntu.md](installguide_ubuntu.md), which is the Linux baseline —
everything here is stated as a delta against it. The two share the canonical
version (`Cargo.toml`), the payload shape, and the on-disk install layout; they
differ only in toolchain, shell, packaging primitives, and their own
per-platform `version.json` descriptor.

---

## 1. How the Linux build works today

The whole pipeline is a hand-run bash snippet in
[installguide_ubuntu.md](installguide_ubuntu.md) §"Build steps" — there is no `Makefile`, no
`build.sh`, no CI. Six inputs, one zip, two output files.

```
Cargo.toml [workspace.package] version  ──→  CANONICAL ──┐
                                                         │ drift check
platforms/linux/version.json  ──jq──┐                    │
                                    ├─→ VERSION / PKG / ARCH / TRIPLE / BIN
.env  ──────────────────────────────┘                    │
                                                         ▼
cargo build --release --target <triple>  ──→  target/<triple>/release/sb ──┐
messages/             ──rsync (−.cache −targets −message_targets)─────────┤
documents/            ──cp -r ────────────────────────────────────────────┼─→ platforms/linux/dist/<ver>/<pkg>/
platforms/linux/dist-tooling/sb.config.yml.template ──cp ─────────────────┤          │
generated VERSION stamp ──────────────────────────────────────────────────┘          │
                                                                                     ▼
                                                                     zip -rq <pkg>.zip <pkg>
                                                                     + install.sh (verbatim copy)
```

**Release output** — `platforms/<os>/dist/<version>/` holds exactly two shipped
files: the payload zip and a version-agnostic installer that finds its sibling
zip by glob. Old version dirs are kept side by side; the staging step only
clears the `<version>/` slot it is building.

**The installer**
([platforms/linux/dist-tooling/install.sh](platforms/linux/dist-tooling/install.sh)) extracts
the zip into `$SB_HOME` (default `~/.swarmbotix`), never overwrites user-edited
`.proto` files, replaces `documents/` wholesale, seeds `sb.config.yml` only when
absent, wires `~/.bashrc`, writes an `uninstall.sh`, and finishes by running
`sb message compile` to populate the vault's generated bindings.

### What in that pipeline is actually Linux-specific

| Layer | Linux-ism | Windows equivalent |
|---|---|---|
| Build host | `jq`, `rsync`, `zip`, `install -m 0755` | `ConvertFrom-Json`, `robocopy`, `Compress-Archive`, `Copy-Item` |
| Binary | `target/<triple>/release/sb` | `target/<triple>/release/sb.exe` |
| Naming | was `platform: "linux-x86_64"` in a root `version.json` | per-platform descriptor `platforms/<os>/version.json`, selected by `.env` — **landed** |
| Dist slot | `platforms/linux/dist/<ver>/` | `platforms/windows/dist/<ver>/` |
| Installer | `install.sh` — `readlink -f`, `mktemp -d`, `unzip`, `sed -i`, `hostname -s` | `install.ps1` |
| PATH wiring | append to `~/.bashrc` | user-scoped `Path` env var |
| Native libs | `libzenohc.so`, `libiceoryx2_ffi_c.so` | `zenohc.dll`, `iceoryx2_ffi_c.dll` |
| Multiplexer | `tmux` | `psmux` (`winget install psmux`) — **already handled**, `sb` only ever invokes `tmux` by name |

The **Rust source is already portable.** `set_executable` is `#[cfg(unix)]` with
a no-op `#[cfg(not(unix))]` twin
([sb-workspace/src/lib.rs:443-459](crates/sb-workspace/src/lib.rs#L443-L459)),
and `sb-launch` deliberately resolves `tmux` through `which` rather than
`/usr/bin/tmux` ([sb-launch/src/lib.rs:225-246](crates/sb-launch/src/lib.rs#L225-L246)).
The gaps are in the *tooling*, not the crates — with the exceptions listed in
§9.

---

## 2. The `.env` switch

`.env` at the repo root is the single source of truth for which platform the
build tooling targets. Every build/stage/package step reads it first and derives
everything else; **no script may branch on `$OSTYPE`, `uname`, or
`$IsWindows`.**

### Contract

```dotenv
# .env — repo-root build switch. Not committed; see .env.example.
OS="windows"                              # windows | linux | mac   (required)

# Optional overrides. Omit to accept the per-OS default from §3.
TARGET_TRIPLE="x86_64-pc-windows-msvc"    # cross-compile target
PROFILE="release"                         # cargo profile to stage
DIST_ROOT="platforms"                     # release output root
```

Release output resolves to `<DIST_ROOT>/<OS>/dist/<version>/`.

Only `OS` is required. Values are lowercase; quotes optional and stripped by the
reader. Unknown keys are ignored (forward-compatible). An `OS` value outside
`{windows, linux, mac}` is a hard error — never a silent fallback to the host.

### Housekeeping — done

`/.env` is in [.gitignore](.gitignore) (per-dev-box state), and
[.env.example](.env.example) carries the block above with `OS="linux"` so a
fresh clone has something to copy.

### Readers

PowerShell — used by `build.ps1` / `install.ps1`:

```powershell
function Read-DotEnv([string]$Path = "$PSScriptRoot\..\..\..\.env") {
    $map = @{}
    if (Test-Path $Path) {
        foreach ($line in Get-Content $Path) {
            if ($line -match '^\s*#') { continue }
            if ($line -match '^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$') {
                $map[$Matches[1]] = $Matches[2].Trim().Trim('"').Trim("'")
            }
        }
    }
    return $map
}
```

Bash — the same three lines for the Linux path, so both sides agree on parsing:

```bash
sb_dotenv() {                       # usage: eval "$(sb_dotenv "$ROOT/.env")"
    sed -E 's/[[:space:]]*#.*$//; /^[[:space:]]*$/d; s/^[[:space:]]*//' "$1" \
      | grep -E '^[A-Za-z_][A-Za-z0-9_]*='
}
```

---

## 3. Platform resolution table

Everything the tooling needs, derived from `OS` alone:

| `.env` `OS` | Default triple | Binary | `arch` tag | Dist slot | Installer | Install root |
|---|---|---|---|---|---|---|
| `linux` | `x86_64-unknown-linux-gnu` | `sb` | `linux-x86_64` | `platforms/linux/dist/<ver>/` | `install.sh` | `$HOME/.swarmbotix` |
| `windows` | `x86_64-pc-windows-msvc` | `sb.exe` | `windows-x86_64` | `platforms/windows/dist/<ver>/` | `install.ps1` | `%USERPROFILE%\.swarmbotix` |
| `mac` | `aarch64-apple-darwin` | `sb` | `macos-arm64` | `platforms/mac/dist/<ver>/` | `install.sh` | `$HOME/.swarmbotix` |

Package stem is always `swarmbotix-<version>-<arch>`, so Windows v0.1.41 is
`swarmbotix-0.1.41-windows-x86_64`.

`%USERPROFILE%\.swarmbotix` is not a free choice — it is what
`dirs::home_dir()` returns on Windows, which is what `sb` itself uses to find
its home. §6 "Install layout on the target machine" has the full tree, the two
ways to override the location, and a warning about leftover `.swarmbotix`
directories on boxes that ran the test suite before the §9.4 fix.

---

## 4. Prerequisites on a Windows build box

| Requirement | Why | Check |
|---|---|---|
| Rust stable, `x86_64-pc-windows-msvc` | host toolchain; pinned by [rust-toolchain.toml](rust-toolchain.toml) | `rustc -vV` → `host: x86_64-pc-windows-msvc` |
| **VS Build Tools** — MSVC v143 + Windows 11 SDK | `iceoryx2-pal-posix` compiles a C POSIX shim through `cc`; zenoh links against system crypto | `cl.exe` resolvable from a Developer prompt |
| PowerShell 7+ | **`build.ps1` only** — it uses `??`. `install.ps1` deliberately runs on 5.1 too; see below | `$PSVersionTable.PSVersion` |
| Long paths enabled | deep `target/<triple>/build/...` trees exceed `MAX_PATH` | `HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem\LongPathsEnabled = 1` |

No `jq`, `rsync`, `zip`, or Git Bash is needed to *build* — that is the point of
`build.ps1`. Git Bash **is** needed at runtime to execute generated
`runscript.bash` files; see §9.5.

**The build box and the target box have different floors.** `build.ps1` is
maintainer-side and may assume pwsh 7. `install.ps1` runs on whatever an end
user already has, which on a stock Windows box is `powershell.exe` — Windows
PowerShell **5.1**. So the installer carries no `#requires` and must stay clear
of 6+/7+ syntax. See §8.2 for the three traps that enforce.

---

## 5. `target/` folder structure

`target/` stays at the repo root and is **not** split under `platforms/`. So
does `crates/`, and so does the single workspace `Cargo.toml`. The reason is
§9.1: the crates compile identically on both platforms, so a per-platform source
tree would be two copies of the same 11.5k lines with nothing platform-specific
in either. Cargo already gives us the separation we need, keyed by target triple
rather than by directory.

Cargo owns this tree; the job of the convention is to make the staging step's
source path unambiguous.

**Rule: always build with an explicit `--target <triple>`.** That forces Cargo
into the per-triple layout on every OS, so the staging step reads exactly one
path and never guesses.

```
target/                                        ← gitignored in full (/target)
├── CACHEDIR.TAG
├── debug/                                     ← host-artifact spill (build scripts,
├── release/                                     proc-macros). NEVER staged.
│
├── x86_64-pc-windows-msvc/                    ← OS="windows"
│   ├── debug/
│   └── release/
│       ├── sb.exe                             ← ✅ STAGE SOURCE
│       ├── sb.pdb                             ← symbols: archive, do not ship
│       ├── build/ deps/ incremental/ .fingerprint/
│       └── examples/
│
├── x86_64-unknown-linux-gnu/                  ← OS="linux"
│   └── release/
│       └── sb                                 ← ✅ STAGE SOURCE
│
└── aarch64-unknown-linux-gnu/                 ← reserved (Jetson / Pi)
    └── release/sb
```

The staging step resolves its source as:

```
target/<TARGET_TRIPLE>/<PROFILE>/sb[.exe]
```

and **never** `target/release/`. That one rule is what keeps a bare
`cargo build --release` (which writes `target/release/`) from being mistaken for
a staged artifact.

`sb.pdb` sits next to `sb.exe` (measured at v0.1.29: 27.2 MB exe, 9.8 MB pdb).
It is deliberately **not** in the payload — copy it to an out-of-band symbol
archive keyed by version if you want to symbolicate crash dumps later.

> **If one checkout is ever shared between Windows and WSL** (same directory on
> an NTFS mount), the per-triple dirs still collide on the shared `target/debug`
> host-artifact spill and Cargo's fingerprint DB will thrash. In that case set
> `CARGO_TARGET_DIR=target/win` vs `target/linux` from `.env` instead. This repo
> is a Windows-side clone, so the simple rule above is sufficient today.

---

## 6. `platforms/` — packaging layout

Packaging is the one thing that genuinely differs per OS, so it gets the
directory split that the source tree does not. The old
`dist/<version>/<platform>/` collapses by one level: the platform is now in the
path, so repeating it inside `dist/` would be redundant.

```
platforms/                                            ← committed (only /target is ignored)
├── README.md                                         ← the layout + its rules
│
├── linux/
│   ├── dist-tooling/                                 ← installer SOURCE, long-lived
│   │   ├── install.sh
│   │   └── sb.config.yml.template                    ← .so paths
│   └── dist/                                         ← release OUTPUT, one dir per version
│       ├── 0.1.41/
│       │   ├── swarmbotix-0.1.41-linux-x86_64.zip
│       │   ├── swarmbotix-0.1.41-linux-x86_64.zip.sha256
│       │   └── install.sh                            ← verbatim from dist-tooling/
│       └── 0.1.28/ …                                 ← previous cuts kept side by side
│
├── windows/
│   ├── dist-tooling/                                 ← specified in §8, not yet written
│   │   ├── build.ps1
│   │   ├── install.ps1
│   │   └── sb.config.win.yml.template                ← .dll paths
│   └── dist/
│       └── 0.1.41/
│           ├── swarmbotix-0.1.41-windows-x86_64.zip  ← the payload
│           ├── swarmbotix-0.1.41-windows-x86_64.zip.sha256
│           └── install.ps1                           ← verbatim from dist-tooling/
│
└── mac/                                              ← not created yet; add as a sibling
```

The `dist-tooling/` ↔ `dist/` pairing is the load-bearing distinction: the first
is checked-in source that outlives any release, the second is generated output
that accumulates one directory per cut.

Invariants, carried over from the Linux flow:

- **Exactly two shipped files per version** (three with the checksum) — the
  installer is version-agnostic and locates its sibling zip by glob, so the pair
  can be handed over as-is.
- **Staging clears only the `<version>/` slot it is building.** Cutting Windows
  never touches `platforms/linux/`, so both platforms of one version can be
  built on different boxes and merged by commit.
- **`dist/` is committed, `target/` is not** — release artifacts are the record
  of what shipped.
- `.sha256` is currently a known gap on Linux. Add it on **both** sides at the
  same time rather than letting the platforms diverge.

### Payload interior (inside the zip)

Identical shape to Linux — one top-level dir named after the package stem, which
is what `install.ps1` asserts on:

```
swarmbotix-0.1.41-windows-x86_64/
├── bin/
│   └── sb.exe                        ← from target/x86_64-pc-windows-msvc/release/
├── messages/                         ← one subdir per style; minus .cache/, targets/, message_targets/
│   ├── std/  std_msgs/  geometry_msgs/  sensor_msgs/  …
├── documents/                        ← verbatim from documents/
├── sb.config.yml.template            ← the WINDOWS flavor (§8.3)
├── VERSION                           ← generated stamp
└── lib/                              ← RESERVED — see §10, open decision
```

### Install layout on the target machine

The Linux `~/.swarmbotix` becomes **`%USERPROFILE%\.swarmbotix`** — e.g.
`C:\Users\you\.swarmbotix`.

Same dotted-directory-in-home convention, **not** `AppData`. That is forced, not
chosen: `dirs::home_dir()` on Windows calls
`SHGetKnownFolderPath(FOLDERID_Profile)`, which returns `%USERPROFILE%`, and
that is what `sb` itself uses to locate its home
([sb-config/src/lib.rs:33-60](crates/sb-config/src/lib.rs#L33-L60)). An
installer that put the payload anywhere else would produce a tree `sb` cannot
find.

```
C:\Users\you\.swarmbotix\
├── bin\sb.exe                        ← this dir is what goes on the user PATH
├── sb.config.yml                     ← seeded from template only if absent
├── VERSION
├── uninstall.ps1                     ← written by install.ps1
├── messages\                         ← message styles; user edits never clobbered
│   ├── ros2\                         ←   std/* + ROS2 vault (138 .proto)
│   │   ├── message_definitions\
│   │   └── message_targets\          ←   generated iox2/ proto/ fb/ bindings
│   └── swarmbotix\                   ←   main style; own defs
│       └── message_definitions\
├── documents\                        ← replaced wholesale on upgrade
├── workspaces\                       ← created by `sb ws create`
└── active                            ← created by `sb ws set`
```

Byte-for-byte the same tree as Linux, under a different root. Nothing in the
CLI branches on OS to find it — `resolve_sb_home_dir` handles it.

#### Overriding the location

| When | How | Effect |
|---|---|---|
| At install time | `$env:SB_HOME='D:\sb'` before running `install.ps1` | Installs under `D:\sb`. The installer skips PATH wiring in this mode and prints the directory to add yourself. Same rule as `install.sh`. |
| After install | `sb_home_dir:` in `sb.config.yml` | `resolve_sb_home_dir` honors it ahead of `dirs::home_dir()`. This is the same knob the test sandboxes use to stay hermetic (§9.4). |

To confirm what a given machine actually resolved:

```powershell
sb config show          # `# config file:` and `# documents:` headers name the paths
sb config show --json   # _config_file / _documents as JSON fields
```

#### A `.swarmbotix` may already exist on a dev box

Before the §9.4 fix, the integration-test sandboxes isolated via `$HOME` — which
does nothing on Windows. Any `cargo test` run on Windows prior to that fix wrote
into the developer's **real** `%USERPROFILE%\.swarmbotix`, typically leaving a
stray `messages\`.

That matters at install time because the installer **preserves** existing
message-definition files rather than overwriting them (deliberately — it is what
protects user edits across upgrades). So a half-populated leftover tree survives
the install and can shadow the shipped vault. On a box that ever ran the tests
pre-fix, inspect it first:

```powershell
Get-ChildItem "$env:USERPROFILE\.swarmbotix"
```

If it is leftover test debris rather than a real install, delete it before
installing. A genuine install has `bin\sb.exe`, `VERSION`, and `sb.config.yml`
alongside the vault; test debris usually has `messages\` and nothing
else.

---

## 7. The Windows tooling

All three files exist:

```
platforms/windows/dist-tooling/
├── build.ps1                         ← the staging script (§8.1)
├── install.ps1                       ← the installer (§8.2)
└── sb.config.win.yml.template        ← .dll paths (§8.3)
```

Cut a release with:

```powershell
pwsh -File platforms\windows\dist-tooling\build.ps1
```

Open question still worth settling: whether to also add a
`build.sh` that formalizes the currently-inline Linux snippet, so both platforms
are scripts rather than one script + one copy-paste block. Recommended — a
copy-paste block in a markdown file cannot be kept honest with `.env`.

---

## 8. Script specifications

### 8.1 `platforms/windows/dist-tooling/build.ps1`

Mirrors installguide_ubuntu.md §"Build steps" step-for-step.

```powershell
#requires -Version 7
[CmdletBinding()] param([switch]$SkipBuild)
$ErrorActionPreference = 'Stop'

# platforms/windows/dist-tooling/ → repo root is three levels up.
$Root = Resolve-Path "$PSScriptRoot\..\..\.."
$env  = Read-DotEnv "$Root\.env"                       # §2
$OS   = $env['OS']; if (-not $OS) { throw "OS not set in .env" }
if ($OS -ne 'windows') { throw "build.ps1 is the windows staging script; .env has OS=$OS" }

# ── 0. Resolve everything from .env + this platform's descriptor ────
$DistRoot = $env['DIST_ROOT'] ?? 'platforms'
$Tooling  = Join-Path $Root "$DistRoot\$OS\dist-tooling"
$desc     = Get-Content (Join-Path $Root "$DistRoot\$OS\version.json") -Raw | ConvertFrom-Json

$Version = $desc.version
$Triple  = $env['TARGET_TRIPLE'] ?? $desc.triple
$Profile = $env['PROFILE']       ?? 'release'
$Exe     = $desc.binary                              # sb.exe — from the descriptor
$Pkg     = $desc.package_stem
$Dist    = Join-Path $Root "$DistRoot\$OS\dist\$Version"
$Stage   = Join-Path $Dist $Pkg

# The descriptor MIRRORS the version; Cargo.toml owns it. Refuse to stage a
# bundle whose zip name would disagree with the binary inside it. (§9.2)
$CargoV = ((cargo metadata --format-version 1 --no-deps | ConvertFrom-Json).packages |
           Where-Object name -eq 'sb-cli').version
if ($Version -ne $CargoV) {
    throw "version drift: descriptor says $Version, Cargo.toml says $CargoV — run /sb-release"
}
if ($Pkg -ne "$($desc.product)-$Version-$($desc.arch)") {
    throw "package_stem $Pkg does not match product/version/arch — run /sb-release"
}

# ── 1. Build ────────────────────────────────────────────────────────
if (-not $SkipBuild) {
    cargo build --profile $Profile --target $Triple --bin sb
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
}
$BinSrc = Join-Path $Root "target\$Triple\$Profile\$Exe"
if (-not (Test-Path $BinSrc)) { throw "missing $BinSrc" }

# ── 2. Clean ONLY this version slot (never the sibling platform) ────
if (Test-Path $Dist) { Remove-Item $Dist -Recurse -Force }
New-Item -ItemType Directory -Force -Path "$Stage\bin", `
    "$Stage\messages", "$Stage\documents" | Out-Null

# ── 3. Binary ───────────────────────────────────────────────────────
Copy-Item $BinSrc "$Stage\bin\$Exe"

# ── 4. Message definitions — same exclusions as the rsync call ──────
robocopy "$Root\messages" "$Stage\messages" /E /NJH /NJS /NFL /NDL `
    /XD '.cache' 'targets' 'custom' | Out-Null
if ($LASTEXITCODE -ge 8) { throw "robocopy failed ($LASTEXITCODE)" }   # 0–7 == success

# ── 5. Reference docs ───────────────────────────────────────────────
Copy-Item "$Root\documents\*" "$Stage\documents\" -Recurse -Force

# ── 6. Config template — the per-OS flavor, from this platform's slot ──
Copy-Item "$Tooling\sb.config.win.yml.template" "$Stage\sb.config.yml.template"

# ── 7. VERSION stamp ────────────────────────────────────────────────
@"
version:    $Version
built:      $(Get-Date -Format 'yyyy-MM-dd')
platform:   $($desc.arch)
binary:     sb $Version
"@ | Set-Content "$Stage\VERSION" -Encoding utf8NoBOM

# ── 8. Zip, drop the staging dir ────────────────────────────────────
Compress-Archive -Path $Stage -DestinationPath "$Dist\$Pkg.zip" -Force
Remove-Item $Stage -Recurse -Force

# ── 9. Installer + checksum next to the zip ─────────────────────────
Copy-Item "$Tooling\install.ps1" "$Dist\install.ps1"
(Get-FileHash "$Dist\$Pkg.zip" -Algorithm SHA256).Hash.ToLower() |
    Set-Content "$Dist\$Pkg.zip.sha256" -Encoding ascii

Get-ChildItem $Dist | Format-Table Name, Length
```

Three details that differ from the bash original and are easy to get wrong:

- **robocopy exit codes 0–7 mean success.** A naive `$LASTEXITCODE -ne 0` check
  fails every run.
- **`Compress-Archive -Path $Stage`** includes the `$Stage` directory itself as
  the zip's single top-level entry — which is what `install.ps1` expects, and
  what `zip -rq "$PKG.zip" "$PKG"` produces on Linux. Passing `$Stage\*` instead
  flattens the payload and silently breaks the installer's layout assertion.
- **`utf8NoBOM`** on the VERSION stamp. PowerShell's default UTF-8-with-BOM puts
  `\xEF\xBB\xBF` in front of `version:`, which would break any parser that
  string-matches the first line.

### 8.2 `platforms/windows/dist-tooling/install.ps1`

Behavioral parity with `install.sh`, step for step. Invoked as:

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1        # interactive
powershell -ExecutionPolicy Bypass -File .\install.ps1 -Yes   # non-interactive
$env:SB_HOME='D:\sb'; .\install.ps1                           # custom prefix
```

#### It must run on Windows PowerShell 5.1 — three traps

A stock Windows box has `powershell.exe` (5.1) and no pwsh 7, so the installer
cannot require 7. Verified by parsing **and executing** it under both editions;
`sb.config.yml` and the generated `uninstall.ps1` come out byte-identical from
5.1 and 7.6.3. Each trap below was a live failure, not a hypothetical:

1. **ASCII only — this one is invisible.** A `.ps1` with no BOM is decoded as
   UTF-8 by pwsh 7 but as the ANSI codepage by 5.1. The `─` in a `# ── Extract ──`
   comment banner is `E2 94 80`, and `0x94` in Windows-1252 is a **smart double
   quote** — a string delimiter to the parser. The file died with `string is
   missing the terminator` pointing at a line 150 lines away from any string.
   Comment decoration is not cosmetic here; keep every byte `< 0x80`.
2. **`-Encoding utf8NoBOM` is 6+.** On 5.1 that value does not exist and plain
   `utf8` writes a BOM. Both writes go through a `Write-Utf8NoBom` helper on
   `[System.IO.File]::WriteAllText` + `UTF8Encoding($false)`, identical on both.
3. **`Get-Content -Raw` is not encoding-neutral** — 7 assumes UTF-8, 5.1 assumes
   the ANSI codepage. Reading the UTF-8 `sb.config.yml.template` on 5.1 mangled
   its `→` arrows, and Windows-1252 slots `0x81 0x8D 0x8F 0x90 0x9D` are
   undefined, arriving as **C1 control characters**. Those landed in the seeded
   config and `sb` rejected it: `control characters are not allowed at position
   334`. A matching `Read-Utf8` helper on `[System.IO.File]::ReadAllText` is the
   exact inverse of the writer.

Also barred, for the same reason: `??`, `?:` ternaries, `&&`/`||` pipeline
chains, and `ForEach-Object -Parallel`. `Expand-Archive` is fine — it ships with
PowerShell 5.0+.

| Step | `install.sh` | `install.ps1` |
|---|---|---|
| Locate payload | glob `swarmbotix-*-linux-x86_64.zip`; error on 0 or >1 | glob `swarmbotix-*-windows-x86_64.zip`; same arity rules |
| Extract | `mktemp -d` + `unzip -q` | `New-TemporaryFile`-derived dir + `Expand-Archive` |
| Assert layout | single top-level dir containing `bin/` | same |
| Install root | `${SB_HOME:-$HOME/.swarmbotix}` | `$env:SB_HOME` else `$env:USERPROFILE\.swarmbotix` |
| Binary | `install -m 0755 … bin/sb` | `Copy-Item … bin\sb.exe` (no mode bit) |
| Messages | `rsync -a --ignore-existing` | per-file loop, skip existing — **user edits are never clobbered** |
| Documents | `rm -rf` then `cp -r` | `Remove-Item -Recurse` then `Copy-Item -Recurse` |
| Seed config | only if absent; `sed s/__HOSTNAME__/$(hostname -s)/` | only if absent; `-replace '__HOSTNAME__', $env:COMPUTERNAME` |
| PATH wiring | append marker line to `~/.bashrc` | user-scoped `Path` env var, marker-free idempotency |
| Uninstaller | writes `uninstall.sh` | writes `uninstall.ps1` |
| Post-install | `sb message compile` from `$SB_HOME` | same, `sb.exe message compile` |

PATH wiring is the one step with no line-by-line analogue. There is no rc file to
append to and no marker comment to grep for; idempotency comes from testing
membership in the split list:

```powershell
$binDir = Join-Path $SbHome 'bin'
$user   = [Environment]::GetEnvironmentVariable('Path', 'User')
$parts  = $user -split ';' | Where-Object { $_ }
if ($parts -notcontains $binDir) {
    [Environment]::SetEnvironmentVariable('Path', (($parts + $binDir) -join ';'), 'User')
    Write-Host "  added $binDir to your user PATH (open a new terminal to pick it up)"
} else {
    Write-Host "  PATH already configured"
}
$env:Path = "$env:Path;$binDir"     # so the post-install compile below works now
```

Two traps worth naming in the script's own comments:

- **Never write the `Machine` scope**, and never build the new value from
  `$env:Path`. `$env:Path` is the *merged* machine+user+session string; writing
  it back into the `User` scope permanently duplicates every machine entry into
  the user's PATH. Read `'User'`, write `'User'`.
- The seeded `sb.config.yml` gets a path like `C:\Users\you\.swarmbotix\…`.
  Emit it **unquoted** (backslashes are literal in a plain YAML scalar) or
  normalize to forward slashes. Emitting it inside double quotes makes YAML
  read `\U` as an escape and the install silently produces an unparseable
  config.

### 8.3 `platforms/windows/dist-tooling/sb.config.win.yml.template`

Same schema, same comments, same `__HOSTNAME__` / `__SB_HOME__` placeholders.
Only the guidance comments for the two native libraries change, since the
field *names* (`libzenohc`, `libiceoryx2`) are part of the config schema
([sb-core/src/config.rs:109-111](crates/sb-core/src/config.rs#L109-L111)) and
must not be renamed per platform:

```yaml
# Host-specific tool paths. `sb doctor` reports which are missing.
protoc:      null      # protoc.exe
flatc:       null      # flatc.exe
libzenohc:   null      # → zenohc.dll          (NOT libzenohc.so)
libiceoryx2: null      # → iceoryx2_ffi_c.dll  (NOT libiceoryx2_ffi_c.so)
tmux:        null      # → psmux, installed as tmux.exe

messages_root: __SB_HOME__/messages
```

`sb doctor`'s `check_lib` calls `libloading::Library::new(path)`
([sb-doctor/src/lib.rs:122-137](crates/sb-doctor/src/lib.rs#L122-L137)), which
maps to `LoadLibraryW` on Windows — so the check works unchanged. Note that
`LoadLibraryW` also resolves the DLL's *own* imports; a `zenohc.dll` whose
dependencies are not on PATH or beside it fails the check with a message that
names `zenohc.dll` rather than the missing dependency. Worth a sentence in
`sbcli_doctor.md` when this lands.

---

## 9. Gaps in the current code

Found while tracing the build. Ordered by what blocks a Windows release.

### 9.1 The tree already builds and runs on Windows — verified

Run on this box (Windows 11, rustc 1.89.0, host `x86_64-pc-windows-msvc`):

- `cargo check --workspace` → **clean**, zero errors, zero warnings. That
  includes `iceoryx2-pal-posix` (the C POSIX shim, compiled through `cc`/MSVC),
  all 26 zenoh 1.9 crates, `iceoryx2 0.9`, and every `sb-*` crate.
- `cargo build --release --bin sb` → **links**, 2m16s cold, producing
  `target\release\sb.exe` (27.2 MB) + `sb.pdb` (9.8 MB).
- `sb.exe --version` → `sb 0.1.41`.
- `sb.exe doctor` → runs, correctly reports the four unset tool paths, and
  **auto-detects psmux**: `[OK] tmux  …\WinGet\Links\tmux.exe (tmux 3.3.4)`.
  The `which`-based resolution in `sb-launch` works exactly as its comment
  claims.

So **no source change is required to produce a working Windows binary.** Every
gap below is in the packaging tooling or the test harness, not in the crates.
That is the good news the rest of this section is measured against.

### 9.2 Version descriptors — **resolved**

There used to be a single `version.json` at the repo root carrying flat
`platform: "linux-x86_64"` / `package_stem` keys. It mixed two unrelated
concerns — product release state and Linux build facts — which is why a second
platform had nowhere to go.

It is now split per platform, next to that platform's tooling:

- [platforms/linux/version.json](platforms/linux/version.json)
- [platforms/windows/version.json](platforms/windows/version.json)

```json
{
  "product": "swarmbotix",
  "version": "0.1.41",
  "os": "windows",
  "binary": "sb.exe",
  "arch": "windows-x86_64",
  "triple": "x86_64-pc-windows-msvc",
  "installer": "install.ps1",
  "package_stem": "swarmbotix-0.1.41-windows-x86_64"
}
```

`os` / `binary` / `arch` / `triple` / `installer` are **platform facts** — they
change when a build target changes. `version` / `package_stem` are **release
state** — mirrors of `Cargo.toml`'s `[workspace.package] version`, which stays
canonical because it is the only site the binary reads (`clap` prints it for
`sb --version`; Rust cannot read JSON at build time).

That leaves the version mirrored in seven files, so the sync is owned by a
skill — **`/sb-release [patch|minor|major|X.Y.Z]`** — rather than by hand. Both
staging scripts additionally refuse to run when the descriptor and `Cargo.toml`
disagree, so drift fails the build instead of shipping a zip whose name
contradicts the binary inside it. See installguide_ubuntu.md §"Versioning" for the full
mirror table, and note the `documents/*.md` carve-out: `sb 0.1.21+`-style
strings are historical feature markers and must never be find-and-replaced.

### 9.3 `install.sh` cannot run on Windows — **resolved by rewrite**

Beyond needing bash, it depends on `readlink -f`, `mktemp -d`, `unzip`,
`install -m 0755`, `rsync`, `sed -i`, `hostname -s`, `/dev/tty`, and `~/.bashrc`.
That is not portable-with-effort, so
[install.ps1](platforms/windows/dist-tooling/install.ps1) is a rewrite against
the same behavior contract (§8.2) rather than a port.

`hostname -s` deserves a specific note: it *appears* portable because
`hostname.exe` exists on Windows, but it rejects `-s`, so a naive port would
silently take the fallback branch and seed the wrong `device:`. The rewrite uses
`$env:COMPUTERNAME`.

### 9.4 Test sandboxes did not isolate on Windows — **fixed**

`Sandbox::cmd()` isolates by setting `HOME` to a tempdir
([sb-cli/tests/common/mod.rs:52-61](crates/sb-cli/tests/common/mod.rs#L52-L61)),
and the L2/L3/L5 sandboxes do the same. On Windows that has no effect:
`dirs::home_dir()` routes to `dirs_sys::known_folder_profile()`, i.e.
`SHGetKnownFolderPath(FOLDERID_Profile)` — it never consults `HOME` or
`USERPROFILE`. (On Linux `dirs_sys::home_dir()` *does* read `$HOME` first, which
is why this works there.)

Consequence: every integration test that creates a workspace, runs `sb init`, or
compiles the vault would read and write the developer's **real**
`C:\Users\<you>\.swarmbotix`. Not merely a false pass — a destructive one.

**Fixed.** The config layer already had the mechanism —
`resolve_sb_home_dir` ([sb-config/src/lib.rs:53-60](crates/sb-config/src/lib.rs#L53-L60))
honors `sb_home_dir` ahead of `dirs::home_dir()` on every platform — it just
wasn't used consistently. Four changes:

- `Sandbox` now pins `sb_home_dir: <tempdir>/.swarmbotix` into the config it
  writes, and keeps `HOME` only as Unix belt-and-braces. Both it and
  `WsSandbox` strip any inherited `sb_home_dir` line first, since serde's
  derived `Deserialize` rejects duplicate fields.
- `resolve_message_targets` and `resolve_message_definitions` route through
  `resolve_sb_home_dir` instead of `dirs::home_dir()` directly.
- New `global_config_path(cfg)` = `<sb_home>/sb.config.yml`, used by
  `config show` / `open` / `set`. The global config file is by definition
  inside the sb home; resolving it independently was the inconsistency that
  made `_config_file` unpinnable.
- `L3Sandbox` seeds the committed `tests/fixtures/iox2_targets/` tree into the
  sandbox's `message_targets` and pins that path. The C++ iceoryx2 template
  reads `<message_targets>/iox2/std/ImageStamped/ImageStamped.h` to resolve the
  payload namespace; previously the test passed only because the Linux dev box
  happened to have compiled it.

### 9.5 `runscript.bash` stays bash — **runtime constraint, document it**

`sb init` emits a bash script with a `#!/usr/bin/env bash` shebang, `set -euo
pipefail`, `${BASH_SOURCE[0]}`, and a `printf %q` argv-quoting loop
([sb-workspace/src/runscript.rs:24-93](crates/sb-workspace/src/runscript.rs#L24-L93)).
`sb up` then hands tmux a POSIX command line — `cd '<dir>' && '<script>'`
([sb-launch/src/lib.rs:82-93](crates/sb-launch/src/lib.rs#L82-L93)) — and
`Script::filename()` hardcodes the `.bash` extension.

psmux dispatches pane commands through PowerShell, so that command line is not
interpretable there no matter which bash is installed. **`sb up` / `sb run` do
not work on Windows yet.** This is the one substantive gap left; it needs a
`runscript.ps1` variant, or panes routed through bash explicitly — a change to
a published contract (`runscript.bash` is documented in requirements.md and
sbcli_docker_runscript.md), so it is deliberately not bundled into this work.

Two things did land around it:

- L5 tests that expect a pane to *execute* now guard on
  `tmux_launch_supported()` rather than `tmux_available()`, and skip on Windows
  with that reason. Tests that only build a plan or check preconditions still
  run.
- Test helpers that invoke a generated `runscript.bash` directly go through
  `bash_script()`, which resolves a **non-WSL** bash and forward-slashes the
  path. Two traps here: a bare `bash` on Windows usually resolves to
  `C:\Windows\System32\bash.exe` (the WSL launcher), which cannot open
  `C:/Users/…` and reports a misleading "No such file or directory"; and a
  native `C:\Users\…` argument reaches bash with every backslash read as an
  escape, arriving mangled as `C:Usershylee…`.

Smaller follow-on: the stub's `flutter run -d linux` example line should become
`-d windows` under `OS="windows"`.

### 9.6 `$EDITOR` fell back to `vi` — **fixed**

`sb message edit` and `sb config open` defaulted to `vi` when `$EDITOR` is
unset. No bare Windows box has `vi`. Both call sites now go through
`default_editor()`, which is `notepad` under `#[cfg(windows)]` and `vi`
elsewhere.

### 9.8 Generated source carried backslash paths — **fixed**

`sb-codegen` bakes absolute paths to the iox2 payload files into the generated
pub/sub source. Built with `PathBuf::join`, those came out backslashed on
Windows, and the consequences are not cosmetic:

- **Rust** — `include!("C:\…\Foo.rs")` does not compile; `\U`, `\s`, `\t` are
  read as string escapes (invalid, or silently wrong).
- **C++** — a `\` in an `#include "…"` header-name is undefined per
  `[lex.header]`; MSVC tolerates it, other toolchains need not.
- **Python** — survives only because the emitted literal happens to be a raw
  string.

All four path strings now go through a `slash_path` helper. Windows accepts
`/` in every one of these, so normalizing is both the portable and the simpler
choice — and it keeps the codegen goldens byte-identical across platforms,
which is how the bug surfaced (a golden diff with identical byte counts).

### 9.9 `sb doctor`'s vault check was behind its spec — **fixed**

[documents/sbcli_doctor.md](documents/sbcli_doctor.md) §4.6 specifies check #6
as `std vault`: walk `<message_definitions>/std/` and confirm every file the
forge-bundled `STD_BUNDLE` ships is present, failing with a count and a hint at
`sb message list`. The implementation only tested `vault_root.is_dir()` and
labelled the line `messages`, so two L1 tests written against the doc had been
failing on every platform.

`sb-vault` now exposes `std_bundle_filenames()` / `missing_std_files()` — read
from the embedded bundle, so the check cannot drift from what
`ensure_installed` writes — and `sb-doctor` implements the documented behavior
against them.

### 9.10 iceoryx2 runtime on Windows — unverified

The crate compiles, but nothing here exercises shared-memory transport on
Windows. iceoryx2 supplies its own PAL rather than needing `/dev/shm`, so it is
expected to work; "expected" is not "tested". The L4/L5 iceoryx2 integration
tests are already `#[ignore]`d on shmem-restricted CI — run them explicitly with
`cargo test -- --ignored` on a Windows box before claiming iox2 support in
release notes. Zenoh is pure Rust over TCP/UDP and carries no such risk.

---

## 10. Open decisions

1. **Ship the DLLs in the payload?** Linux does not bundle `libzenohc.so` —
   users install it and point `sb.config.yml` at it. Windows has no apt/brew to
   defer to, so the `lib/` slot reserved in §6 exists for `zenohc.dll` +
   `iceoryx2_ffi_c.dll`. Bundling adds ~10 MB and a licensing/versioning
   obligation; not bundling makes first-run `sb doctor` fail for everyone.
   **Recommendation: do not bundle for the first Windows cut**, and revisit once
   there is a real user on it — but decide before publishing, because adding a
   `lib/` dir later is a payload-layout change the installer has to learn.
2. **Formalize the Linux build as `build.sh`?** Recommended (§7) — a copy-paste
   block in markdown cannot be kept honest against `.env`.
3. **Code-signing.** Unsigned `sb.exe` triggers SmartScreen on download. Deferred
   until distribution moves off direct hand-off, but it is the one item with a
   procurement lead time, so it should be *tracked* now.
4. **Checksums on both platforms at once** (§6) rather than letting the two
   slots diverge.

---

## 11. Smoke test

Windows analogue of installguide_ubuntu.md §"Smoke testing before release" — a sandbox
`SB_HOME` so a test install never touches the real one. All three paths the
Linux smoke test covers have an equivalent:

```powershell
$Dist = "platforms\windows\dist\0.1.41"

$Sandbox = Join-Path $env:TEMP "sb-smoke-$(Get-Random)"
New-Item -ItemType Directory -Force $Sandbox | Out-Null
$env:SB_HOME = "$Sandbox\.swarmbotix"

# 1. Auto-accept path
& "$Dist\install.ps1" -Yes

# Verify
& "$env:SB_HOME\bin\sb.exe" --version              # → sb 0.1.41
Get-ChildItem "$env:SB_HOME\messages"
& "$env:SB_HOME\bin\sb.exe" doctor                 # lists what's missing

# 2. Idempotent re-run — PATH must not gain a duplicate entry
& "$Dist\install.ps1" -Yes
(([Environment]::GetEnvironmentVariable('Path','User') -split ';') |
    Where-Object { $_ -like '*\.swarmbotix\bin' }).Count    # → 1

# 3. Decline path — answer 'n'; PATH left untouched
& "$Dist\install.ps1"

Remove-Item $Sandbox -Recurse -Force
Remove-Item Env:\SB_HOME
```

Note that a custom `SB_HOME` makes `install.ps1` skip PATH wiring entirely (same
rule as `install.sh`), so check 2 must run against the default prefix on a
throwaway box or VM — the sandbox verifies extraction and layout, not PATH.

---

## 12. Order of work

- [x] `platforms/{windows,linux}/{dist,dist-tooling}/` created; the existing
      `dist-tooling/` moved into the linux slot; both installguides repointed.
      *(§6)*
- [x] `/.env` gitignored; [.env.example](.env.example) committed. *(§2)*
- [x] Root `version.json` split into `platforms/{linux,windows}/version.json`;
      both staging scripts gained a drift check; `/sb-release` owns the bump.
      *(§9.2)*
- [x] `platforms/windows/dist-tooling/sb.config.win.yml.template`. *(§8.3)*
- [x] `build.ps1` — verified by producing `platforms/windows/dist/0.1.29/`
      (9.5 MB zip + .sha256 + install.ps1). *(§8.1)*
- [x] `install.ps1` — verified against a sandbox `SB_HOME`: 138 vault files
      copied, config seeded from `$env:COMPUTERNAME`, `sb.exe --version` and
      `sb.exe doctor` run from the install. *(§8.2, §11)*
- [x] Test sandboxes → `sb_home_dir` instead of `HOME`. `cargo test
      --workspace` on Windows: **281 passed, 0 failed**. *(§9.4)*
- [x] `$EDITOR` → `notepad` under `#[cfg(windows)]`. *(§9.6)*
- [x] Codegen emits forward-slashed paths. *(§9.8)*
- [x] `sb doctor` implements the documented `std vault` check. *(§9.9)*

Remaining:

- [ ] **1.** Port the launch path — `sb up` / `sb run` panes are a POSIX command
      line running a bash script, which psmux cannot dispatch. The L5
      pane-executing tests skip on Windows via `tmux_launch_supported()`.
      *(§9.5)*
- [ ] **2.** Run the iceoryx2 integration tests with `--ignored` on Windows
      before claiming iox2 support. *(§9.10)*
- [ ] **3.** Decide the `lib/` DLL-bundling question before publishing. *(§10)*

The release path is done — a Windows bundle can be cut and installed today.
Item 1 is what stands between that and `sb up` working on Windows.
