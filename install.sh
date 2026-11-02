#!/usr/bin/env bash
# install.sh -- build webcam-tune, install it, and run it as a LaunchAgent.
#
#   ./install.sh                     build, install to $PREFIX/bin, load the LaunchAgent
#   ./install.sh --uninstall         unload the LaunchAgent, remove it and the binary
#   ./install.sh --check-platform    the platform gate alone (exit 0 = supported)
#   ./install.sh --render-plist BIN LOG DIR   print the LaunchAgent plist (the test seam)
#   curl -fsSL https://raw.githubusercontent.com/ztomer/webcam_tune/main/install.sh | bash
#
# Run outside a checkout (the curl line), it clones the repo into
# ${XDG_DATA_HOME:-~/.local/share}/webcam_tune/src at its newest release tag (WEBCAM_TUNE_REPO
# overrides the URL) and runs that checkout's install.sh.
#
# Apple silicon macOS only, and the gate is a hard failure: nothing here has ever run on an Intel
# Mac or on Linux, so nothing is built there. WEBCAM_TUNE_OS / WEBCAM_TUNE_ARCH override uname so
# both directions of the gate are testable on one machine. LAUNCHAGENTS_DIR redirects the plist
# and skips launchctl (tests); PREFIX moves the binary (default ~/.local). Profiles go to
# ${XDG_CONFIG_HOME:-~/.config}/webcam_tune/profiles, each copied from the repo's profiles/ only
# when absent: a profile you have edited is never overwritten.
#
# Everything is inside functions and `main` runs on the last line, so a download cut short by
# `curl | bash` runs nothing at all.
set -euo pipefail

label="io.github.ztomer.webcam-tune"
repo_url="${WEBCAM_TUNE_REPO:-https://github.com/ztomer/webcam_tune.git}"
prefix="${PREFIX:-$HOME/.local}"
agents="${LAUNCHAGENTS_DIR:-$HOME/Library/LaunchAgents}"
log="$HOME/Library/Logs/webcam-tune.log"
profiles="${XDG_CONFIG_HOME:-$HOME/.config}/webcam_tune/profiles"

# The house TUI when this machine has it; the same icons in plain text otherwise.
if [ -f "${GOH_DIR:-$HOME/Projects/gates_of_heck}/tui/lib.sh" ]; then
    # shellcheck source=/dev/null
    . "${GOH_DIR:-$HOME/Projects/gates_of_heck}/tui/lib.sh"
else
    info() { printf '→ %s\n' "$1"; }
    ok() { printf '✓ %s\n' "$1"; }
    err() { printf '✗ %s\n' "$1" >&2; }
    die() { err "$1"; exit "${2:-1}"; }
fi

check_platform() {
    local os="${WEBCAM_TUNE_OS:-$(uname -s)}" arch="${WEBCAM_TUNE_ARCH:-$(uname -m)}"
    case "$os/$arch" in
        Darwin/arm64) return 0 ;;
        Darwin/*) die "unsupported: macOS on $arch. Only Apple silicon Macs are supported." ;;
        *) die "unsupported: $os/$arch. webcam-tune drives macOS's USB and power APIs; it builds on Apple silicon macOS only." ;;
    esac
}

# The checkout this script sits in, or nothing when it arrived on stdin.
checkout() {
    local src="${BASH_SOURCE[0]:-}" dir
    [ -n "$src" ] && [ -f "$src" ] || return 0
    dir="$(cd "$(dirname "$src")" && pwd)"
    grep -qs '^name = "webcam-tune"' "$dir/Cargo.toml" && printf '%s\n' "$dir"
    return 0
}

# Clone (or refresh) the repo at its newest release tag and run ITS installer.
bootstrap() {
    check_platform
    command -v git >/dev/null || die "git not found: install the Xcode command line tools (xcode-select --install)"
    local src="${XDG_DATA_HOME:-$HOME/.local/share}/webcam_tune/src" tags tag
    if [ -d "$src/.git" ]; then
        info "updating $src"
        git -C "$src" fetch --quiet --tags --force origin
    else
        info "cloning $repo_url into $src"
        mkdir -p "$(dirname "$src")"
        git clone --quiet "$repo_url" "$src"
    fi
    tags="$(git -C "$src" tag --list 'v*' --sort=-version:refname)"
    tag="${tags%%$'\n'*}"
    if [ -n "$tag" ]; then
        git -C "$src" checkout --quiet "$tag"
        info "using release $tag"
    else
        git -C "$src" checkout --quiet "origin/HEAD"
        info "no release tag yet; using the default branch"
    fi
    exec bash "$src/install.sh" "$@"
}

render_plist() {
    local bin="$1" logfile="$2" dir="$3"
    cat <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$label</string>
    <key>ProgramArguments</key>
    <array>
        <string>$bin</string>
        <string>watch</string>
        <string>--profiles-dir</string>
        <string>$dir</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ProcessType</key>
    <string>Background</string>
    <key>StandardOutPath</key>
    <string>$logfile</string>
    <key>StandardErrorPath</key>
    <string>$logfile</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>NO_COLOR</key>
        <string>1</string>
    </dict>
</dict>
</plist>
EOF
}

remove_all() {
    [ -n "${LAUNCHAGENTS_DIR:-}" ] || launchctl bootout "gui/$(id -u)/$label" 2>/dev/null || true
    rm -f "$agents/$label.plist" "$prefix/bin/webcam-tune"
    ok "removed the LaunchAgent and $prefix/bin/webcam-tune (your profiles in $profiles stay)"
}

seed_profiles() {
    local here="$1" shipped name
    mkdir -p "$profiles"
    for shipped in "$here"/profiles/*.toml; do
        name="$(basename "$shipped")"
        if [ -f "$profiles/$name" ]; then
            info "keeping your profile $profiles/$name"
        else
            cp "$shipped" "$profiles/$name"
            ok "added profile $profiles/$name"
        fi
    done
}

build_and_install() {
    local here="$1" built
    check_platform
    command -v cargo >/dev/null || die "cargo not found: install Rust from https://rustup.rs"
    info "building (release)"
    cargo build --release --locked --manifest-path "$here/Cargo.toml"
    built="$(cargo metadata --format-version 1 --no-deps --manifest-path "$here/Cargo.toml" \
        | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')/release/webcam-tune"
    mkdir -p "$prefix/bin" "$agents" "$(dirname "$log")"
    command install -m 0755 "$built" "$prefix/bin/webcam-tune"
    "$prefix/bin/webcam-tune" --version >/dev/null || die "the installed binary does not run"
    ok "installed $prefix/bin/webcam-tune"
    seed_profiles "$here"
    render_plist "$prefix/bin/webcam-tune" "$log" "$profiles" > "$agents/$label.plist"
    plutil -lint -s "$agents/$label.plist" || die "rendered an invalid plist"
    if [ -n "${LAUNCHAGENTS_DIR:-}" ]; then
        ok "wrote $agents/$label.plist (LAUNCHAGENTS_DIR is set, so it was not loaded)"
        return 0
    fi
    launchctl bootout "gui/$(id -u)/$label" 2>/dev/null || true
    launchctl bootstrap "gui/$(id -u)" "$agents/$label.plist"
    ok "LaunchAgent $label loaded; it logs to $log"
}

main() {
    local here
    here="$(checkout)"
    case "${1:-}" in
        "") if [ -n "$here" ]; then build_and_install "$here"; else bootstrap "$@"; fi ;;
        --uninstall) remove_all ;;
        --check-platform) check_platform && ok "supported platform" ;;
        --render-plist) render_plist "${2:?BIN}" "${3:?LOG}" "${4:?DIR}" ;;
        -h|--help) sed -n '2,23p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' ;;
        *) die "unknown option $1 (./install.sh --help)" 2 ;;
    esac
}

main "$@"
