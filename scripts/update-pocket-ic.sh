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

# Update [package] version (first occurrence of ^version = "...")
perl -i -pe "s/^version = \"[^\"]*\"/version = \"$new_version\"/" "$CARGO_TOML"

# Update pocket-ic rev = "..." (single-line dependency entry)
perl -i -pe "s/(pocket-ic = \{[^}]*rev = \")[^\"]*(\")/\${1}$commit_sha\${2}/" "$CARGO_TOML"

echo ""
echo "Cargo.toml updated:"
echo "  version     = \"$new_version\""
echo "  pocket-ic   rev = \"$commit_sha\""

# ── 7. Verify the crate still compiles ───────────────────────────────────────

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
