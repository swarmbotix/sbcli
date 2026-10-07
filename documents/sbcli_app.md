# `sb app` / `sb install`: App Package Reference

Official reference for app packages: how a folder becomes a package that
`sb` can install from a git URL or a local path, and how an installed
package runs as `sb <name> [args...]` without any change to `PATH`.

This document is normative for behavior shipped in `sb 0.2.1+`.

The canonical spec is `requirements.md` in the sbcli **source
repository**, a maintainer document that is not part of your install;
when it and this page disagree, it wins.

Related: [sbcli_init.md](sbcli_init.md) (modules, a different concept:
long-running pub/sub processes launched under tmux),
[sbcli_doctor.md](sbcli_doctor.md) (the `app` lines `sb doctor` prints).

---

## 1. Conceptual model

An **app package** is any folder (usually a git repository) that holds an
`sb.app.yml` manifest at its root. The manifest names one **entry**, a
script or binary inside the folder. After `sb install`, the package's
name becomes an `sb` subcommand:

```
sb install https://github.com/ubicoders/sb_kalibr.git
sb camcalib ~/data/mycam --models pinhole-equi
```

`sb camcalib ...` runs the package's entry in **your current directory**,
with **your arguments verbatim**, and exits with **the entry's exit code**.
`sb` is only the dispatcher; it adds nothing to the process other than
three environment variables (see §6).

An app is not a module. Modules (`sb init`) are pub/sub processes that a
workspace launches under tmux. Apps are foreground tools: calibration,
converters, code generators, anything you would otherwise run from a
cloned repository. Apps are global to the host and do not depend on the
active workspace.

Two roles, two commands:

| Role | Does | Reads docs? |
|---|---|---|
| package author | `sb app init` once in the repository, commit, push | no, the generated manifest is self-explaining |
| user | `sb install <url \| path>`, then `sb <name> ...` | no |

### 1.1 Files involved

```
~/.swarmbotix/                        (sb home, see sbcli_config.md §2)
├── apps.yml                          registry: name -> where it is, where it came from
└── apps/<name>/                      clones that sb made from a git URL (sb owns these)

/path/to/<package>/                   a package installed from a local folder (stays in place)
├── sb.app.yml                        manifest, written by `sb app init`
├── <entry>                           what `sb <name>` runs
└── install.bash                      only for kind: host
```

---

## 2. Command surface

```
sb app init    [PATH] [--name N] [--entry FILE] [--kind docker|host|none] [--image IMG] [--description TEXT] [--force]
sb install     <URL|PATH> [--prefetch]
sb app install <URL|PATH> [--prefetch]          same as sb install
sb app list                                     alias: sb app ls
sb app info    <NAME>
sb app update  [NAME]
sb app remove  <NAME>                           alias: sb app rm
sb <NAME> [args...]                             run an installed app
```

`sb <verb> --help` is the authority for flags on the build you have.

---

## 3. `sb app init` (package author)

Makes a folder a package. Run it once at the root of the repository you
want to share.

```bash
cd ~/dev/sb_kalibr
sb app init --name camcalib --entry ./camcalib --kind docker --image swarmbotix/sb_kalibr:latest
```

Output:

```
created app package camcalib (kind: docker, root: /home/el/dev/sb_kalibr)
  wrote sb.app.yml
  entry ./camcalib (existing file, left as is)
next:
  sb install /home/el/dev/sb_kalibr   then run: sb camcalib [args...]
  commit and push; others run: sb install <git-url>
```

### 3.1 Defaults and prompts

| Field | Flag | Default |
|---|---|---|
| name | `--name` | folder name, with characters other than letters, digits, `_` turned into `_`; a leading digit gets `_` prefixed |
| entry | `--entry` | `./<name>` if that file exists, else `./run.bash` (created as a stub) |
| kind | `--kind` | asked on an interactive terminal; otherwise `none`, or `docker` when `--image` is given |
| image | `--image` | asked when kind is `docker` and no image was given |
| description | `--description` | none |

Without `--kind`, on a terminal, `sb` asks for name, entry, kind and (for
docker) image, offering the defaults in brackets. Scripts and CI pass
the flags and are never prompted.

### 3.2 What it writes

| File | When | Content |
|---|---|---|
| `sb.app.yml` | always; refused if present unless `--force` | the manifest in §5, every line commented |
| `<entry>` | only if the entry file does not exist | executable bash stub: prints a TODO and exits 1 until you put your command in it |
| `install.bash` | `--kind host`, only if absent | executable bash stub: exits 0 until you add setup steps |

Existing scripts are never overwritten, with or without `--force`. The
manifest is validated before it is written, so a freshly initialised
package always installs.

### 3.3 Name rule

Letters, digits and `_`, not starting with a digit (the same rule as
module and workspace names, see [sbcli_init.md](sbcli_init.md) §3), and
**not an `sb` built-in command**: `doctor`, `message`, `config`, `ws`,
`init`, `list`, `pub`, `sub`, `service`, `topic`, `up`, `run`, `stop`,
`down`, `attach`, `gopro`, `install`, `app`, `help`. `sb app init`
refuses a built-in name at init time, so the collision is never
discovered by a user.

---

## 4. `sb install` (user)

```bash
sb install https://github.com/ubicoders/sb_kalibr.git        # clone and install
sb install https://github.com/ubicoders/sb_kalibr.git@v1.2   # pin a tag or branch
sb install git@github.com:ubicoders/sb_kalibr.git            # over ssh
sb install ~/dev/sb_kalibr                                   # local folder, in place
sb install ~/dev/sb_kalibr --prefetch                        # also pull the docker image now
```

### 4.1 Source detection

| Argument | Treated as |
|---|---|
| starts with `https://`, `http://`, `ssh://`, `git://`, `file://`, `git@` | git URL |
| ends with `.git` (URL or local path) | git URL |
| anything else | local folder; `~` expanded, path canonicalised, must exist |

`<url>@<ref>` pins a branch or tag; refs containing `/` cannot be pinned
this way.

### 4.2 Where the package ends up

| Source | Location | Owned by sb? |
|---|---|---|
| git URL | `<sb_home>/apps/<name>/`, shallow clone; `<name>` comes from the manifest, not the URL | yes: `sb app remove` deletes it |
| local folder | where it already is; never copied | no: `sb app remove` only unregisters it |

A local folder installed in place is live: edits to it apply on the next
`sb <name>` run, with no reinstall. That is the intended loop for a
package author testing their own package (`sb install .`).

### 4.3 Steps, in order

1. Resolve the source (clone, or canonicalise the path).
2. Read and validate `sb.app.yml` (§5).
3. Claim the name. A built-in name is an error. A name already registered
   to a different folder is an error naming that folder. The same folder
   again is a reinstall.
4. Run the kind's install step:

   | `kind` | `sb install` does |
   |---|---|
   | `none` | nothing more |
   | `docker` | `docker pull <docker.image>` only when `docker.prefetch: true` or `--prefetch`; otherwise prints a note and leaves the pull to the app's first run |
   | `host` | `bash <host.install>` with cwd = package root, stdio inherited; non-zero exit aborts the install |

5. Check every binary in `requires` on `PATH`. Missing ones are **warnings**,
   printed and recorded, never errors.
6. Write the registry entry (§7).

Output of a successful install:

```
installed app camcalib 0.1.0 (kind: docker, source: path)
  path: /home/el/dev/sb_kalibr
  note: image swarmbotix/sb_kalibr:latest is pulled on first run (prefetch: false); pass --prefetch to pull it now
run it: sb camcalib [args...]
```

### 4.4 Reinstalling and re-pointing

| Situation | Result |
|---|---|
| same git URL again | `git pull --ff-only` in the existing clone (same as `sb app update`) |
| same URL, different `@ref` | the clone is replaced |
| same name, different folder or URL | error: `an app named `X` is already installed from <origin>; run `sb app remove X` first` |
| leftover unregistered folder at `<sb_home>/apps/<name>` | replaced silently (sb owns `apps/`) |
| folder at `<sb_home>/apps/<name>` registered to another app | error naming that app |

---

## 5. `sb.app.yml` schema

As written by `sb app init --kind docker --image swarmbotix/sb_kalibr:latest`
in a folder whose entry already exists:

```yaml
# sb.app.yml: swarmbotix app package manifest. Written by `sb app init`.
# `kind` picks what `sb install` does besides registering the app:
#   none    nothing; `entry` runs as shipped
#   docker  `docker pull` docker.image (at install if prefetch: true, else on first run)
#   host    run host.install from the package root, at install and at `sb app update`
name: camcalib             # subcommand: `sb camcalib ...`. Letters, digits, underscore; must not be an sb builtin.
version: 0.1.0
description: Camera calibration in a box (Kalibr in Docker)
entry: ./camcalib          # run with the user's args, from the user's cwd. Relative to this file's folder.
kind: docker               # docker | host | none
docker:                    # only for kind: docker
  image: swarmbotix/sb_kalibr:latest
  prefetch: false          # true = pull at `sb install`; false = app pulls on first run
requires: [docker]         # binaries that must be on PATH; `sb doctor` checks them
```

| Key | Required | Rule |
|---|---|---|
| `name` | yes | §3.3 |
| `version` | yes | non-empty string; shown by `list` / `info`, recorded at install |
| `description` | no | one line, shown by `sb app info` |
| `entry` | yes | relative path inside the package; must exist at install and at every run |
| `kind` | yes | `none`, `docker` or `host` |
| `docker.image` | with `kind: docker` | image reference passed verbatim to `docker pull` |
| `docker.prefetch` | no | default `false` |
| `host.install` | with `kind: host` | relative path inside the package; run with `bash` |
| `requires` | no | list of binary names; checked on `PATH` by `sb install` and `sb doctor` |

Unknown keys are rejected, so a typo fails at `sb install` rather than
being ignored. The consequence is that a manifest using a key added in
a later `sb` release does not install on an older `sb`.

`entry` and `host.install` must stay inside the package folder: an
absolute path or one that climbs out with `..` is refused.

### 5.1 Choosing a kind

| Your tool | `kind` | Why |
|---|---|---|
| runs a container, the script pulls the image itself if missing | `docker`, `prefetch: false` | install stays instant; the first run pays for the pull |
| runs a container, users should wait at install instead | `docker`, `prefetch: true` | |
| needs `pip install`, `apt`, a build step on the host | `host` | put the steps in `install.bash`; it runs again on `sb app update` |
| a plain script or prebuilt binary | `none` | |

---

## 6. Running: `sb <name> [args...]`

```
sb camcalib examples --dry-run
```

1. The name is looked up in `<sb_home>/apps.yml`. Built-in commands always
   win; an app can never shadow one, because §3.3 forbids the name.
2. The entry is resolved relative to the recorded package path and must
   be a file.
3. On Linux and macOS the `sb` process **is replaced** by the entry
   (`exec`), so there is no intermediate process: signals, exit codes and
   terminal ownership are the app's own. On Windows the entry is spawned
   through `bash` and its exit code is returned.
4. The entry runs with:

   | Property | Value |
   |---|---|
   | cwd | your current directory, never the package folder |
   | args | yours, verbatim; `sb` parses nothing after the app name |
   | `SB_HOME` | the resolved sb home |
   | `SB_APP_DIR` | the package folder (absolute) |
   | `SB_APP_NAME` | the app name |
   | exit code | the entry's exit code |

An entry that is not executable but ends in `.bash` or `.sh` is run via
`bash`. An entry needs to locate its own siblings itself; the `camcalib`
script does so with `dirname "${BASH_SOURCE[0]}"`, and `SB_APP_DIR` is
available for the same purpose.

### 6.1 Unknown name

```
$ sb camcalibx
error: `camcalibx` is not an sb command or an installed app. Did you mean `sb camcalib`? Installed apps: camcalib. Install an app with `sb install <url|path>`; `sb --help` lists the built-in commands.
```

Exit code 1. Before `sb 0.2.1`, an unknown verb produced clap's own
error with exit code 2.

### 6.2 Package moved or deleted

```
error: app 'camcalib' path /home/el/dev/sb_kalibr no longer exists; run `sb app remove camcalib` then reinstall
```

---

## 7. The registry: `<sb_home>/apps.yml`

```yaml
# Installed sb apps. Managed by `sb install` and `sb app update|remove`;
# edit with care. `path` under <sb_home>/apps/ is a clone sb owns.
apps:
  camcalib:
    path: /home/el/dev/sb_kalibr
    source: path                      # path | git
    url: null                         # git only
    git_ref: null                     # git only: the @ref given at install, or null
    commit: null                      # git only: HEAD at install / last update
    version: 0.1.0                    # from sb.app.yml at install / last update
    installed_at: 2026-10-07T22:36:29Z
```

A missing file means no apps. The file is global to the host and
independent of workspaces.

---

## 8. `sb app list` / `info` / `update` / `remove`

```
$ sb app list
NAME      VERSION  KIND    SOURCE  PATH
camcalib  0.1.0    docker  path    /home/el/dev/sb_kalibr
```

```
$ sb app info camcalib
name:         camcalib
description:  Camera calibration in a box (Kalibr in Docker)
version:      0.1.0 (installed 2026-10-07T22:36:29Z)
path:         /home/el/dev/sb_kalibr
source:       local folder (installed in place)
kind:         docker (image swarmbotix/sb_kalibr:latest, prefetch false)
entry:        ./camcalib
requires:     docker (/usr/bin/docker)
run:          sb camcalib [args...]
```

`sb app update [NAME]`: for a git source, `git pull --ff-only` (a clone
pinned to a tag reports `pinned` and stays put); a local folder is used
as it is. Then the kind's install step runs again and the recorded
`version` and `commit` are refreshed. Without a name every app is
updated, in registry order, stopping at the first failure.

`sb app remove NAME`: removes the registry entry. The folder is deleted
only when it lives under `<sb_home>/apps/`; a folder installed in place
is never touched.

---

## 9. `sb doctor`

One `app` line per installed app, `[OK]` or `[WARN]`, never `[FAIL]`: a
broken app is the package author's concern and does not fail `sb doctor`.
Nothing is printed when no apps are installed.

```
[OK]   app          camcalib: kind docker, path ok, requires docker ok
[WARN] app          camcalib: kind docker, path /home/el/dev/sb_kalibr MISSING (`sb app remove camcalib`, then reinstall), requires docker ok
[WARN] app          mytool: kind host, path ok, requires ffmpeg MISSING
```

---

## 10. Error catalogue

| Message | Cause | Fix |
|---|---|---|
| `no sb.app.yml in <dir>; an app package needs one at its root (package authors create it with `sb app init`)` | folder is not a package | author runs `sb app init` |
| `sb.app.yml `name`: ...` | name breaks §3.3 | rename in the manifest |
| `an app named `X` is already installed from <origin>; run `sb app remove X` first` | name taken by another folder or URL | remove, then install |
| `app 'X' path <p> no longer exists; run `sb app remove X` then reinstall` | folder moved or deleted after install | as the message says |
| `app 'X': entry <p> is missing; run `sb app update X`, or reinstall it` | entry deleted inside the package | update or reinstall |
| ``git` is not on PATH; install git to install or update apps from a URL` | URL source without git | install git |
| `sb.app.yml has `kind: docker` but no `docker:` section` | manifest edited by hand | add `docker.image` |
| ``host.install` scripts run with bash, but bash is not on PATH` | `kind: host` on a host without bash | install bash |

---

## 11. Worked example: `sb_kalibr`

Author side, once:

```bash
cd ~/dev/sb_kalibr
sb app init --name camcalib --entry ./camcalib --kind docker --image swarmbotix/sb_kalibr:latest
git add sb.app.yml && git commit -m "sb app package" && git push
```

User side:

```bash
sb install https://github.com/ubicoders/sb_kalibr.git
sb camcalib ~/data/mycam            # first run pulls the image (~7 GB)
sb camcalib inspect ~/data/mycam
sb app update camcalib               # later: pull the repository forward
```

Nothing was added to `PATH`, `.bashrc`, or the system. Removing it all:

```bash
sb app remove camcalib               # deletes ~/.swarmbotix/apps/camcalib/
docker rmi swarmbotix/sb_kalibr:latest
```
