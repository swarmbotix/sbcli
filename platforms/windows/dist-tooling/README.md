# `platforms/windows/dist-tooling/`

Windows packaging source. Counterpart to
[../../linux/dist-tooling/](../../linux/dist-tooling/).

| File | Role | Spec |
|---|---|---|
| [build.ps1](build.ps1) | staging script — builds, stages, zips, checksums | installguide_windows.md §8.1 |
| [install.ps1](install.ps1) | version-agnostic installer, shipped next to the zip | §8.2 |
| [sb.config.win.yml.template](sb.config.win.yml.template) | seeded into new installs (`.dll` paths) | §8.3 |

## Cut a release

```powershell
pwsh -File platforms\windows\dist-tooling\build.ps1
```

Reads `OS` from [`.env`](../../../.env) at the repo root, then every build
fact — `triple`, `binary`, `arch`, `package_stem` — from
[../version.json](../version.json). Refuses to run if that descriptor's
`version` disagrees with `Cargo.toml` (run `/sb-release` to resync).

Output lands in `../dist/<version>/`: the payload zip, its `.sha256`, and a
copy of `install.ps1`. Flags: `-SkipBuild` reuses the existing
`target/<triple>/release/`, `-NoZip` stops after staging so you can inspect
the tree.

## Notes

`install.ps1` is a rewrite of `install.sh`, not a port — the Linux installer
leans on `readlink -f`, `mktemp -d`, `unzip`, `install -m 0755`, `rsync`,
`sed -i`, `hostname -s`, `/dev/tty`, and `~/.bashrc`. What carries over is the
behavior contract: never clobber user-edited `.proto` files, replace
`documents/` wholesale, seed `sb.config.yml` only when absent, stay idempotent
on re-run. installguide_windows.md §8.2 maps it step for step.

Three Windows-specific traps, all handled in the scripts and worth preserving
if you edit them:

- **robocopy exit codes 0–7 mean success.** A naive `-ne 0` check fails every
  run.
- **`Compress-Archive -Path $Stage`**, not `$Stage\*` — the package dir must be
  the zip's single top-level entry, matching `zip -rq $PKG.zip $PKG` on Linux.
  The flattened form silently breaks `install.ps1`'s layout assertion.
- **PATH is read from and written to the `User` scope only.** Never build the
  new value from `$env:Path` — that is the merged machine+user+session string,
  and writing it back into `User` permanently duplicates every machine entry.
