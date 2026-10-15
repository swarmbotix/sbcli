#!/usr/bin/env bash
# get-sb.sh: install the swarmbotix `sb` CLI on Linux with one command.
#
# Version-free bootstrap. Every Linux release carries this same file beside
# its zips, so `releases/latest/download/get-sb.sh` always resolves. It does
# what `sb update` does, minus an existing `sb`: find the release, download
# swarmbotix-<version>-<arch>.zip and its .sha256, verify, unzip into a temp
# dir, and run the install.sh that comes out of the package. The temp dir is
# removed on exit. It never calls sudo.
#
# The whole body is inside main(), so a download cut short by the network
# cannot run half a script: bash parses all of main before running any of it.

set -euo pipefail

DEFAULT_BASE="https://github.com/swarmbotix/sbcli"
PRODUCT="swarmbotix"

# Globals, not locals of main: the EXIT trap runs after main has returned,
# and fetch_asset reads LOCAL_DIR (set for a file:// SB_RELEASE_BASE).
GET_SB_TMP=""
LOCAL_DIR=""

usage() {
    cat <<'EOF'
get-sb.sh: install the swarmbotix sb CLI on Linux (x86_64, aarch64).

Usage:
  curl -fsSL https://github.com/swarmbotix/sbcli/releases/latest/download/get-sb.sh | bash -s -- --yes
  bash get-sb.sh [--yes] [--version X.Y.Z]

Options:
  -y, --yes            passed to install.sh: add ~/.swarmbotix/bin to PATH in
                       ~/.bashrc without asking
  --version X.Y.Z      install this release instead of the latest
  -h, --help           show this text

Environment:
  SB_HOME              install prefix, passed through to install.sh
                       (default ~/.swarmbotix). With curl | bash, set it on
                       bash, not on curl: curl ... | SB_HOME=/x bash -s -- --yes
  SB_RELEASE_BASE      repository URL to use instead of
                       https://github.com/swarmbotix/sbcli, or file:///<dir>
                       for a folder holding latest.txt and the release assets
                       side by side (offline testing). Same meaning as for
                       `sb update`.
EOF
}

die() {
    echo "error: $*" >&2
    exit 1
}

cleanup() {
    if [[ -n "$GET_SB_TMP" && -d "$GET_SB_TMP" ]]; then
        rm -rf "$GET_SB_TMP"
    fi
}

# Strip leading and trailing whitespace.
trim() {
    local s="$1"
    s="${s#"${s%%[![:space:]]*}"}"
    s="${s%"${s##*[![:space:]]}"}"
    printf '%s' "$s"
}

# The folder a file:// URL names (file:///tmp/rel -> /tmp/rel), taken
# literally, like dir_from_file_url in crates/sb-update/src/fetch.rs.
dir_from_file_url() {
    local rest="${1#file://}"
    rest="${rest#localhost}"
    printf '%s' "$rest"
}

# The release tag at the end of a .../releases/tag/v0.2.3 redirect, or
# nothing. Mirrors tag_from_location in crates/sb-update/src/fetch.rs.
tag_from_location() {
    local loc="$1"
    loc="${loc%%[?#]*}"
    while [[ "$loc" == */ ]]; do loc="${loc%/}"; done
    local tag="${loc##*/}"
    if [[ "$tag" == v?* ]]; then
        printf '%s' "$tag"
    fi
}

# Newest release tag of the repository at $1, read from the Location header
# of the redirect GitHub answers <repo>/releases/latest with. No API token,
# no rate limit, and no redirect following: the redirect IS the answer.
latest_tag_remote() {
    local base="$1"
    local url="${base}/releases/latest"
    local headers status loc tag
    if ! headers="$(curl -sSI --connect-timeout 20 --max-time 60 "$url" 2>&1)"; then
        die "cannot reach ${url}: ${headers}"
    fi
    headers="$(printf '%s\n' "$headers" | tr -d '\r')"
    status="$(printf '%s\n' "$headers" | awk 'toupper($1) ~ /^HTTP\// { s = $2 } END { print s }')"
    loc="$(printf '%s\n' "$headers" | awk 'tolower($1) == "location:" { l = $2 } END { print l }')"
    [[ -n "$loc" ]] \
        || die "GET ${url}: expected a redirect to the latest release, got HTTP ${status:-?}"
    tag="$(tag_from_location "$loc")"
    [[ -n "$tag" ]] \
        || die "GET ${url}: redirected to ${loc}, which does not name a release tag"
    printf '%s' "$tag"
}

# Copy (file:// base) or download (any other base) one release asset.
fetch_asset() {
    local src="$1" dest="$2"
    if [[ -n "$LOCAL_DIR" ]]; then
        [[ -f "$src" ]] || die "no such release asset: ${src}"
        cp "$src" "$dest"
        return 0
    fi
    # A progress bar on a terminal, quiet in CI logs; errors are shown either way.
    local progress=(-sS)
    if [[ -t 2 ]]; then progress=(-#); fi
    curl -fL "${progress[@]}" --retry 3 --connect-timeout 30 -o "$dest" "$src" \
        || die "download failed: ${src}"
}

main() {
    local yes=0 version=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --yes|-y)    yes=1 ;;
            --version)   shift
                         [[ $# -gt 0 && -n "$1" ]] || die "--version needs a value, e.g. --version 0.9.0"
                         version="$1" ;;
            --version=*) version="${1#--version=}"
                         [[ -n "$version" ]] || die "--version needs a value, e.g. --version 0.9.0" ;;
            --help|-h)   usage; exit 0 ;;
            *)           echo "error: unknown argument: $1 (see --help)" >&2; exit 2 ;;
        esac
        shift
    done

    # Linux packages only. A Mac reports arm64 too, and would otherwise be
    # handed a Linux binary that fails long after the download.
    [[ "$(uname -s)" == Linux ]] \
        || die "get-sb.sh installs the Linux build; this machine is $(uname -s). On Windows, unzip the windows-x86_64 release and run install.ps1."

    # Same mapping as install.sh, which re-checks it against the package.
    local arch
    case "$(uname -m)" in
        x86_64|amd64)  arch=linux-x86_64 ;;
        aarch64|arm64) arch=linux-aarch64 ;;
        *) die "no swarmbotix build for this machine ($(uname -m)); supported: x86_64, aarch64" ;;
    esac

    local tool pkg
    for tool in curl unzip sha256sum; do
        if ! command -v "$tool" >/dev/null 2>&1; then
            case "$tool" in sha256sum) pkg=coreutils ;; *) pkg="$tool" ;; esac
            die "${tool} is required but not on PATH (Ubuntu: sudo apt install ${pkg})"
        fi
    done

    # SB_RELEASE_BASE, as `sb update` reads it: unset or empty is the public
    # repository, file:///<dir> is a local folder, anything else is another
    # repository URL.
    local base
    base="$(trim "${SB_RELEASE_BASE:-}")"
    if [[ -z "$base" ]]; then
        base="$DEFAULT_BASE"
    elif [[ "$base" == file://* ]]; then
        LOCAL_DIR="$(dir_from_file_url "$base")"
        [[ -n "$LOCAL_DIR" ]] || die "SB_RELEASE_BASE=${base} names no folder"
        [[ -d "$LOCAL_DIR" ]] || die "SB_RELEASE_BASE=${base}: ${LOCAL_DIR} is not a folder"
    fi
    while [[ "$base" == */ ]]; do base="${base%/}"; done

    if [[ -z "$version" ]]; then
        if [[ -n "$LOCAL_DIR" ]]; then
            [[ -f "$LOCAL_DIR/latest.txt" ]] || die "no ${LOCAL_DIR}/latest.txt (SB_RELEASE_BASE=${base})"
            version="$(trim "$(cat "$LOCAL_DIR/latest.txt")")"
            [[ -n "$version" ]] || die "${LOCAL_DIR}/latest.txt names no version"
        else
            echo "Looking up the latest release at ${base} ..."
            version="$(latest_tag_remote "$base")"
        fi
    fi
    version="$(trim "$version")"
    version="${version#v}"
    [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.+-]+)?$ ]] \
        || die "\`${version}\` is not a version; expected X.Y.Z, e.g. 0.9.0"

    local stem="${PRODUCT}-${version}-${arch}"
    local asset="${stem}.zip"
    local src_zip src_sum
    if [[ -n "$LOCAL_DIR" ]]; then
        src_zip="${LOCAL_DIR%/}/${asset}"
    else
        src_zip="${base}/releases/download/v${version}/${asset}"
    fi
    src_sum="${src_zip}.sha256"

    GET_SB_TMP="$(mktemp -d "${TMPDIR:-/tmp}/get-sb.XXXXXXXX")"
    local tmp="$GET_SB_TMP"

    echo "swarmbotix ${version} for ${arch}"
    echo "  from ${src_zip}"
    fetch_asset "$src_sum" "$tmp/${asset}.sha256"
    fetch_asset "$src_zip" "$tmp/${asset}"

    # The sidecar is sha256sum format, "<hex>  <name>". Only its digest is
    # taken, and checked against the file this script downloaded, so the name
    # written inside the sidecar cannot point the check at some other file.
    local want
    want="$(tr -d '\r' < "$tmp/${asset}.sha256" | awk 'NF && !seen { print $1; seen = 1 }')"
    [[ "$want" =~ ^[0-9A-Fa-f]{64}$ ]] \
        || die "${asset}.sha256 holds no sha256 digest (from ${src_sum})"
    ( cd "$tmp" && printf '%s  %s\n' "${want,,}" "$asset" | sha256sum -c --quiet - ) \
        || die "sha256 mismatch for ${asset}: the download does not match ${asset}.sha256"
    echo "  sha256 OK"

    # -o: the folder is fresh, but unzip must never stop to ask on stdin,
    # which under curl | bash is the script itself.
    unzip -q -o "$tmp/${asset}" -d "$tmp/unpacked" </dev/null \
        || die "cannot unpack ${asset}"
    local pkgdir="$tmp/unpacked/${stem}"
    [[ -f "$pkgdir/install.sh" ]] \
        || die "${asset} has no ${stem}/install.sh; found: $(ls -A "$tmp/unpacked" | tr '\n' ' ')"

    # `bash install.sh`, not `./install.sh`: a noexec /tmp must not stop it.
    # stdin is /dev/null so install.sh never reads this piped script; without
    # --yes it still asks its PATH question on /dev/tty when there is one.
    # SB_HOME, when set, is inherited as is.
    echo
    if [[ "$yes" -eq 1 ]]; then
        bash "$pkgdir/install.sh" --yes </dev/null
    else
        bash "$pkgdir/install.sh" </dev/null
    fi

    print_next_steps "$arch"
}

# protoc and flatc are separate, pinned downloads that `sb tools install`
# fetches into /opt. sb never runs sudo, so name the folders it will need
# first: the parents from `sb tools install` that do not exist yet or are
# not writable (flatc has no aarch64 build, so only protoc's there).
print_next_steps() {
    local arch="$1"
    local sb_home="${SB_HOME:-$HOME/.swarmbotix}"
    local parents=(/opt/protobuf)
    if [[ "$arch" == linux-x86_64 ]]; then parents+=(/opt/flatbuffers); fi

    local need=() p
    for p in "${parents[@]}"; do
        if [[ ! -d "$p" || ! -w "$p" ]]; then need+=("$p"); fi
    done

    echo
    echo "Next steps:"
    case ":${PATH}:" in
        *":${sb_home}/bin:"*) ;;
        *)
            echo "  # sb is not on this shell's PATH yet; open a new terminal, or:"
            # shellcheck disable=SC2016  # print $PATH literally
            printf '  export PATH="%s/bin:$PATH"\n' "$sb_home"
            ;;
    esac
    if [[ ${#need[@]} -gt 0 ]]; then
        # shellcheck disable=SC2016  # print $USER literally
        printf '  sudo mkdir -p %s && sudo chown "$USER" %s\n' "${need[*]}" "${need[*]}"
    fi
    echo "  sb tools install --yes"
    echo "  sb message compile"
    echo "  sb doctor"
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

main "$@"
