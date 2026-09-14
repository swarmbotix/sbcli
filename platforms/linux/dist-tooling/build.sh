#!/usr/bin/env bash
# Stage a swarmbotix Linux release into platforms/linux/dist/<version>/.
#
# Linux counterpart of platforms/windows/dist-tooling/build.ps1. Reads the
# target platform from .env at the repo root (never from `uname` / $OSTYPE)
# and every build fact from platforms/<os>/version.json.
#
# Produces, for EVERY architecture in the descriptor:
#     <package_stem>.zip
#     <package_stem>.zip.sha256
#
# install.sh and uninstall.sh go INSIDE each zip, at the top of the package.
# Nothing loose is staged beside the archives: one download is the whole
# product, and there is no second copy of the installer to drift.
#
# Usage:
#   ./build.sh                 build + stage + zip, every architecture
#   ./build.sh --arch <arch>   just one, e.g. linux-aarch64 (CI builds one per job)
#   ./build.sh --skip-build    reuse target/<triple>/<profile>/; do not run cargo
#   ./build.sh --no-zip        stop after staging the payload dir (inspect the tree)

set -euo pipefail

SKIP_BUILD=0
NO_ZIP=0
ONLY_ARCH=""
while [ $# -gt 0 ]; do
    case "$1" in
        --skip-build) SKIP_BUILD=1 ;;
        --no-zip)     NO_ZIP=1 ;;
        --arch)       shift; ONLY_ARCH="${1:-}"
                      [ -n "$ONLY_ARCH" ] || { echo "error: --arch needs a value" >&2; exit 2; } ;;
        --arch=*)     ONLY_ARCH="${1#--arch=}" ;;
        --help|-h)    sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
    shift
done

for tool in cargo jq zip rsync sha256sum readelf; do
    command -v "$tool" >/dev/null || { echo "error: $tool not on PATH" >&2; exit 1; }
done

# The link step targets an OLD glibc (see `glibc_floor` in version.json), which
# the host toolchain cannot do on its own: linking against the build machine's
# libc bakes in that machine's symbol versions, and a binary built on 24.04
# refuses to load on 22.04. zig ships the glibc stubs for every version, so
# cargo-zigbuild produces the same floor on any distro — a maintainer's zip and
# a CI zip stay the same zip, which is the whole point of this script.
if [ "$SKIP_BUILD" -eq 0 ]; then
    if ! command -v cargo-zigbuild >/dev/null || ! command -v zig >/dev/null; then
        echo "error: cargo-zigbuild and zig are required to stage a Linux release." >&2
        echo "       Install both (they are on PyPI, no system packages needed):" >&2
        echo "         pip install cargo-zigbuild ziglang" >&2
        echo "       and make sure a 'zig' entry point is on PATH." >&2
        echo "       See installguide_ubuntu.md §Prerequisites on the build machine." >&2
        exit 1
    fi
fi

# Copy an executable into place with mode 0755. `install -m` fails outright on
# filesystems that cannot represent unix modes (an NTFS/exFAT mount, which is a
# perfectly normal place to keep a checkout), so fall back to cp + best-effort
# chmod there. install.sh re-applies 0755 on the target machine either way, so a
# mount that pins its own mode bits cannot ship a non-executable binary.
place_exec() {
    local src="$1" dst="$2"
    install -m 0755 "$src" "$dst" 2>/dev/null && return 0
    cp "$src" "$dst"
    chmod 0755 "$dst" 2>/dev/null || true
}

# platforms/linux/dist-tooling/ → repo root is three levels up.
SCRIPT_DIR="$(cd "$(dirname "$(readlink -f "$0")")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"

# ─────────────────────────────────────────────────────────────────────
# .env reader — the ONLY platform switch. See installguide_windows.md §2.
# ─────────────────────────────────────────────────────────────────────
# Prints the value or nothing. Always succeeds: DIST_ROOT / TARGET_TRIPLE /
# PROFILE are optional overrides, and under `set -e` a grep miss in a command
# substitution would otherwise abort the script instead of falling back.
dotenv() {
    local key="$1"
    [ -f "${ROOT}/.env" ] || { echo "error: no .env at ${ROOT}/.env" >&2; exit 2; }
    sed -E 's/[[:space:]]*#.*$//' "${ROOT}/.env" \
        | grep -E "^[[:space:]]*${key}=" | head -n1 | cut -d= -f2- | tr -d "\"' " || true
}

OS="$(dotenv OS)"
[ -n "$OS" ] || { echo "error: OS not set in ${ROOT}/.env" >&2; exit 2; }
[ "$OS" = linux ] || {
    echo "error: build.sh is the linux staging script, but .env has OS=${OS}." >&2
    echo "       Set OS=\"linux\" or use build.ps1 (installguide_windows.md §8)." >&2
    exit 2; }

# ─────────────────────────────────────────────────────────────────────
# 0. Resolve everything from .env + this platform's descriptor
# ─────────────────────────────────────────────────────────────────────
DIST_ROOT="$(dotenv DIST_ROOT)"; DIST_ROOT="${DIST_ROOT:-platforms}"
TOOLING="${ROOT}/${DIST_ROOT}/${OS}/dist-tooling"
DESC="${ROOT}/${DIST_ROOT}/${OS}/version.json"
[ -f "$DESC" ] || { echo "error: missing platform descriptor ${DESC}" >&2; exit 2; }

VERSION=$(jq -r .version      "$DESC")
ARCH=$(jq    -r .arch         "$DESC")
BIN=$(jq     -r .binary       "$DESC")
PKG=$(jq     -r .package_stem "$DESC")
PRODUCT=$(jq -r .product      "$DESC")

# Oldest glibc the shipped binary must load against. 2.17 is the manylinux2014
# baseline and covers everything still in the field (Ubuntu 20.04's 2.31,
# Debian 11's 2.31, JetPack 5). Not an optional knob: without it the floor is
# whatever the build machine happens to run, silently.
GLIBC_FLOOR=$(jq -r '.glibc_floor // empty' "$DESC")
[ -n "$GLIBC_FLOOR" ] || {
    echo "error: ${DESC} has no glibc_floor — a Linux release cannot be staged without one" >&2
    exit 2; }

# The architectures to cut, as "<arch> <triple>" lines. The descriptor's own
# `arch`/`triple` is the default one — the only arch `package_stem` mirrors,
# which is what keeps /sb-release's seven-way mirror at seven. Every entry in
# `extra_arches` is the same source built for another machine; their stems are
# derived below rather than stored, so adding an architecture never adds a
# mirror site.
#
# TARGET_TRIPLE in .env still overrides the DEFAULT arch's triple only. It is an
# escape hatch for retargeting one build, not a way to redefine the arch list.
DEFAULT_TRIPLE="$(dotenv TARGET_TRIPLE)"
DEFAULT_TRIPLE="${DEFAULT_TRIPLE:-$(jq -r .triple "$DESC")}"
TARGETS="$(printf '%s %s\n' "$(jq -r .arch "$DESC")" "$DEFAULT_TRIPLE"
           jq -r '(.extra_arches // [])[] | "\(.arch) \(.triple)"' "$DESC")"

if [ -n "$ONLY_ARCH" ]; then
    SELECTED="$(printf '%s\n' "$TARGETS" | awk -v a="$ONLY_ARCH" '$1 == a')"
    [ -n "$SELECTED" ] || {
        echo "error: --arch ${ONLY_ARCH} is not in ${DESC}. Known:" >&2
        printf '%s\n' "$TARGETS" | awk '{print "         " $1}' >&2
        exit 2; }
    TARGETS="$SELECTED"
fi

PROFILE="$(dotenv PROFILE)";     PROFILE="${PROFILE:-release}"
# cargo puts the `dev` profile under target/<triple>/debug, not .../dev.
# Written as an if rather than `[ ] && x=y`, whose non-zero status on the
# non-dev path would trip `set -e`.
if [ "$PROFILE" = dev ]; then PROFILE_DIR=debug; else PROFILE_DIR="$PROFILE"; fi

# Drift check. The descriptor MIRRORS the version; Cargo.toml owns it.
# A bundle whose zip name disagrees with the binary inside it is worse than
# no bundle, so this is fatal rather than a warning. See /sb-release.
CARGO_V=$(cargo metadata --format-version 1 --no-deps --manifest-path "${ROOT}/Cargo.toml" \
          | jq -r '.packages[]|select(.name=="sb-cli").version')
[ "$VERSION" = "$CARGO_V" ] || {
    echo "error: version drift: ${DESC} says ${VERSION}, Cargo.toml says ${CARGO_V} — run /sb-release" >&2
    exit 2; }
WANT_STEM="${PRODUCT}-${VERSION}-${ARCH}"
[ "$PKG" = "$WANT_STEM" ] || {
    echo "error: package_stem drift: descriptor says ${PKG}, expected ${WANT_STEM} — run /sb-release" >&2
    exit 2; }

DIST="${ROOT}/${DIST_ROOT}/${OS}/dist/${VERSION}"

echo "swarmbotix ${VERSION}  (${PROFILE}, glibc ≥ ${GLIBC_FLOOR})"
printf '%s\n' "$TARGETS" | awk 'NF {print "  " $1 "  " $2}'

# ─────────────────────────────────────────────────────────────────────
# Steps 1–9 run once per architecture. Only the payload's binary differs;
# messages, documents, the config template and the two scripts are the same
# bytes in every arch's zip, because they are the same source.
# ─────────────────────────────────────────────────────────────────────
stage_arch() {
    local ARCH="$1" TRIPLE="$2"
    local PKG="${PRODUCT}-${VERSION}-${ARCH}"
    local STAGE="${DIST}/${PKG}"
    local BIN_SRC BIN_V BIN_GLIBC

    echo
    echo "── ${ARCH}  (${TRIPLE})"

# ─────────────────────────────────────────────────────────────────────
# 1. Build — always with an explicit --target, so the artifact lands in
#    the per-triple dir and a bare `cargo build --release` can never be
#    mistaken for a staged one. installguide_windows.md §5.
# ─────────────────────────────────────────────────────────────────────
#    `cargo zigbuild` rather than `cargo build`: the ".${GLIBC_FLOOR}" suffix on
#    the target is zig's, and it is what pins the symbol versions. Output still
#    lands in target/<triple>/<profile>/, so everything downstream is unchanged.
if [ "$SKIP_BUILD" -eq 0 ]; then
    cargo zigbuild --profile "$PROFILE" --target "${TRIPLE}.${GLIBC_FLOOR}" --bin sb \
        --manifest-path "${ROOT}/Cargo.toml"
fi
BIN_SRC="${ROOT}/target/${TRIPLE}/${PROFILE_DIR}/${BIN}"
[ -f "$BIN_SRC" ] || {
    echo "error: missing ${BIN_SRC} — drop --skip-build, or check the profile/triple" >&2
    exit 1; }

# The binary is the only site that carries the version at runtime; prove the
# staged artifact agrees before wrapping a version-named zip around it.
#
# A cross-built binary cannot be run here, so this check is skipped for it
# unless the host can emulate that architecture (binfmt/qemu, which CI does not
# have). Skipping is stated out loud rather than silently: it is the check that
# catches a stale target/ under --skip-build, and on a cross target that guard
# is simply absent.
if "$BIN_SRC" --version >/dev/null 2>&1; then
    BIN_V="$("$BIN_SRC" --version | awk '{print $NF}')"
    [ "$BIN_V" = "$VERSION" ] || {
        echo "error: ${BIN_SRC} reports ${BIN_V}, descriptor says ${VERSION} — stale target/, rebuild" >&2
        exit 2; }
elif [ "$(uname -m)" = x86_64 ] && [ "${TRIPLE#x86_64}" = "$TRIPLE" ]; then
    echo "note: version check skipped — this host cannot execute ${ARCH} binaries"
else
    echo "error: ${BIN_SRC} is for this machine but will not run — the artifact is broken" >&2
    exit 2
fi

# glibc floor, asserted on the artifact rather than trusted from the build flag.
# A too-high floor is invisible on the build machine and fatal on the target
# ("version `GLIBC_2.39' not found" before main runs), so it is checked here
# and not left to whoever installs the zip. --skip-build is exactly the path
# that can hand us a natively-linked binary, which is why this is outside the
# `if` above.
#
# .gnu.version_r lists every glibc version the binary demands; the highest one
# IS the floor. `sort -V` orders 2.9 < 2.17 correctly, which a lexical sort
# does not.
BIN_GLIBC="$(readelf -V "$BIN_SRC" \
    | grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' | sed 's/^GLIBC_//' | sort -uV | tail -n1)"
[ -n "$BIN_GLIBC" ] || {
    echo "error: no GLIBC version requirements found in ${BIN_SRC} — cannot verify the floor" >&2
    exit 2; }
if [ "$(printf '%s\n%s\n' "$BIN_GLIBC" "$GLIBC_FLOOR" | sort -V | tail -n1)" != "$GLIBC_FLOOR" ]; then
    echo "error: ${BIN_SRC} requires glibc ${BIN_GLIBC}, floor is ${GLIBC_FLOOR}." >&2
    echo "       It will not load on an older target. Rebuild without --skip-build," >&2
    echo "       or raise glibc_floor in ${DESC} deliberately." >&2
    exit 2
fi
echo "glibc floor OK (binary needs ≤ ${BIN_GLIBC}, target ${GLIBC_FLOOR})"

# ─────────────────────────────────────────────────────────────────────
# 2. Clean ONLY this arch's artifacts in this version slot — never the
#    sibling platform, and never the sibling architecture. `--arch` exists
#    so CI can cut one arch per job; wiping the whole slot would make the
#    second job delete the first job's zip.
# ─────────────────────────────────────────────────────────────────────
rm -rf "$STAGE" "${DIST}/${PKG}.zip" "${DIST}/${PKG}.zip.sha256"
mkdir -p "${STAGE}/bin" "${STAGE}/messages" "${STAGE}/documents"

# ─────────────────────────────────────────────────────────────────────
# 3. Binary
# ─────────────────────────────────────────────────────────────────────
place_exec "$BIN_SRC" "${STAGE}/bin/${BIN}"

# ─────────────────────────────────────────────────────────────────────
# 4. Message styles — the whole messages/ tree, one subdir per style, each
#    with its own message_definitions/. .cache/ is the generated protobuf
#    descriptor set and targets/ | message_targets/ are generated bindings;
#    all three are regenerated by `sb message compile` and never staged.
# ─────────────────────────────────────────────────────────────────────
rsync -a \
  --exclude '.cache' \
  --exclude 'targets' \
  --exclude 'message_targets' \
  "${ROOT}/messages/" "${STAGE}/messages/"

# Every shipped style must carry a message_definitions/ or the installer has
# nothing to place. Catches a style added to the repo without one.
for s in "${STAGE}"/messages/*/; do
    [ -d "$s" ] || continue
    [ -d "${s}message_definitions" ] || {
        echo "error: style '$(basename "$s")' staged without a message_definitions/ directory" >&2
        exit 1; }
done

# ─────────────────────────────────────────────────────────────────────
# 5. Reference docs
# ─────────────────────────────────────────────────────────────────────
cp -r "${ROOT}/documents/." "${STAGE}/documents/"

# ─────────────────────────────────────────────────────────────────────
# 6. Config template (verbatim from this platform's dist-tooling/)
# ─────────────────────────────────────────────────────────────────────
cp "${TOOLING}/sb.config.yml.template" "${STAGE}/sb.config.yml.template"

# ─────────────────────────────────────────────────────────────────────
# 7. Installer + uninstaller, verbatim from this platform's dist-tooling/.
#    They ship INSIDE the package: the user unzips one file and runs the
#    install.sh that comes out of it, and $SB_HOME gets that same
#    uninstall.sh placed in it. place_exec keeps the 0755 bit, which zip
#    records and unzip restores.
# ─────────────────────────────────────────────────────────────────────
place_exec "${TOOLING}/install.sh"   "${STAGE}/install.sh"
place_exec "${TOOLING}/uninstall.sh" "${STAGE}/uninstall.sh"

# ─────────────────────────────────────────────────────────────────────
# 8. VERSION stamp (generated)
# ─────────────────────────────────────────────────────────────────────
cat > "${STAGE}/VERSION" <<EOF
version:    ${VERSION}
built:      $(date +%F)
platform:   ${ARCH}
binary:     sb ${VERSION}
EOF

if [ "$NO_ZIP" -eq 1 ]; then
    echo "staged (not zipped): ${STAGE}"
    return 0
fi

# ─────────────────────────────────────────────────────────────────────
# 9. Zip + checksum, drop the staging dir
# ─────────────────────────────────────────────────────────────────────
# Zip $PKG (not $PKG/*) so the package dir is the archive's single top-level
# entry — install.sh asserts on that layout, and it matches what
# Compress-Archive -Path $Stage produces on Windows.
( cd "$DIST" && zip -rq "${PKG}.zip" "$PKG" && rm -rf "$PKG" )
# `sha256sum` format ("<hash>  <name>"), so the recipient can verify with
# `sha256sum -c <pkg>.zip.sha256`. build.ps1 writes a bare hash because
# Get-FileHash has no -c counterpart to satisfy.
( cd "$DIST" && sha256sum "${PKG}.zip" > "${PKG}.zip.sha256" )
}

mkdir -p "$DIST"
while read -r target_arch target_triple; do
    [ -n "$target_arch" ] || continue
    stage_arch "$target_arch" "$target_triple"
done <<TARGETS
${TARGETS}
TARGETS

if [ "$NO_ZIP" -eq 1 ]; then
    echo
    echo "staged (not zipped) → ${DIST}"
    exit 0
fi

echo
echo "staged → ${DIST}"
ls -lh "$DIST"
