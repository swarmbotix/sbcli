#!/usr/bin/env bash
# swarmbotix sb CLI uninstaller.
#
# Removes swarmbotix from this machine, in this order:
#   1. the PATH line install.sh appended to ~/.bashrc
#   2. $SB_HOME itself (default: ~/.swarmbotix) — binary, config, message
#      vault and reference docs
#
# It ships inside swarmbotix-<version>-<arch>.zip beside install.sh, and
# install.sh also places a copy at $SB_HOME/uninstall.sh. Either copy works;
# they are the same file.
#
# Usage:
#   ./uninstall.sh               interactive (confirms before deleting $SB_HOME)
#   ./uninstall.sh --yes         non-interactive
#   ./uninstall.sh --keep-home   remove the PATH line and the binary only,
#                                keeping config + edited messages
#   SB_HOME=/opt/sb ./uninstall.sh   uninstall a custom prefix

set -euo pipefail

YES=0
KEEP_HOME=0
for arg in "$@"; do
    case "$arg" in
        --yes|-y)    YES=1 ;;
        --keep-home) KEEP_HOME=1 ;;
        # --purge was the old spelling of "also delete $SB_HOME". That is the
        # default now, so accept it and do nothing rather than fail on a
        # command someone has in their notes.
        --purge)     ;;
        --help|-h)
            sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'
            exit 0 ;;
        *) echo "unknown argument: $arg" >&2; exit 2 ;;
    esac
done

SB_HOME="${SB_HOME:-$HOME/.swarmbotix}"
# readlink -m canonicalizes a path whether or not it exists, which matters
# because everything below is guarded on this value.
SB_HOME_ABS="$(readlink -m "$SB_HOME")"

# Never let a stray SB_HOME turn this into a much bigger delete.
HOME_ABS="$(readlink -m "$HOME")"
if [[ "$SB_HOME_ABS" == "/" || "$SB_HOME_ABS" == "$HOME_ABS" ]]; then
    echo "error: SB_HOME is ${SB_HOME_ABS} — refusing to delete it." >&2
    exit 1
fi

# The installed copy of this script lives inside the tree it is about to
# delete. bash reads a script incrementally and seeks back into the file
# between commands, so removing it mid-run can abort the uninstall halfway.
# Re-exec from a temp copy first; the child cleans that copy up on exit.
SELF="$(readlink -f "$0")"
if [[ "${SB_UNINSTALL_RELAUNCHED:-0}" -eq 1 ]]; then
    trap 'rm -f "$SELF"' EXIT
elif [[ "$KEEP_HOME" -eq 0 ]]; then
    case "$SELF/" in
        "$SB_HOME_ABS"/*)
            relaunch="$(mktemp)"
            cp "$SELF" "$relaunch"
            chmod 0755 "$relaunch"
            SB_UNINSTALL_RELAUNCHED=1 SB_HOME="$SB_HOME" exec bash "$relaunch" "$@"
            ;;
    esac
fi

echo "Uninstalling swarmbotix from ${SB_HOME_ABS}"

# ── 1. PATH ──────────────────────────────────────────────────────────
# install.sh tags its line with the marker comment precisely so this can find
# it without guessing at the surrounding file.
RC="$HOME/.bashrc"
MARKER='# added by swarmbotix installer'
if [[ -f "$RC" ]] && grep -q "$MARKER" "$RC"; then
    sed -i '/# added by swarmbotix installer$/d' "$RC"
    echo "  removed the PATH line from ~/.bashrc"
else
    echo "  ~/.bashrc has no swarmbotix PATH line"
fi

# ── 2. $SB_HOME ──────────────────────────────────────────────────────
if [[ "$KEEP_HOME" -eq 1 ]]; then
    rm -f "$SB_HOME_ABS/bin/sb"
    echo "  removed $SB_HOME_ABS/bin/sb"
    echo "  kept $SB_HOME_ABS (config, messages, documents)"
elif [[ ! -d "$SB_HOME_ABS" ]]; then
    echo "  $SB_HOME_ABS does not exist — nothing to remove"
else
    do_delete=0
    if [[ "$YES" -eq 1 ]]; then
        do_delete=1
    elif [[ -t 0 ]]; then
        read -r -p "Delete ${SB_HOME_ABS} (config, messages, documents)? [Y/n] " ans
        case "$ans" in ""|y|Y|yes|YES) do_delete=1 ;; esac
    # Attempt the open rather than testing `-r /dev/tty`: access(2) says yes
    # even with no controlling terminal, and the ENXIO from the redirect would
    # end the script under `set -e`. See the same note in install.sh.
    elif { exec 3</dev/tty; } 2>/dev/null; then
        read -r -p "Delete ${SB_HOME_ABS} (config, messages, documents)? [Y/n] " ans <&3 || ans=n
        exec 3<&-
        case "$ans" in ""|y|Y|yes|YES) do_delete=1 ;; esac
    else
        # No way to ask. Deleting a config directory is not something to do on
        # an assumption, so stop and name the flag that says yes on purpose.
        echo "  no terminal to confirm on — re-run with --yes to delete ${SB_HOME_ABS}" >&2
        exit 1
    fi
    if [[ "$do_delete" -eq 1 ]]; then
        rm -rf "$SB_HOME_ABS"
        echo "  removed $SB_HOME_ABS"
    else
        echo "  kept $SB_HOME_ABS"
    fi
fi

echo
echo "Done. 'sb' stays resolvable in shells that are already open —"
echo "run 'hash -r' or start a new terminal."
