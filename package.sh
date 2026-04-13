#!/usr/bin/env bash
set -e
cd "$(dirname "$0")"
die() {
    echo "$1" >&2
    exit 1
}
command -v jq >/dev/null 2>&1 || die "please install jq"

case $(uname -s) in
    Linux*)     os="linux";;
    Darwin*)    os="darwin";;
    *)          echo "Unsupported OS $(uname -s)"; exit 1;;
esac
case $(uname -m) in
    x86_64*)    arch="x86_64";;
    arm64*)     arch="arm64";;
    aarch64*)   arch="arm64";;
    *)          echo "Unsupported architecture $(uname -m)"; exit 1;;
esac

dir=
cargo_args=()
while (( $# )); do
    case "$1" in
        --) shift; cargo_args=("$@"); break;;
        *) [[ -z "$dir" ]] && dir="$1" || die "too many arguments";;
    esac
    shift
done

maketarball=0
if [[ -z "$dir" ]]; then
    maketarball=1
fi
tar=tar
if [[ "$os" = "darwin" && "$maketarball" = 1 ]]; then
    command -v gtar >/dev/null 2>&1 || die "please install gtar (brew install gnu-tar)"
    tar=gtar
fi

v=$(cargo metadata --format-version=1 --no-deps | jq -r '.packages[] | select(.name=="icp-cli-network-launcher") | .version')
source=$(cargo metadata --format-version=1 --no-deps | jq -r '.packages[] | select(.name=="icp-cli-network-launcher") | .dependencies[] | select(.name=="pocket-ic") | .source')
# Parse version: X.Y.Z with optional suffix
if [[ "$v" =~ ^([0-9]+\.[0-9]+\.[0-9]+)(-.+)?$ ]]; then
    pkgver=${BASH_REMATCH[1]}
    suffix=${BASH_REMATCH[2]:-}
    suffix=${suffix#-}
else
    die "could not parse package version $v"
fi
githash=""
icdate=""
patchrel=""
if [[ -z "$suffix" ]]; then
    :
elif [[ "$suffix" =~ ^(r[0-9]+)$ ]]; then
    patchrel=${BASH_REMATCH[1]}
elif [[ "$suffix" =~ ^([0-9a-f]{40})(-(r[0-9]+))?$ ]]; then
    githash=${BASH_REMATCH[1]}
    patchrel=${BASH_REMATCH[3]:-}
elif [[ "$suffix" =~ ^([0-9]{4}-[0-9]{2}-[0-9]{2}-[0-9]{2}-[0-9]{2})(\.(r[0-9]+))?$ ]]; then
    icdate=${BASH_REMATCH[1]}
    patchrel=${BASH_REMATCH[3]:-}
else
    die "could not parse package version $v - expected 1.2.3[-r1], 1.2.3-2026-01-29-16-08[.r1], or 1.2.3-<git-hash>[-r1]"
fi
if [[ -n "$icdate" || -n "$githash" || -n "$patchrel" ]]; then
    [[ "$source" = "git+"* ]] || die "package.version is patch but pocket-ic dependency is not git"
else
    [[ "$source" != "git+"* ]] || die "package.version is not patch but pocket-ic dependency is git"
fi
name="icp-cli-network-launcher-${arch}-${os}-v${v}"
outdir="${dir:-"dist/${name}"}"
cargo build --release "${cargo_args[@]}"
mkdir -p "${outdir}"
cp "$(cargo metadata --no-deps --format-version=1 | jq -r .target_directory)/release/icp-cli-network-launcher" "${outdir}/"
if [[ -n "$githash" ]]; then
    echo "Fetching pocket-ic from: https://download.dfinity.systems/ic/${githash}/binaries/${arch}-${os}/pocket-ic.gz"
    curl --proto '=https' -sSfL --tlsv1.2 "https://download.dfinity.systems/ic/${githash}/binaries/${arch}-${os}/pocket-ic.gz" -o "${outdir}/pocket-ic.gz"
elif [[ -n "$icdate" ]]; then
    icver=$(sed 's/-/_/3' <<<"${icdate}")
    echo "Fetching pocket-ic from: https://github.com/dfinity/ic/releases/download/release-${icver}-base/pocket-ic-${arch}-${os}.gz"
    curl --proto '=https' -sSfL --tlsv1.2 "https://github.com/dfinity/ic/releases/download/release-${icver}-base/pocket-ic-${arch}-${os}.gz" -o "${outdir}/pocket-ic.gz" ${GITHUB_TOKEN:+ -H "Authorization: Bearer ${GITHUB_TOKEN}" }
else
    echo "Fetching pocket-ic from: https://github.com/dfinity/pocketic/releases/download/${pkgver}/pocket-ic-${arch}-${os}.gz"
    curl --proto '=https' -sSfL --tlsv1.2 "https://github.com/dfinity/pocketic/releases/download/${pkgver}/pocket-ic-${arch}-${os}.gz" -o "${outdir}/pocket-ic.gz" ${GITHUB_TOKEN:+ -H "Authorization: Bearer ${GITHUB_TOKEN}" }
fi
gunzip -f "${outdir}/pocket-ic.gz"
chmod a+x "${outdir}/pocket-ic"

if [[ "$maketarball" = 1 ]]; then
    "$tar" -C dist -czf "dist/${name}.tar.gz" "${name}"
fi
