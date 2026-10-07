# `sb update` / `sb --version`: Self-Update Reference

Official reference for keeping `sb` current: the update notice under
`sb --version`, `sb update --check`, and `sb update` itself.

This document is normative for behavior shipped in `sb 0.2.1+`.

The canonical spec is `requirements.md` in the sbcli **source
repository**, a maintainer document that is not part of your install;
when it and this page disagree, it wins.

Related: [sbcli_config.md](sbcli_config.md) §2 (where `<sb_home>` is),
[sbcli_doctor.md](sbcli_doctor.md) (the `sb <version>` header line).

---

## 1. Conceptual model

Releases are published at `github.com/swarmbotix/sbcli` as tag `vX.Y.Z`
with one zip per platform. `sb` reads that page itself; no account, no
token, no GitHub API. Three surfaces use the same lookup:

| Surface | Network | Changes anything? |
|---|---|---|
| `sb --version` | at most once per 24 h, cached, silent on failure | writes the cache only |
| `sb update --check` | always | writes the cache only |
| `sb update` | always | replaces the installed `sb` |

Only an **installed** `sb` updates itself: the running binary must be
`<sb_home>/bin/sb` (or `sb.exe`). A development build from `cargo` is
refused and told to rebuild with cargo.

### 1.1 Files involved

```
<sb_home>/
├── bin/sb                    the binary that is replaced
├── VERSION                   written by the installer: version, build date, platform
├── update-check.json         cache: latest known release and when it was checked
├── documents/                replaced wholesale by every update
├── messages/                 new bundled files added; your edits kept
├── sb.config.yml             kept
├── workspaces/  apps.yml     kept
```

---

## 2. `sb --version`

```
$ sb --version
sb 0.2.1
update available: 0.2.3   run `sb update`
```

Line 1 is always printed and is the only line scripts should parse. Line 2
appears only when the lookup succeeds: `update available: X.Y.Z   run
`sb update`` when a newer release exists, `(latest)` when `sb` is at the
latest release. When `sb` is newer than the latest release (a build ahead
of the release page), or the lookup fails, nothing is added.

Rules that keep `--version` safe to call anywhere:

- **Cache first.** `<sb_home>/update-check.json` is reused for 24 hours.
  Within that window `sb --version` touches no network at all.
- **Hard time limit** of 4 seconds on the lookup, including DNS. On a
  host without DNS the notice is simply absent.
- **Never changes the exit code.** `sb --version` exits 0 whatever the
  lookup did.
- **Opt out** with `SB_NO_UPDATE_CHECK=1` (any value other than empty or
  `0`): prints exactly one line, touches neither network nor cache. Use it
  in CI, build scripts and offline robots.

`-V` is the short form.

---

## 3. `sb update --check`

Always asks the release page, refreshes the cache, changes nothing else.

```
$ sb update --check
installed : 0.2.1
latest    : 0.2.3
platform  : linux-x86_64
asset     : https://github.com/swarmbotix/sbcli/releases/download/v0.2.3/swarmbotix-0.2.3-linux-x86_64.zip
status    : update available   run `sb update`
```

| `status` | Exit code |
|---|---|
| `up to date` | 0 |
| `ahead (newer than the latest release)` | 0 |
| `update available   run `sb update`` | 10 |

Exit code 10 makes it scriptable:

```bash
sb update --check || sb update -y     # update only when something new exists
```

`--check` cannot be combined with `--version` or `--force`.

---

## 4. `sb update`

```
sb update [--version X.Y.Z] [-y|--yes] [--force]
```

```
$ sb update
sb 0.2.1 -> 0.2.3
Proceed? [y/N] y
updated: sb 0.2.3
```

### 4.1 Steps, in order

1. **Guard:** the running executable must be `<sb_home>/bin/sb`. Otherwise:
   `this sb is <path>, not the installed <sb_home>/bin/sb; sb update only updates an installed sb (a dev build updates itself with cargo)`.
2. **Resolve the target:** `--version X.Y.Z` (a leading `v` is accepted), else
   the latest release. Same as the running version: refused unless `--force`.
   Older than the running version: allowed, with a downgrade warning.
3. **Confirm:** `sb <old> -> <new>` then `Proceed? [y/N]` on a terminal.
   `--yes` skips the question; without a terminal `--yes` is required.
   Answering no exits 1 with `cancelled`.
4. **Download** `swarmbotix-<ver>-<platform>.zip` and its `.sha256` into a
   temporary folder outside `<sb_home>`.
5. **Verify** the SHA-256 against the sidecar. A mismatch aborts and
   deletes the download.
6. **Unpack** and run the package's own installer with `SB_HOME` set to
   this `sb`'s home: `install.sh --yes` on Linux, `install.ps1 -Yes` on
   Windows. The result is identical to installing that zip by hand (see
   the install guide shipped with the package): `bin/sb`, `documents/` and
   `VERSION` are replaced, new bundled message files are added, and
   `sb.config.yml`, workspaces, edited message files and installed apps are
   kept.
7. **Confirm the binary:** the new `<sb_home>/bin/sb --version` must
   report the target version, else the error names what it reported.
8. **Refresh the cache** when the installed version is the latest release.

Only the last lines of the installer's output are shown, and only on
failure.

### 4.2 Platform mapping

| Build target | Asset suffix |
|---|---|
| Linux x86_64 | `linux-x86_64` |
| Linux aarch64 | `linux-aarch64` |
| Windows x86_64 | `windows-x86_64` |

Any other build reports `no release asset for this platform`.

### 4.3 Windows

A running `sb.exe` cannot be overwritten, so `sb update` first renames it
to `sb.exe.old`, then installs. Every later start of `sb` deletes the
`.old` file if it is still there. If the installer fails, the old binary
is put back. If a leftover `sb.exe.old` cannot be removed, the error asks
you to close any `sb` still running from it.

---

## 5. Environment variables

| Variable | Effect |
|---|---|
| `SB_NO_UPDATE_CHECK=1` | `sb --version` prints one line and does no lookup |
| `SB_HOME` | honoured by the installer `sb update` runs; `sb` passes the home it resolved, so normally nothing to set |
| `SB_RELEASE_BASE` | alternative release location (`https://...` of a fork, or `file:///dir` holding `latest.txt` and the zips). Testing only |
| `SB_UPDATE_ALLOW_ANY_EXE=1` | skips the installed-binary guard. Testing only |

---

## 6. Error catalogue

| Message | Cause | Fix |
|---|---|---|
| `this sb is <path>, not the installed <sb_home>/bin/sb; ...` | running a dev build | `cargo build`, or run the installed one |
| `already at X.Y.Z (use --force to reinstall)` | target equals running version | `--force`, or nothing to do |
| `not a terminal; pass --yes` | no stdin to ask on | add `-y` |
| `sha256 mismatch for <zip>: the release says <a>, the download is <b>; deleted the download` | corrupt or tampered download | run again; if it repeats, report the release |
| `no release asset for this platform (<os>-<arch>); releases exist for ...` | unsupported build target | install by hand from source |
| `installed binary reports `sb X`, expected `sb Y` (<path>)` | installer put a different binary in place | inspect `<sb_home>/bin/sb`, reinstall by hand |
| `GET <url>: ...` (HTTP status, transport error, or `expected a redirect to the latest release`) with `--check` or `update` | offline, DNS, or GitHub unreachable | retry with network; `--version` never shows this |
| `unexpected zip layout: ...` | asset is not a swarmbotix package | report the release |

---

## 7. What is not updated

`sb update` updates `sb` only. Installed app packages keep their own
versions and update with `sb app update` (see [sbcli_app.md](sbcli_app.md)
§8). The transport libraries (`libzenohc`, `libiceoryx2`) are host
installs checked by `sb doctor`, not part of the zip.
