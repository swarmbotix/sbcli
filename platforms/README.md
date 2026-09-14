# `platforms/` — per-platform packaging

Everything here is about **shipping** `sb`, never about building it.

The Rust source is one workspace at the repo root: one [Cargo.toml](../Cargo.toml),
one [crates/](../crates/) tree, one `target/`. That is deliberate — the crates
compile identically on Linux and Windows (verified: `cargo check --workspace` is
clean on `x86_64-pc-windows-msvc`), so there is nothing to fork. Cargo already
separates the build outputs by target triple:

```
target/x86_64-pc-windows-msvc/release/sb.exe
target/x86_64-unknown-linux-gnu/release/sb
```

What genuinely differs per platform is the packaging: which shell the installer
is written in, how PATH gets wired, what the native libraries are called. That
is what lives here.

```
platforms/
├── linux/
│   ├── version.json                ← platform descriptor (see below)
│   ├── dist-tooling/               ← long-lived build + installer source (in repo)
│   │   ├── build.sh                   staging script
│   │   ├── install.sh                 version-agnostic entry-point
│   │   ├── uninstall.sh               PATH line + $SB_HOME removal
│   │   └── sb.config.yml.template     seeded into new installs (.so paths)
│   └── dist/                       ← release output, one dir per version
│       └── 0.1.41/
│           ├── swarmbotix-0.1.41-linux-x86_64.zip
│           ├── swarmbotix-0.1.41-linux-x86_64.zip.sha256
│           ├── swarmbotix-0.1.41-linux-aarch64.zip
│           └── swarmbotix-0.1.41-linux-aarch64.zip.sha256
│
└── windows/
    ├── version.json                ← platform descriptor
    ├── dist-tooling/
    │   ├── build.ps1                  staging script (counterpart of build.sh)
    │   ├── install.ps1
    │   ├── uninstall.ps1
    │   └── sb.config.win.yml.template (.dll paths)
    └── dist/
        └── 0.1.41/
            ├── swarmbotix-0.1.41-windows-x86_64.zip
            └── swarmbotix-0.1.41-windows-x86_64.zip.sha256
```

Both platforms ship for `0.1.41`. The two staging scripts take the same inputs
(`.env` + the platform descriptor), enforce the same drift check, and emit the
same two files per architecture, so a release cut on one box matches one cut on
the other.

**The installer is not a loose file.** `install.sh` / `install.ps1` and their
uninstall counterparts are staged *inside* the package, at the top of the tree
next to `bin/`, so a release download is one self-contained zip:

```
swarmbotix-<version>-<arch>/
├── bin/
├── documents/
├── messages/
├── sb.config.yml.template
├── install.sh          ← copied verbatim from dist-tooling/
├── uninstall.sh        ← ditto; install.sh also places it in $SB_HOME
└── VERSION
```

The installers used to sit beside the zips and pick one to extract, which meant
a user needed two downloads that had to match, and the release page carried a
script that could drift from the payload it installed. Now the package is the
unit: unzip it, run the `install.sh` that comes out of it, and it installs its
own siblings. Slots up to `0.1.40` were cut the old way and still hold a loose
installer; from the next cut they do not.

| Platform | Cut with |
|---|---|
| Linux | `./platforms/linux/dist-tooling/build.sh` |
| Windows | `pwsh -File platforms\windows\dist-tooling\build.ps1` |

The one deliberate difference is the checksum format: `build.sh` writes
`sha256sum` format (`<hash>  <name>`) so the recipient can run
`sha256sum -c`, while `build.ps1` writes a bare hash because `Get-FileHash`
has no `-c` counterpart to satisfy.

## `version.json` — the platform descriptor

There is deliberately **no root `version.json`**. The old one mixed product
release state with Linux build facts, which is why a second platform had nowhere
to go. Each platform now carries its own, next to its tooling:

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

| Field | Kind | Changes when |
|---|---|---|
| `os`, `binary`, `arch`, `triple`, `installer` | platform fact | a build target changes |
| `glibc_floor` (Linux only) | platform fact | the oldest supported distro moves |
| `extra_arches` (Linux only) | platform fact | a machine type is added or dropped |
| `version`, `package_stem` | release state | every release cut |

`glibc_floor` is the oldest glibc the shipped binary must load against — `2.17`,
the manylinux2014 baseline, which covers Ubuntu 20.04, Debian 11 and JetPack 5.
`build.sh` passes it to `cargo zigbuild` and then re-reads it off the artifact,
so it is enforced rather than intended. Windows has no counterpart: the MSVC
runtime is not versioned this way.

`extra_arches` is every machine type built **besides** the descriptor's own
`arch`/`triple`, which stays the default and is the only one `package_stem`
mirrors:

```json
"extra_arches": [
  { "arch": "linux-aarch64", "triple": "aarch64-unknown-linux-gnu" }
]
```

Their package stems are **derived** at build time as `<product>-<version>-<arch>`
and deliberately not stored. That is what keeps adding an architecture from
adding an eighth mirror site for `/sb-release` to hold in sync — the stem is a
function of three fields it already owns. `build.sh` cuts every listed arch by
default; `--arch <arch>` cuts one, which is how CI gets a job per target.

`version` is a **mirror**. The canonical version is `Cargo.toml`'s
`[workspace.package] version` — the only site the binary itself reads, since
`clap` prints it for `sb --version` and Rust cannot read JSON at build time.

**Do not hand-edit `version` or `package_stem`. Run `/sb-release`** — it writes
all seven mirror sites and verifies they agree. Both staging scripts refuse to
run on drift, so a mismatch fails the build rather than shipping a zip whose
name contradicts the binary inside it.

## Rules

- **`dist/<version>/` has no platform segment.** The platform is already in the
  path (`platforms/windows/dist/…`), so the old `dist/<version>/<platform>/`
  nesting collapses by one level.
- **Exactly three shipped files per version**: the payload zip, its `.sha256`,
  and a version-agnostic installer that locates its sibling zip by glob. The
  set can be handed over as-is.
- **Staging clears only the `<version>/` slot it is building.** Cutting Windows
  never touches `platforms/linux/`, so the two platforms of one version can be
  built on different boxes and merged by commit.
- **`dist/` is committed; `target/` is not.** Release artifacts are the record
  of what shipped.
- **Which platform gets built is read from [`.env`](../.env)** at the repo root
  (`OS="windows" | "linux" | "mac"`). No script may branch on `uname`,
  `$OSTYPE`, or `$IsWindows`.
- **Versions are bumped by `/sb-release`, never by hand** — see below.

## Building on CI

Both bundles can be cut by GitHub Actions instead of by hand. The workflows do
not reimplement any packaging: they supply a runner, write the `.env` selector,
and invoke the same `build.sh` / `build.ps1` a maintainer runs locally, so a CI
bundle and a hand-cut bundle cannot diverge.

There is **one** workflow, [build.yml](../.github/workflows/build.yml), and it
starts on exactly two things:

| Trigger | What happens |
|---|---|
| a `v*` tag | check → build every target → publish a GitHub Release |
| manual dispatch | the same, minus the Release |

**Nothing runs per push.** A release build of zenoh + iceoryx2 across four jobs
is expensive, and Windows minutes bill at 2x on a private repo. The trade is
real: lint and tests give no CI feedback until you tag, so run them locally
before cutting. If that becomes a problem the answer is a second, cheap
push-triggered workflow, not putting this one back on every push.

Three jobs, in a chain:

| Job | Runs on | What it does |
|---|---|---|
| `check` | `ubuntu-latest` + `windows-latest` | guards that the tag matches `Cargo.toml`, then `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` |
| `build` | one job per target | writes `.env`, then runs this platform's staging script; uploads the slot as a run artifact |
| `release` | `ubuntu-latest` | on a `v*` tag only: downloads every artifact, asserts all three zips are present, attaches them to a GitHub Release |

`check` gates `build` gates `release`: nothing is packaged unless the tree
passes its own gates, and nothing is published unless it packaged. A mistyped
tag fails in the first seconds, before anything is compiled — the tag is the
release number, so a tag disagreeing with `Cargo.toml` would put three different
version numbers on the zip names, `sb --version`, and the Release title.

`check` runs on Windows as well as Linux because parts of the tree are `cfg`'d
per platform, and a Linux-only clippy run cannot see the Windows arms at all.

The `build` matrix is one job per shipped artifact:

| `arch` | Runner | How |
|---|---|---|
| `linux-x86_64` | `ubuntu-latest` | `build.sh --arch linux-x86_64` |
| `linux-aarch64` | `ubuntu-latest` | `build.sh --arch linux-aarch64`, cross-compiled through zig |
| `windows-x86_64` | `windows-latest` | `build.ps1` |

aarch64 does not get its own runner. zig cross-links against the target's glibc
stubs from the x86_64 image, which also sidesteps the fact that GitHub's ARM
runners are not free on a private repo. `--arch` matters here: a bare `build.sh`
cuts every architecture, and two jobs doing that in parallel would each clear
the other's slot.

`workflow_dispatch` is set, so a run can be forced from the Actions tab on any
branch. That is a full rehearsal short of publishing: `release` is gated on the
ref being a `v*` tag, so a dispatch leaves the artifacts on the run and cannot
create a Release by accident.

Cutting a release is therefore:

```bash
/sb-release                     # bumps all seven mirror sites
git commit -am "release 0.1.41"
git tag v0.1.41                 # the tag mirrors Cargo.toml — checked in CI
git push --follow-tags
```

Three runner facts the workflow encodes, all learned the hard way:

- **`ubuntu-latest` needs disk freed first.** `zenoh` + `iceoryx2` built
  `--all-targets` outgrows what the image leaves spare; the preinstalled
  dotnet / android / ghc / CodeQL trees are dropped up front so the ceiling is
  never reached as a linker SIGBUS.
- **`windows-latest` needs `LIBCLANG_PATH`.** `iceoryx2-pal-posix` runs bindgen
  on Windows only (on Linux it binds POSIX through `libc`), so `clang-sys` has
  to be pointed at the image's LLVM. The probe sets it only when
  `libclang.dll` is really there: a wrong value is worse than an unset one,
  because `clang-sys` stops searching once the variable is set.
- **`ubuntu-latest` cannot link a shippable binary on its own.** It is Ubuntu
  24.04, so a native link bakes in glibc 2.39 and the result will not start on
  22.04, let alone 20.04. The runner installs `cargo-zigbuild` + `ziglang` from
  PyPI and `build.sh` refuses to stage without them. See `glibc_floor` above.

`cargo fmt --all --check` is a CI gate, which is why [rustfmt.toml](../rustfmt.toml)
pins `style_edition = "2024"`. Without the pin the toolchain infers 2021 from
the crate edition and reorders every mixed-case `use` in the workspace.

## Docs

| | |
|---|---|
| Linux build + install | [installguide_ubuntu.md](../installguide_ubuntu.md) |
| Windows build + install | [installguide_windows.md](../installguide_windows.md) |

`mac/` is not created yet. Add it as a sibling when a macOS target lands; nothing
in the layout above assumes there are only two.
