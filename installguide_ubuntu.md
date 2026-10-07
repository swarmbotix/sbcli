# swarmbotix install guide — Ubuntu / Linux

Two audiences in one document:

- **[Part 1 — End users](#part-1--end-users)**: how to install `sb` on your machine.
- **[Part 2 — Maintainers](#part-2--maintainers)**: how to cut a release from this repo.

This document covers the **Linux** build. For the Windows build and packaging
pattern — the `.env` switch, `target/` and `dist/` layouts, `build.ps1` /
`install.ps1` specs, and the platform gaps found in the current code — see
[installguide_windows.md](installguide_windows.md).

---

## Part 1 — End users

### What you get

`sb` is the swarmbotix CLI. A release ships as **one zip per architecture**,
plus a checksum beside it, in a per-version, per-platform directory:

```
platforms/linux/dist/0.2.1/
  swarmbotix-0.2.1-linux-x86_64.zip           ← the payload, installer included
  swarmbotix-0.2.1-linux-x86_64.zip.sha256    ← checksum (sha256sum -c)
  swarmbotix-0.2.1-linux-aarch64.zip
  swarmbotix-0.2.1-linux-aarch64.zip.sha256
```

**The installer is inside the zip**, not beside it. One download is the whole
product: unzip it and run the `install.sh` that comes out.

Each platform owns a slot under [platforms/](platforms/) — `platforms/linux/`
and `platforms/windows/` exist today, `platforms/mac/` gets added when that
build target lands. See [platforms/README.md](platforms/README.md) for the full
layout. The Rust workspace itself is **not** split per platform: one
`Cargo.toml`, one `crates/` tree, one `target/`.

The payload bundles:

- The `sb` binary (Linux x86_64)
- The canonical message-definition vault in `.proto` form: two styles, `ros2/`
  (`std/*` plus 10 ROS2 packages — `geometry_msgs`, `sensor_msgs`, etc.; 138
  `.proto`) and `swarmbotix/` (14 `.proto`)
- Reference documentation
- A default `sb.config.yml` template
- A `VERSION` stamp
- `install.sh` and `uninstall.sh`

`install.sh` is version-agnostic: it copies its own siblings into
`~/.swarmbotix/`. There is nothing for it to download or extract.

### Prerequisites on the target machine

`sb` ships for **x86_64 and aarch64** (Jetson, Raspberry Pi 64-bit, ARM servers),
linked against **glibc 2.17**, so it loads on anything from CentOS 7 / Ubuntu
14.04 forward — Ubuntu 20.04 and 22.04, Debian 11, and JetPack 5 and 6 included.
There is no distro floor to check before installing. Pick the zip matching
`uname -m` (`x86_64` → `linux-x86_64`, `aarch64` → `linux-aarch64`); if you
take the wrong one, `install.sh` says so and stops rather than installing a
binary this machine cannot execute.

> Older releases were linked against whatever glibc the build machine ran, and
> refuse to start on anything older than it with
> `` `GLIBC_2.xx' not found (required by sb)`` before `main` is reached. If you
> see that, the fix is to upgrade — nothing on the target machine can help.

The binary itself is self-contained, but some commands (`sb doctor`,
`sb message compile`, `sb up`) shell out to external tools. Install whichever
of these you actually need — `sb doctor` will tell you what's missing:

| Tool | Needed for | Min version |
|---|---|---|
| `protoc` | `.proto` codegen | libprotoc 3.21+ |
| `flatc` | Flatbuffers codegen | 24.x+ |
| `libzenohc.so` | cross-device pub/sub | matches `zenoh 1.9` |
| `libiceoryx2_ffi_c.so` | same-device pub/sub | matches `iceoryx2 0.9` |
| `tmux` | launching modules via `sb up` | 3.2+ |

### One-line install

Download the zip for your machine from `platforms/linux/dist/0.2.1/`
(`uname -m` tells you which; the `.sha256` too if you want to verify), then:

```bash
sha256sum -c swarmbotix-0.2.1-linux-x86_64.zip.sha256   # optional
unzip swarmbotix-0.2.1-linux-x86_64.zip
./swarmbotix-0.2.1-linux-x86_64/install.sh
```

Unzip anywhere except inside `~/.swarmbotix` itself — the installer refuses
that case, because source and destination would be the same tree.

The installer will:

1. Check that it really is sitting in an unpacked package, and that the
   package's architecture matches this machine
2. Create `~/.swarmbotix/{bin,messages,documents}/`
3. Copy `sb` → `~/.swarmbotix/bin/sb`
4. Copy message `.proto` files → `~/.swarmbotix/messages/ros2/message_definitions/` — **existing files are never overwritten** (your edits are safe across upgrades)
5. Replace `~/.swarmbotix/documents/` (reference docs ship verbatim)
6. Stamp `~/.swarmbotix/VERSION`
7. Seed `~/.swarmbotix/sb.config.yml` from the template **only if no config exists yet**. `device:` is filled in from `hostname -s`; vault paths are pinned to your `$SB_HOME`.
8. Place `~/.swarmbotix/uninstall.sh` — the same file that is in the zip, copied
   rather than generated, so the two cannot drift
9. Prompt:
   > `Add ~/.swarmbotix/bin to your PATH in ~/.bashrc? [Y/n]`
   - **Y** (default): appends `export PATH="$HOME/.swarmbotix/bin:$PATH"  # added by swarmbotix installer` to `~/.bashrc`
   - **n**: prints the exact line so you can add it to whichever shell rc file you prefer

Verify after install:

```bash
source ~/.bashrc      # or open a new terminal
sb --version          # → sb 0.2.1
sb doctor             # lists what's missing on your system
```

### If you skipped the bashrc edit

Add this to your shell config (`~/.bashrc`, `~/.zshrc`, or `~/.profile`):

```bash
export PATH="$HOME/.swarmbotix/bin:$PATH"
```

Then reload: `source ~/.bashrc`

### Layout after install

```
~/.swarmbotix/
  bin/sb                       ← the binary
  sb.config.yml                ← seeded from template (only if absent)
  VERSION                      ← build stamp for diagnostics
  uninstall.sh                 ← copied out of the package by install.sh
  messages/                    ← message styles; one subdir per style
    ros2/                      ←   std/* + the ROS2 mirror (138 .proto)
      message_definitions/
        std/  std_msgs/  geometry_msgs/  sensor_msgs/  ...
      message_targets/         ←   generated iox2/ proto/ fb/ bindings
    swarmbotix/                ←   main style; swarmbotix's own defs
      message_definitions/
        header/  primitives/  images/  sensors/
  documents/                   ← reference docs (sbcli.md, sbcli_doctor.md, ...)
  workspaces/                  ← created by `sb ws create`
  active                       ← created by `sb ws set`
```

On Windows the same tree lands under `%USERPROFILE%\.swarmbotix`
(e.g. `C:\Users\you\.swarmbotix`) — a dotted directory in the home dir, not
`AppData`. See [installguide_windows.md](installguide_windows.md) §6 "Install
layout on the target machine".

### Non-interactive install

For CI, scripts, or anyone tired of being prompted:

```bash
./swarmbotix-0.2.1-linux-x86_64/install.sh --yes
```

Auto-accepts the bashrc edit. Everything else is unchanged.

### Custom install prefix

```bash
SB_HOME=/opt/swarmbotix ./swarmbotix-0.2.1-linux-x86_64/install.sh
```

Installs under `$SB_HOME` instead of `~/.swarmbotix`. You're responsible for
adding `$SB_HOME/bin` to `$PATH` yourself in this mode (the installer skips
the bashrc prompt for non-default prefixes).

### Upgrading

From `sb 0.2.1` on, `sb` upgrades itself:

```bash
sb update --check      # installed vs latest release; exit 10 when newer exists
sb update              # download, verify sha256, run the package's install.sh
```

`sb --version` also prints `update available: X.Y.Z` when the release page has
something newer (checked at most once a day; `SB_NO_UPDATE_CHECK=1` disables it).

By hand, same result: unzip the newer package and run its `install.sh`. The
binary is overwritten; your config, workspaces, installed apps and edited
message files are preserved. Nothing needs uninstalling first.

### Uninstalling

`uninstall.sh` removes the PATH line from `~/.bashrc`, then deletes `$SB_HOME`
— binary, config, message vault and docs. It confirms before the delete unless
you pass `--yes`.

```bash
~/.swarmbotix/uninstall.sh              # PATH line, then delete ~/.swarmbotix (asks first)
~/.swarmbotix/uninstall.sh --yes        # same, no prompt
~/.swarmbotix/uninstall.sh --keep-home  # PATH line + binary only; config & messages stay
```

The copy in the zip is the same file, so `./swarmbotix-<version>-<arch>/uninstall.sh`
works too if you still have the package. It reads `$SB_HOME` the same way the
installer does, and refuses to run if that resolves to `/` or your home
directory.

---

## Part 2 — Maintainers

This is the workflow for cutting a new release from this repo.

### Prerequisites on the build machine

- Rust toolchain (`cargo`) — the workspace pins via `rust-toolchain.toml`
- `zip`, `rsync`, `jq`, `sha256sum`, `readelf` (all standard on modern Linux;
  `sha256sum` is coreutils, `readelf` is binutils)
- `cargo-zigbuild` and `zig`, which pin the glibc floor of the shipped binary

`build.sh` checks for all of them before it starts staging.

The two zig pieces both come from PyPI, so there is no apt package and no
container to maintain:

```bash
python3 -m venv ~/.zigbuild
~/.zigbuild/bin/pip install cargo-zigbuild ziglang
# The ziglang wheel ships no `zig` entry point, and cargo-zigbuild wants one.
printf '#!/bin/sh\nexec "%s/bin/python" -m ziglang "$@"\n' "$HOME/.zigbuild" \
  > ~/.zigbuild/bin/zig
chmod +x ~/.zigbuild/bin/zig
export PATH="$HOME/.zigbuild/bin:$PATH"
```

**Why:** linking against the build machine's own libc bakes that machine's
symbol versions into the binary, so a release cut on 24.04 will not start on
22.04 — it dies in the loader with `` `GLIBC_2.39' not found `` before any of
our code runs. zig carries the glibc stubs for every version, so the floor
becomes a build input (`glibc_floor` in `platforms/linux/version.json`, `2.17`)
instead of an accident of the build host. That is also what keeps a
maintainer's zip and a CI zip the same zip regardless of the distro either
one runs.

### Source layout

```
Cargo.toml  crates/  target/       ← ONE workspace, not split per platform

platforms/
  linux/
    version.json                   ← platform descriptor (triple, binary, arch)
    dist-tooling/                  ← long-lived build + installer source (in repo)
      build.sh                     ← staging script; the whole release procedure
      install.sh                   ← version-agnostic entry-point, staged INTO the zip
      uninstall.sh                 ← ditto; also placed in $SB_HOME at install time
      sb.config.yml.template       ← seeded into new installs
    dist/                          ← release output (one dir per version)
      0.2.1/
        swarmbotix-0.2.1-linux-x86_64.zip
        swarmbotix-0.2.1-linux-x86_64.zip.sha256
        swarmbotix-0.2.1-linux-aarch64.zip
        swarmbotix-0.2.1-linux-aarch64.zip.sha256
  windows/                         ← see installguide_windows.md
  mac/                             ← future: when a macOS build exists
```

Note there is **no platform segment inside `dist/`** — the platform is already
in the path. Old release dirs are kept side by side; the staging script only
touches the `<version>/` slot being built, so cutting Windows never disturbs
`platforms/linux/`.

### Build steps

Bump first with `/sb-release` (see §Versioning), then run the staging script:

```bash
./platforms/linux/dist-tooling/build.sh
```

That is the whole procedure, and it cuts **every** architecture in the
descriptor: `x86_64` plus everything in `extra_arches`, currently `aarch64`. It
reads the platform from `.env` and every build fact from
`platforms/linux/version.json`, and refuses to run if the descriptor disagrees
with `Cargo.toml`. It is the Linux counterpart of
[build.ps1](platforms/windows/dist-tooling/build.ps1) — same inputs, same output
shape, so the two platforms of one release are cut the same way.

aarch64 is **cross-compiled from x86_64**; no ARM machine is involved. zig
supplies both the target's glibc stubs and a cross C compiler for the one
dependency that needs one (`ring`), so the same `build.sh` on the same laptop
produces both zips.

Prerequisites: `cargo`, `jq`, `zip`, `rsync`, `sha256sum`, `readelf`, plus
`cargo-zigbuild` and `zig` (see §Prerequisites above). The script checks for
them up front rather than failing halfway through staging.

| Flag | Effect |
|---|---|
| `--arch <arch>` | Cut just one, e.g. `linux-aarch64`. Leaves the other architectures' zips in the slot untouched, which is how CI runs a job per target |
| `--skip-build` | Reuse `target/<triple>/<profile>/`; do not run cargo |
| `--no-zip` | Stop after staging the payload dir, for inspecting the tree |

`--skip-build` skips the compile, **not** the glibc floor check. A plain
`cargo build` produces a natively-linked binary, and that is exactly the thing
the check exists to keep out of a zip.

Optional `.env` overrides, same names `build.ps1` honours: `DIST_ROOT`
(default `platforms`), `TARGET_TRIPLE` (default the descriptor's `triple`),
`PROFILE` (default `release`). `TARGET_TRIPLE` retargets the **default** arch
only; it is an escape hatch for one build, not a way to redefine the arch list.

What it does, in order:

1. **Read `.env`** for the platform. Never `uname` / `$OSTYPE` — see
   installguide_windows.md §2 for the contract. Refuses to run under
   `OS="windows"` and points at `build.ps1` instead.
2. **Drift check.** Compares the descriptor's `version` and `package_stem`
   against `cargo metadata`. Fatal on mismatch: a bundle whose zip name
   disagrees with the binary inside it is worse than no bundle.
Steps 3 to 8 run once **per architecture**; the rest happen once.

3. **Build** with `cargo zigbuild` and an explicit
   `--target <triple>.<glibc_floor>`, so the artifact lands in the per-triple
   dir and a bare `cargo build --release` can never be mistaken for a staged
   one. See installguide_windows.md §5.
4. **Verify the binary agrees** — runs `sb --version` on what it is about to
   stage and refuses to wrap a version-named zip around a stale `target/`.
   This is the check that catches `--skip-build` used after a bump. On a
   cross-built arch the host cannot run the binary, so this is skipped with a
   printed note rather than quietly passing.
5. **Verify the glibc floor** by reading `.gnu.version_r` off the artifact: the
   highest `GLIBC_*` version it demands must not exceed `glibc_floor`. Read
   from the binary rather than trusted from the build flag, because a too-high
   floor is invisible on the build machine and fatal on the target. Unlike
   step 4 this works on any architecture, since it inspects rather than
   executes.
6. **Stage** into `dist/<version>/<package_stem>/`, clearing only *this arch's*
   files in that version slot — never the sibling platform, never the sibling
   architecture: binary → `bin/`, the whole `messages/` tree (excluding the
   generated `.cache/`, `targets/`, `message_targets/`), `documents/`, the
   config template, and a generated `VERSION` stamp. Every staged style must
   carry a `message_definitions/` or staging fails.
7. **Copy `install.sh` and `uninstall.sh`** verbatim from `dist-tooling/` into
   the staged payload, at its top level. They ride inside the zip, so a release
   download is one self-contained file and there is no loose installer that can
   drift from the payload it installs.
8. **Zip** the payload as a single top-level entry (`install.sh` asserts on that
   layout) and write its `.sha256`.

> **On a checkout that cannot represent unix modes** (an NTFS/exFAT mount),
> `install -m 0755` fails outright, so the script falls back to `cp` plus a
> best-effort `chmod`. `install.sh` re-applies `0755` on the target machine, so
> the shipped binary is executable either way.

### What goes in `platforms/linux/dist/<version>/`

One zip and one checksum **per architecture**, and nothing else:

| Path | Source | Notes |
|---|---|---|
| `swarmbotix-<version>-linux-x86_64.zip` | staged payload | Binary + messages + docs + VERSION + template + the two scripts |
| `swarmbotix-<version>-linux-aarch64.zip` | staged payload | Same payload, aarch64 binary |
| `<pkg>.zip.sha256` | generated | One per zip. `sha256sum` format, so the recipient can `sha256sum -c` it |

Slots up to `0.1.40` were cut before the installer moved inside the package and
still carry a loose `install.sh`. Nothing reads it any more.

### What goes in the zip

| Path | Source | Notes |
|---|---|---|
| `bin/sb` | `target/<triple>/release/sb` | Release-built binary (~24 MB x86_64, ~21 MB aarch64) |
| `messages/` | `messages/` | One subdir per style, each with `message_definitions/`. Excludes `.cache/`, `targets/`, `message_targets/` |
| `documents/` | `documents/` | Reference docs verbatim |
| `sb.config.yml.template` | `platforms/linux/dist-tooling/` | Has `__HOSTNAME__` and `__SB_HOME__` placeholders |
| `install.sh` | `platforms/linux/dist-tooling/` | Version-agnostic; installs its own siblings. Mode `0755`, which `zip` records and `unzip` restores |
| `uninstall.sh` | `platforms/linux/dist-tooling/` | Copied to `$SB_HOME/uninstall.sh` at install time |
| `VERSION` | generated | `version: / built: / platform: / binary:` lines. Its `platform:` line is what `install.sh` checks `uname -m` against |

End size for v0.1.x: **~9 MB zipped per architecture** — the ~24 MB release binary compresses hard; `messages/` is ~360 KB of `.proto` and `documents/` ~290 KB of Markdown, so neither moves the number.

### Smoke testing before release

Always test in a sandbox `$HOME` so you don't pollute your real `~/.swarmbotix/`:

```bash
VERSION=$(jq -r .version platforms/linux/version.json)
DIST=$PWD/platforms/linux/dist/${VERSION}
PKG=swarmbotix-${VERSION}-linux-x86_64

SANDBOX=$(mktemp -d)
mkdir -p "${SANDBOX}/home"
echo "# pre-existing" > "${SANDBOX}/home/.bashrc"
unzip -q "${DIST}/${PKG}.zip" -d "${SANDBOX}"      # the installer is in here

# 1. Auto-accept path
HOME="${SANDBOX}/home" SB_HOME="${SANDBOX}/home/.swarmbotix" \
  "${SANDBOX}/${PKG}/install.sh" --yes

# Verify
"${SANDBOX}/home/.swarmbotix/bin/sb" --version
ls "${SANDBOX}/home/.swarmbotix/messages/ros2/message_definitions/"
tail -3 "${SANDBOX}/home/.bashrc"

# 2. Idempotent re-run (should detect marker, not double-add)
HOME="${SANDBOX}/home" SB_HOME="${SANDBOX}/home/.swarmbotix" \
  "${SANDBOX}/${PKG}/install.sh" --yes
grep -c 'swarmbotix installer' "${SANDBOX}/home/.bashrc"   # → 1

# 3. Decline path — no tty and stdin closed, so the prompt cannot be answered.
#    `setsid` detaches from the controlling terminal; without it the installer
#    falls back to /dev/tty and this tests nothing.
SANDBOX3=$(mktemp -d); mkdir -p "${SANDBOX3}/home"
echo "# pre-existing" > "${SANDBOX3}/home/.bashrc"
unzip -q "${DIST}/${PKG}.zip" -d "${SANDBOX3}"
HOME="${SANDBOX3}/home" SB_HOME="${SANDBOX3}/home/.swarmbotix" \
  setsid "${SANDBOX3}/${PKG}/install.sh" < /dev/null
grep -c 'swarmbotix installer' "${SANDBOX3}/home/.bashrc"  # → 0, bashrc untouched

# 4. Uninstall, from the copy install.sh placed in $SB_HOME — that copy is
#    inside the tree it deletes, so this also exercises the re-exec.
HOME="${SANDBOX}/home" SB_HOME="${SANDBOX}/home/.swarmbotix" \
  "${SANDBOX}/home/.swarmbotix/uninstall.sh" --yes
ls -d "${SANDBOX}/home/.swarmbotix"                        # → No such file or directory
grep -c 'swarmbotix installer' "${SANDBOX}/home/.bashrc"   # → 0

rm -rf "${SANDBOX}" "${SANDBOX3}"
```

Run it against a freshly staged slot, not a released one: releases up to
`0.1.40` were cut before the installer moved inside the package, so their zips
have no `install.sh` to run.

A fresh sandbox has no `protoc` path in its seeded config, so the vault compile
at the end of the install reports `did not fully compile`. That is expected
there and is not a release blocker — it is the same message a first-run user
sees before `sb doctor`.

Four paths the smoke test should cover before any release:

1. **`--yes` auto-accept** — the happy path; bashrc gets the export line.
2. **Idempotent re-run** — second `--yes` invocation must detect the marker comment and skip the bashrc edit. Line count should stay at 1.
3. **Decline path** — when the user answers `n` (or stdin is closed and `/dev/tty` cannot be opened), the installer falls through to printing manual instructions and leaves `~/.bashrc` untouched. Note it tests a *successful exit*, not just an untouched bashrc: `/dev/tty` passes an `-r` test even with no controlling terminal, and a redirect that then fails with `ENXIO` would end the script under `set -e` after everything is already copied. The installer opens the tty instead of testing it, precisely so this path exits 0.
4. **Uninstall** — `$SB_HOME` gone and the bashrc line with it. The uninstaller it runs lives inside the directory it deletes, so this is also the test that its re-exec-from-a-temp-copy works.

### Versioning

**Do not bump by hand — run `/sb-release [patch|minor|major|X.Y.Z]`.** The
version is mirrored across seven files; the skill writes all of them and then
verifies they agree. What follows is what it does, for when you need to audit it.

**The canonical version is `Cargo.toml`'s `[workspace.package] version`.** It is
canonical because it is the only site the *binary* reads — `clap` picks up
`version.workspace = true`, which is what `sb --version` and the `sb` line in
`sb doctor` print. Rust does not read JSON at build time, so no JSON file can
ever influence it. Everything else mirrors it.

There is no root `version.json`. Each platform carries its own descriptor next
to its tooling — [platforms/linux/version.json](platforms/linux/version.json)
and [platforms/windows/version.json](platforms/windows/version.json):

```json
{
  "product": "swarmbotix",
  "version": "0.2.1",
  "os": "linux",
  "binary": "sb",
  "arch": "linux-x86_64",
  "triple": "x86_64-unknown-linux-gnu",
  "installer": "install.sh",
  "package_stem": "swarmbotix-0.2.1-linux-x86_64"
}
```

`os` / `binary` / `arch` / `triple` / `installer` are **platform facts** — they
change when a build target changes, not when a release is cut. `version` and
`package_stem` are **release state** — mirrors, rewritten on every bump.

The seven mirror sites:

| # | Site | Field |
|---|---|---|
| 1 | `Cargo.toml` | `[workspace.package] version` — **canonical** |
| 2 | `Cargo.lock` | 15 `sb-*` entries — refreshed by `cargo update --workspace` |
| 3 | `platforms/linux/version.json` | `version`, `package_stem` |
| 4 | `platforms/windows/version.json` | `version`, `package_stem` |
| 5 | this file | the Part 1 example paths + expected `sb --version` output |
| 6 | [installguide_windows.md](installguide_windows.md) | example paths, smoke-test `$Dist` |
| 7 | [platforms/README.md](platforms/README.md) | the `dist/<version>/` tree diagram |

**Never** rewrite the `sb 0.1.21+` / `sb 0.1.23+` strings in `documents/*.md` —
those are feature-introduction markers recording which release changed an API,
not mirrors of the current version. A blind find-and-replace across the repo is
always wrong.

The staging script above refuses to run when the descriptor and `Cargo.toml`
disagree, so drift fails the build rather than shipping a zip whose name
contradicts the binary inside it. Everything else (the in-zip `VERSION` stamp)
is generated from the descriptor.

### Distribution

**Tagging is what publishes.** [build.yml](.github/workflows/build.yml) starts
on a `v*` tag and nothing else — not on pushes. It checks, builds all three
targets, and attaches every zip and checksum to a GitHub Release — and nothing
besides, since the installer travels inside each zip:

```bash
/sb-release                     # bumps all seven mirror sites
git commit -am "release 0.2.1"
git tag v0.2.1                 # must match Cargo.toml; CI checks it first
git push --follow-tags
```

A tag that disagrees with `Cargo.toml` fails in the first seconds of the run,
before anything compiles, and produces no Release.

To hand off files directly instead (Dropbox, static host), the end-user
instructions are unchanged regardless of where they came from — pick the zip
for the target machine's `uname -m`:

```bash
ARCH=linux-x86_64   # or linux-aarch64
PKG=swarmbotix-${VERSION}-${ARCH}
curl -L <stable URL>/${PKG}.zip         -o ${PKG}.zip
curl -L <stable URL>/${PKG}.zip.sha256  -o ${PKG}.zip.sha256
sha256sum -c ${PKG}.zip.sha256
unzip ${PKG}.zip && ./${PKG}/install.sh
```

Two files to hand over, not three, and the second is only there to verify the
first. Whoever receives them cannot end up running an installer from one
release against a payload from another.

### Known limitations

- **glibc compatibility**: the binary is dynamically linked against the build
  machine's glibc. It will run on the same-or-newer distro family but may
  fail with a glibc version error on older targets. If you need to ship to
  arbitrarily old boxes, rebuild with `--target x86_64-unknown-linux-musl`
  for a fully static binary.
- **No signatures**: `build.sh` ships a `.sha256` beside the zip, which detects
  a corrupted download but not a substituted one — an attacker who can replace
  the zip can replace the checksum next to it. Sign the release (minisign, GPG,
  or a signed manifest on a host the attacker does not control) before handing
  it out beyond direct collaborators.
- **x86_64 only**: Linux (`x86_64-unknown-linux-gnu`) and Windows
  (`x86_64-pc-windows-msvc`) both build, package, and ship — `build.sh` and
  [build.ps1](platforms/windows/dist-tooling/build.ps1) are counterparts. No
  macOS and no aarch64 on either platform. Add cross-build targets to the
  workflow when you need them; `platforms/mac/` is the slot for the first.
