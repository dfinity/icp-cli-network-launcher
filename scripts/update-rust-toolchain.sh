#!/usr/bin/env bash
# Pins the Rust toolchain used by this repo to a target stable version, keeping
# rust-toolchain.toml and the Docker base images (Dockerfile,
# cloudengine.Dockerfile) in sync so the containerized release build never drifts
# behind the toolchain.
#
# Usage: ./update-rust-toolchain.sh [version]
#
#   With no argument, the latest stable release from static.rust-lang.org is
#   used — but only if it is at least MIN_AGE_DAYS old (default 14) and differs
#   from the currently pinned version. This matches the weekly CI cadence: a
#   fresh release is left to soak before adoption. Otherwise the script makes no
#   changes and exits 0.
#
#   With an explicit version (e.g. 1.97.0) the freshness gate is skipped and all
#   files are pinned to that version. Useful for local runs and for forcing a
#   bump or re-syncing the Docker images to the current toolchain.

set -euo pipefail

MIN_AGE_DAYS="${MIN_AGE_DAYS:-14}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TOOLCHAIN_TOML="$REPO_ROOT/rust-toolchain.toml"
DOCKERFILES=("$REPO_ROOT/Dockerfile" "$REPO_ROOT/cloudengine.Dockerfile")

# Portable YYYY-MM-DD -> epoch seconds (GNU date on Linux/CI, BSD date on macOS).
date_to_epoch() {
    local d="$1"
    if date --version >/dev/null 2>&1; then
        date -d "$d" +%s          # GNU coreutils
    else
        date -j -f "%Y-%m-%d" "$d" +%s   # BSD / macOS
    fi
}

current=$(sed -n 's/^channel = "\(.*\)"/\1/p' "$TOOLCHAIN_TOML")
echo "Currently pinned Rust: $current"

# ── Resolve the target version ────────────────────────────────────────────────

if [[ $# -ge 1 ]]; then
    target="$1"
    echo "Using provided version: $target (freshness gate skipped)"
else
    echo "Fetching latest stable Rust from static.rust-lang.org..."
    manifest=$(curl -sf https://static.rust-lang.org/dist/channel-rust-stable.toml)

    # The [pkg.rust] section holds a line like: version = "1.96.0 (<hash> <date>)".
    # awk prints at END (never exits early) so the whole manifest is consumed —
    # closing the pipe mid-stream would raise SIGPIPE under `set -o pipefail`.
    latest=$(printf '%s' "$manifest" | awk '
        /^\[pkg\.rust\]/ { in_rust = 1; next }
        /^\[/            { in_rust = 0 }
        in_rust && /^version = / && v == "" {
            split($0, a, "\""); split(a[2], b, " "); v = b[1]
        }
        END { print v }')
    release_date=$(printf '%s' "$manifest" | awk -F'"' '
        /^date = / && d == "" { d = $2 }
        END { print d }')

    if [[ -z "$latest" || -z "$release_date" ]]; then
        echo "error: could not parse latest version/date from the release manifest" >&2
        exit 1
    fi

    days=$(( ( $(date +%s) - $(date_to_epoch "$release_date") ) / 86400 ))
    echo "Latest stable: $latest (released $release_date, $days days ago)"

    if [[ "$latest" == "$current" ]]; then
        echo "Already on the latest stable ($current). Nothing to do."
        exit 0
    fi
    if [[ "$days" -lt "$MIN_AGE_DAYS" ]]; then
        echo "Latest stable is only $days days old (< $MIN_AGE_DAYS). Leaving it to soak."
        exit 0
    fi

    target="$latest"
fi

# ── Patch the files ───────────────────────────────────────────────────────────
#
# perl -i is used instead of `sed -i` because the latter is not portable between
# GNU (Linux/CI) and BSD (macOS) sed.

echo ""
echo "Pinning Rust $target in:"

perl -i -pe "s/^channel = \"[^\"]*\"/channel = \"$target\"/" "$TOOLCHAIN_TOML"
echo "  rust-toolchain.toml"

for df in "${DOCKERFILES[@]}"; do
    perl -i -pe "s/^FROM rust:[0-9.]+-slim-trixie/FROM rust:$target-slim-trixie/" "$df"
    echo "  $(basename "$df")"
done

echo ""
echo "Done. Rust toolchain and Docker base images pinned to $target."
