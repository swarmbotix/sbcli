# `sb tools install`: Code-Generator Install Reference

Official reference for `sb tools install`, which downloads the `protoc` and
`flatc` releases this `sb` is pinned to, unpacks them into a fixed folder
layout, and points `sb.config.yml` at them.

This document is normative for behavior shipped in `sb 0.9.0+`.

The canonical spec is `requirements.md` in the sbcli **source
repository**, a maintainer document that is not part of your install;
when it and this page disagree, it wins.

Related: [sbcli_doctor.md](sbcli_doctor.md) §4.1 / §4.2 (the two rows
this command prints), [sbcli_config.md](sbcli_config.md) §1 (where the
global `sb.config.yml` is), [sbcli_messages.md](sbcli_messages.md) §9.3
(what needs `protoc` and `flatc`), [sbcli_update.md](sbcli_update.md)
(the same download-and-verify pattern, for `sb` itself).

---

## 0. A fresh Linux machine, start to finish

From nothing to a green `sb doctor`, non-interactively, one line at a
time:

```bash
curl -fsSL https://github.com/swarmbotix/sbcli/releases/latest/download/get-sb.sh | bash -s -- --yes
export PATH="$HOME/.swarmbotix/bin:$PATH"
sudo mkdir -p /opt/protobuf /opt/flatbuffers && sudo chown "$USER" /opt/protobuf /opt/flatbuffers
sb tools install --yes
sb message compile
sb doctor
```

| Line | What it does |
|---|---|
| `curl ... get-sb.sh \| bash -s -- --yes` | Downloads the latest release zip for this machine (`linux-x86_64` or `linux-aarch64`), checks its SHA-256, and runs the `install.sh` inside it with `--yes`. Installs into `~/.swarmbotix/` |
| `export PATH=...` | Puts `sb` on this shell's `PATH`. The installer already appended the same line to `~/.bashrc` for future shells, but a running shell, and any non-interactive one, never reads that file; without this line the next `sb` is `command not found` |
| `sudo mkdir ... && sudo chown ...` | The **only** line that needs root. Creates the two folders under `/opt` and gives them to you (§4) |
| `sb tools install --yes` | Installs `protoc` 35.0 and `flatc` 25.12.19 into those folders and writes their paths into `~/.swarmbotix/sb.config.yml` (§5) |
| `sb message compile` | Generates the bindings for every message style, now that `protoc` is configured |
| `sb doctor` | Exits 0. `[SKIP]` rows for `libzenohc`, `libiceoryx2` and `tmux` are expected until you install those (§9) |

`get-sb.sh` ends with the same steps under `Next steps:`: the `export PATH`
line (only when `sb` is not on `PATH` yet), the `sudo` line listing only
the `/opt` folders that are actually missing or not writable (none if
they already are; only `/opt/protobuf` on aarch64), then
`sb tools install --yes`, `sb message compile` and `sb doctor`. The block
above is the same thing written out for a fresh machine, so it can be
run without reading that output first.

Run it from a folder with no `messages/` directory in it (your home
directory is fine): `sb message compile` and `sb doctor` prefer a
`messages/` tree found from the current directory over the installed
one ([sbcli_messages.md](sbcli_messages.md) §1).

**`sb` never runs `sudo`.** The `sudo` line is a separate step on
purpose: a script or an AI agent cannot type a password, so a tool that
called `sudo` itself would hang or fail at a prompt nobody can answer.
Either a person runs that one line, or the machine allows passwordless
`sudo`, or you skip it and install under a folder you already own with
`--prefix` (§4.1). Everything else runs as you.

On a Raspberry Pi or Jetson (`linux-aarch64`) the same block works, but
installs `protoc` only: see §6.

---

## 1. Conceptual model

`sb` links Zenoh and iceoryx2 into its own binary, but it shells out to
two code generators it does not carry:

| Tool | Pinned version | Needed for |
|---|---|---|
| `protoc` | `35.0` | every `sb message compile`, including `--iox2`-only (the descriptor IR comes from `protoc`), and `sb topic listen` decoding |
| `flatc` | `25.12.19` | the `fb` backend (`sb message compile --fb`) and FlatBuffers decoding only |

Each `sb` release is tested against exactly one version of each. That
version is compiled into `sb`, together with the official GitHub release
asset and its SHA-256. `sb tools install`:

1. downloads that asset for this platform,
2. checks its SHA-256 against the pinned one,
3. unpacks it into a fixed folder under a prefix (`/opt` on Linux,
   `C:/cmake_ext_libs` on Windows),
4. runs the unpacked binary's `--version` and requires the pinned version,
5. writes `protoc:` and `flatc:` into the global `sb.config.yml`,
6. prints the same two rows `sb doctor` would print.

It is not a package manager. It changes no `PATH`, touches nothing
outside the prefix and `sb.config.yml`, and never asks for elevated
rights. Run it again at any time: a tool already at its pinned version
is left alone.

`sb doctor` compares the configured `protoc` and `flatc` with the same
pins, and its rows name `sb tools install` as the fix for each problem
it can solve ([sbcli_doctor.md](sbcli_doctor.md) §4.1).

### 1.1 Files involved

Linux x86_64, default prefix:

```
/opt/
├── protobuf/
│   └── protoc-35.0-linux-x86_64/       the protoc release zip, unpacked as is
│       ├── bin/protoc
│       ├── include/google/protobuf/*.proto
│       └── readme.txt
└── flatbuffers/
    └── flatc-25.12.19/
        └── flatc                       the one file in the flatc zip, made executable

~/.swarmbotix/sb.config.yml             protoc: and flatc: rewritten
```

---

## 2. Pins and assets

All assets come from the official upstream GitHub releases.

| Platform | Tool | Asset | SHA-256 |
|---|---|---|---|
| `linux-x86_64` | `protoc` | `protoc-35.0-linux-x86_64.zip` | `a45cda0989c17dd950db55f6fbe1e5814c50fda08e87aa422980ac1f89dddbbc` |
| `linux-aarch64` | `protoc` | `protoc-35.0-linux-aarch_64.zip` | `36b518ac14d90351cc6598228ed2bbe5afe4e357b1af470b07e0ec1609875de2` |
| `windows-x86_64` | `protoc` | `protoc-35.0-win64.zip` | `d1cede9e308cc3eb072392af1c02ccae4bdd3d2f374ec2970dbd8cdfdaa91363` |
| `linux-x86_64` | `flatc` | `Linux.flatc.binary.g++-13.zip` | `9f87066dc5dfa7fe02090b55bab5f3e55df03e32c9b0cdf229004ade7d091039` |
| `linux-aarch64` | `flatc` | none published upstream (§6) | — |
| `windows-x86_64` | `flatc` | `Windows.flatc.binary.zip` | `fff9445c9db907227bc64b54cc98743084c4949282aa4e576cff6a955724ddc8` |

Download locations:

- `protoc`: `https://github.com/protocolbuffers/protobuf/releases/download/v35.0/<asset>`
- `flatc`: `https://github.com/google/flatbuffers/releases/download/v25.12.19/<asset>`

The platform is the one this `sb` was built for, the same names as
[sbcli_update.md](sbcli_update.md) §4.2.

---

## 3. Install layout

`dir` is relative to the prefix; the executable is relative to `dir`.

| Platform | Tool | `dir` | Executable |
|---|---|---|---|
| `linux-x86_64` | `protoc` | `protobuf/protoc-35.0-linux-x86_64` | `bin/protoc` |
| `linux-aarch64` | `protoc` | `protobuf/protoc-35.0-linux-aarch_64` | `bin/protoc` |
| `linux-*` | `flatc` | `flatbuffers/flatc-25.12.19` | `flatc` |
| `windows-x86_64` | `protoc` | `protoc/35.0` | `bin/protoc.exe` |
| `windows-x86_64` | `flatc` | `flatbuffers/25.12.19` | `flatc.exe` |

The resulting `sb.config.yml` on Linux x86_64:

```yaml
protoc: /opt/protobuf/protoc-35.0-linux-x86_64/bin/protoc
flatc:  /opt/flatbuffers/flatc-25.12.19/flatc
```

On Windows:

```yaml
protoc: C:/cmake_ext_libs/protoc/35.0/bin/protoc.exe
flatc:  C:/cmake_ext_libs/flatbuffers/25.12.19/flatc.exe
```

The version is part of every folder name, so a later `sb` that pins
newer tools installs beside the old ones instead of over them.

---

## 4. Privilege: `sb` never runs `sudo`

The folders that must be writable are `<prefix>/<first part of dir>`:
on Linux `/opt/protobuf` and `/opt/flatbuffers`. `/opt` itself belongs to
root, so on a fresh machine they have to be created once, as root:

```bash
sudo mkdir -p /opt/protobuf /opt/flatbuffers && sudo chown "$USER" /opt/protobuf /opt/flatbuffers
```

When one of them does not exist or is not writable, `sb tools install`
prints the table (§5.1), then stops with exit code 1 before downloading
anything:

```
error: /opt/protobuf, /opt/flatbuffers not writable; run this once, then re-run `sb tools install`:
  sudo mkdir -p /opt/protobuf /opt/flatbuffers && sudo chown "$USER" /opt/protobuf /opt/flatbuffers
```

The line lists only the folders that are actually missing or not
writable, and only for tools that still need installing. Copy it, run it
(or have someone with the password run it), then run
`sb tools install` again.

Why `sb` does not do this itself: a password prompt cannot be answered
by a script, a CI job or an AI agent, and a tool that escalates
privileges on its own is one you have to trust with root. Printing the
exact line keeps the one privileged step visible and in your hands.

### 4.1 Without `sudo` at all

Install under a folder you own:

```bash
sb tools install --prefix ~/tools
```

The layout under the prefix is the same (`~/tools/protobuf/...`,
`~/tools/flatbuffers/...`), and `sb.config.yml` points there. Nothing
else changes.

### 4.2 Windows

`C:/cmake_ext_libs` can be created by a normal user on a standard
Windows install, so there is no elevated step. If the prefix cannot be
created, `sb tools install` prints `mkdir` lines for the folders it
needs instead of a `sudo` line.

---

## 5. `sb tools install`

```
sb tools install [--prefix DIR] [--only protoc|flatc] [-y|--yes]
```

| Flag | Effect |
|---|---|
| `--prefix DIR` | Install under `DIR` instead of `/opt` (Windows: `C:/cmake_ext_libs`) |
| `--only protoc\|flatc` | Only this tool. Case-insensitive. Anything else: ``unknown tool `<x>`; expected protoc or flatc`` |
| `-y`, `--yes` | Do not ask before installing. Required when stdin is not a terminal |

### 5.1 Output

A fresh Linux x86_64 machine, after the `sudo` line of §4:

```
$ sb tools install --yes
prefix    : /opt
protoc    : 35.0       missing      -> /opt/protobuf/protoc-35.0-linux-x86_64/bin/protoc
flatc     : 25.12.19   missing      -> /opt/flatbuffers/flatc-25.12.19/flatc
downloading https://github.com/protocolbuffers/protobuf/releases/download/v35.0/protoc-35.0-linux-x86_64.zip ...
verifying sha256
unpacking into /opt/protobuf/protoc-35.0-linux-x86_64
downloading https://github.com/google/flatbuffers/releases/download/v25.12.19/Linux.flatc.binary.g++-13.zip ...
verifying sha256
unpacking into /opt/flatbuffers/flatc-25.12.19
installed: protoc 35.0 -> /opt/protobuf/protoc-35.0-linux-x86_64/bin/protoc
installed: flatc 25.12.19 -> /opt/flatbuffers/flatc-25.12.19/flatc
set protoc in /home/user/.swarmbotix/sb.config.yml
set flatc in /home/user/.swarmbotix/sb.config.yml
[OK]   protoc       /opt/protobuf/protoc-35.0-linux-x86_64/bin/protoc (libprotoc 35.0)
[OK]   flatc        /opt/flatbuffers/flatc-25.12.19/flatc (flatc version 25.12.19)
```

The progress lines (`downloading ...`, `verifying sha256`,
`unpacking into ...`) go to **stderr**; everything else goes to stdout.
The last two lines are exactly what `sb doctor` prints for these two
checks.

The table's state column is one of:

| State | Meaning |
|---|---|
| `installed` | the executable exists, and the first line of its `--version` contains the pinned version |
| `missing` | the executable does not exist |
| `stale (<line>)` | the executable exists, but its `--version` reports something else, or fails; `<line>` is what it said |
| `unavailable (<reason>)` | no prebuilt asset for this platform; the row has no `->` path (§6) |

`missing` and `stale` rows are installed; `stale` ones are replaced.

### 5.2 Steps, in order

1. **Resolve** the prefix (`--prefix`, else the platform default) and
   `--only`.
2. **Plan:** for each tool, its target folder, its executable, and its
   state (table above).
3. **Print the table.**
4. **Nothing to do?** When no tool is `missing` or `stale`, print
   `nothing to do: every available tool is at its pinned version`, still
   point `sb.config.yml` at any installed tool whose configured path
   differs, print the doctor rows, and exit 0.
5. **Privilege check:** any target parent missing or unwritable stops
   here with the `sudo` line (§4). Nothing has been downloaded.
6. **Confirm:** `Proceed? [y/N]` on a terminal, unless `--yes`. Without a
   terminal and without `--yes`: `not a terminal; pass --yes`. Answering
   no exits 1 with `cancelled`.
7. **Install** each `missing` / `stale` tool: download into a temporary
   folder, verify the SHA-256 (a mismatch deletes the download and stops),
   unpack, mark the executable `0755` on Linux, replace the target folder,
   then run `<exe> --version` and require the pinned version. Prints
   `installed: <tool> <version> -> <exe>` per tool.
8. **Configure:** `sb config set protoc <exe>` / `sb config set flatc <exe>`
   for every tool just installed, and for every already-installed tool
   whose configured path differs. Each prints `set <tool> in <path>`.
   This writes the global `sb.config.yml`, the same file `sb config set`
   writes ([sbcli_config.md](sbcli_config.md) §4.2).
9. **Report:** reload the config and print the `protoc` and `flatc`
   doctor rows.
10. **Notes:** for each tool with no asset on this platform,
    `note: <tool>: <reason>` on stderr. Exit 0.

### 5.3 Second run

```
$ sb tools install --yes
prefix    : /opt
protoc    : 35.0       installed    -> /opt/protobuf/protoc-35.0-linux-x86_64/bin/protoc
flatc     : 25.12.19   installed    -> /opt/flatbuffers/flatc-25.12.19/flatc
nothing to do: every available tool is at its pinned version
[OK]   protoc       /opt/protobuf/protoc-35.0-linux-x86_64/bin/protoc (libprotoc 35.0)
[OK]   flatc        /opt/flatbuffers/flatc-25.12.19/flatc (flatc version 25.12.19)
```

No network, no `sudo` line, exit 0. This is also the quickest way to
re-point a config that was edited by hand back at the pinned tools.

---

## 6. Linux aarch64 (Raspberry Pi, Jetson): no prebuilt `flatc`

The FlatBuffers project publishes no Linux aarch64 `flatc`. On those
machines `sb tools install` installs `protoc` and reports `flatc` as
unavailable:

```
$ sb tools install --yes
prefix    : /opt
protoc    : 35.0       missing      -> /opt/protobuf/protoc-35.0-linux-aarch_64/bin/protoc
flatc     : 25.12.19   unavailable (no prebuilt flatc 25.12.19 for linux-aarch64; build it from source: https://github.com/google/flatbuffers)
...
installed: protoc 35.0 -> /opt/protobuf/protoc-35.0-linux-aarch_64/bin/protoc
set protoc in /home/user/.swarmbotix/sb.config.yml
[OK]   protoc       /opt/protobuf/protoc-35.0-linux-aarch_64/bin/protoc (libprotoc 35.0)
[SKIP] flatc        not set in sb.config.yml (run `sb tools install`)
note: flatc: no prebuilt flatc 25.12.19 for linux-aarch64; build it from source: https://github.com/google/flatbuffers
```

The exit code is 0, and `sb doctor` stays green with `[SKIP] flatc`.
That is the expected steady state on aarch64: `sb message compile`
emits the `fb` backend only when `flatc` is configured, and `protoc`
covers everything else. The ``(run `sb tools install`)`` hint on that row
cannot be satisfied here; ignore it.

Only if you need FlatBuffers on that machine: build `flatc` 25.12.19
from source (https://github.com/google/flatbuffers), then
`sb config set flatc <path to flatc>`.

The `sudo` line of §0 creates `/opt/flatbuffers` on aarch64 too. It
stays empty and does no harm; `sb tools install` itself asks only for
`/opt/protobuf` there.

---

## 7. Environment variables

| Variable | Effect |
|---|---|
| `SB_TOOLS_BASE=file:///<dir>` | Copy each asset from `<dir>/<asset>` instead of downloading it (asset names as in §2). When `<dir>/<asset>.sha256` exists, its first word replaces the pinned SHA-256: that is for offline tests only, so never put a `.sha256` file there for a real install. Any value that is not a `file:///` URL is an error. Unset or empty: download from GitHub |

For a machine without internet access, copy the official zips of §2
into one folder, then run
`SB_TOOLS_BASE=file:///<that folder> sb tools install --yes`. Without
`.sha256` files beside them, the pinned hashes are enforced as usual.

---

## 8. Error catalogue

| Message | Cause | Fix |
|---|---|---|
| ``unknown tool `<x>`; expected protoc or flatc`` | `--only` with another name | `--only protoc` or `--only flatc` |
| ``<dirs> not writable; run this once, then re-run `sb tools install`:`` followed by the `sudo` line | target folders missing or owned by root | run the printed line, then re-run (§4); or `--prefix` (§4.1) |
| `not a terminal; pass --yes` | no stdin to ask on | add `-y` |
| `cancelled` | answered no | nothing to do |
| `sha256 mismatch for <asset>: pinned <hex>, download is <hex>` | corrupt, truncated or replaced download | run again; if it repeats, the upstream asset changed: report it |
| `GET <url>: ...` | offline, DNS, proxy, or GitHub unreachable | retry with network, or use `SB_TOOLS_BASE` (§7) |
| the installed tool does not report the pinned version | the asset unpacked to something else | report it |

---

## 9. What this does not install

`sb tools install` handles `protoc` and `flatc` only. The other tools
`sb doctor` checks are separate host installs, `[SKIP]` until configured,
and only needed for the features that use them:

| Doctor row | Needed for | Configure with |
|---|---|---|
| `libzenohc` | C/C++ consumers on Zenoh | `sb config set libzenohc <path>` |
| `libiceoryx2` | C/C++ consumers on iceoryx2 | `sb config set libiceoryx2 <path>` |
| `tmux` | `sb up` and the other launch verbs | install `tmux` (Windows: psmux) |

---

## 10. Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `sb: command not found` right after `get-sb.sh` | the `PATH` line went into `~/.bashrc`, which this shell has not read | `export PATH="$HOME/.swarmbotix/bin:$PATH"`, or open a new terminal |
| `[WARN] protoc` in `sb doctor`, saying `sb pins protoc 35.0` | `protoc:` points at another version, e.g. a distribution package | `sb tools install --yes`; it installs the pinned one beside it and re-points the config. The other copy is not touched |
| `[FAIL] protoc protoc not found at <path> ...` | the configured file was deleted or moved | `sb tools install --yes` |
| `sb tools install` succeeds, but `sb doctor` still shows the old path | a workspace `sb.config.yml` or `$SB_CONFIG` sets `protoc:` / `flatc:` and wins over the global file | remove the key there ([sbcli_config.md](sbcli_config.md) §3) |
| `[SKIP] flatc` on a Pi or Jetson | no prebuilt aarch64 `flatc` | expected (§6) |
| removing the tools | — | delete the two folders under the prefix; `sb doctor` then fails on the dead paths until you set new ones |
