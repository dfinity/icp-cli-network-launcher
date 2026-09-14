#!/usr/bin/env bash
# Updates the package version and pocket-ic git revision in Cargo.toml
# based on a dfinity/ic release tag.
#
# Usage: ./update-pocket-ic.sh [release-tag]
#
# If no tag is given, the latest tag matching release-YYYY-MM-DD_HH-MM-base is used.

set -euo pipefail

IC_REPO="dfinity/ic"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CARGO_TOML="$(cd "$SCRIPT_DIR/.." && pwd)/Cargo.toml"

# ── 1. Resolve the release tag ─────────────────────────────────────────────────

if [[ $# -ge 1 ]]; then
    release_tag="$1"
    echo "Using provided release tag: $release_tag"
else
    echo "Fetching tags from $IC_REPO to find the latest release..."
    release_tag=$(
        gh api "repos/$IC_REPO/git/refs/tags" --paginate \
            --jq '.[].ref | ltrimstr("refs/tags/")' \
            | grep -E '^release-[0-9]{4}-[0-9]{2}-[0-9]{2}_[0-9]{2}-[0-9]{2}-base$' \
            | sort \
            | tail -1 \
            || true
    )
    if [[ -z "$release_tag" ]]; then
        echo "error: no tag matching release-YYYY-MM-DD_HH-MM-base found" >&2
        exit 1
    fi
    echo "Latest release tag: $release_tag"
fi

# ── 2. Parse date/time components from the tag ────────────────────────────────

if [[ "$release_tag" =~ ^release-([0-9]{4}-[0-9]{2}-[0-9]{2})_([0-9]{2})-([0-9]{2})-base$ ]]; then
    tag_date="${BASH_REMATCH[1]}"
    tag_hour="${BASH_REMATCH[2]}"
    tag_minute="${BASH_REMATCH[3]}"
else
    echo "error: '$release_tag' does not match expected format release-YYYY-MM-DD_HH-MM-base" >&2
    exit 1
fi

# ── 3. Resolve the commit SHA for the tag ─────────────────────────────────────

echo "Resolving commit SHA for $release_tag..."
ref_info=$(gh api "repos/$IC_REPO/git/ref/tags/$release_tag")
object_type=$(printf '%s' "$ref_info" | jq -r '.object.type')
object_sha=$(printf '%s' "$ref_info" | jq -r '.object.sha')

if [[ "$object_type" == "tag" ]]; then
    # Annotated tag — dereference to the underlying commit
    commit_sha=$(gh api "repos/$IC_REPO/git/tags/$object_sha" | jq -r '.object.sha')
else
    commit_sha="$object_sha"
fi

echo "Commit SHA: $commit_sha"

# ── 4. Read pocket-ic version at that revision ────────────────────────────────

echo "Reading pocket-ic version from $IC_REPO @ $commit_sha..."
pocket_ic_toml=$(curl -sf \
    "https://raw.githubusercontent.com/$IC_REPO/$commit_sha/packages/pocket-ic/Cargo.toml")

pocket_ic_version=$(
    printf '%s' "$pocket_ic_toml" \
        | awk -F'"' '/^version[[:space:]]*=/ { print $2; exit }'
)

if [[ -z "$pocket_ic_version" ]]; then
    echo "error: could not parse version from pocket-ic Cargo.toml" >&2
    exit 1
fi

echo "pocket-ic version: $pocket_ic_version"

# ── 5. Build the new package version string ───────────────────────────────────

new_version="${pocket_ic_version}-${tag_date}-${tag_hour}-${tag_minute}"
echo "New package version: $new_version"

# ── 6. Patch Cargo.toml ───────────────────────────────────────────────────────

echo "Patching $CARGO_TOML..."

# The pocket-ic entry comes in two shapes: the git pin this script writes, and
# the plain crates.io version set by hand when pocket-ic publishes a release. So
# match the whole entry rather than a `rev = "..."` inside it — a pattern that
# only fits the git shape silently leaves a published one untouched while the
# package version below is bumped anyway, and package.sh then refuses to build a
# dated version whose pocket-ic dependency is not a git source.

old_dep=$(perl -ne 'if (/^pocket-ic = (.*?)\s*$/) { print $1; last }' "$CARGO_TOML")
if [[ -z "$old_dep" ]]; then
    echo "error: no single-line 'pocket-ic = ...' entry found in $CARGO_TOML" >&2
    exit 1
fi

new_dep="{ git = \"https://github.com/$IC_REPO\", rev = \"$commit_sha\" }"

# Both substitutions take their replacement from the environment, so a value
# holding quotes or slashes cannot break out of the expression.

# Update [package] version (first occurrence of ^version = "...")
NEW_VERSION="$new_version" perl -i -pe 's|^version = "[^"]*"|version = "$ENV{NEW_VERSION}"|' "$CARGO_TOML"

# Replace the whole pocket-ic entry, whichever shape it had
NEW_DEP="$new_dep" perl -i -pe 's|^pocket-ic = .*|pocket-ic = $ENV{NEW_DEP}|' "$CARGO_TOML"

# A substitution that matched nothing leaves the file valid but wrong, which is
# only caught at release time, so confirm both landed before going on.
if ! grep -qxF "version = \"$new_version\"" "$CARGO_TOML"; then
    echo "error: failed to set the package version in $CARGO_TOML" >&2
    exit 1
fi
if ! grep -qxF "pocket-ic = $new_dep" "$CARGO_TOML"; then
    echo "error: failed to rewrite the pocket-ic entry in $CARGO_TOML" >&2
    exit 1
fi

echo ""
echo "Cargo.toml updated:"
echo "  version     = \"$new_version\""
echo "  pocket-ic   = $new_dep"

# ── 7. Re-resolve Cargo.lock for the new revision ────────────────────────────

# A bare `cargo check` only re-resolves the entries it has to, so a transitive
# dependency that the new pocket-ic requires at a higher version stays pinned at
# its locked one and resolution fails. Unlocking pocket-ic lets cargo re-resolve
# its whole subtree.

if [[ "$old_dep" != "$new_dep" ]]; then
    echo ""
    echo "Updating Cargo.lock for the new pocket-ic revision..."
    if ! cargo update --manifest-path "$CARGO_TOML" --package pocket-ic 2>&1; then
        echo "" >&2
        echo "error: 'cargo update --package pocket-ic' failed for $commit_sha ($release_tag)." >&2
        echo "  See the cargo output above for the cause. If it reports a version conflict," >&2
        echo "  the new pocket-ic needs dependency versions incompatible with this crate's" >&2
        echo "  own, and Cargo.toml has to be reconciled manually." >&2
        exit 1
    fi
fi

# ── 8. Verify the crate still compiles ───────────────────────────────────────

REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
echo ""
echo "Running cargo check..."
if ! cargo check --manifest-path "$REPO_ROOT/Cargo.toml" 2>&1; then
    echo "" >&2
    echo "error: cargo check failed after updating pocket-ic to $commit_sha ($release_tag)." >&2
    echo "  The new pocket-ic version ($pocket_ic_version) likely introduced breaking API changes" >&2
    echo "  that require adjustments to this crate's source code before releasing." >&2
    exit 1
fi
echo ""
echo "Crate compiles successfully."
