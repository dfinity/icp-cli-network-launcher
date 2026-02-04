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
maketarball=0
if [[ -z "$1" ]]; then
    maketarball=1
fi
tar=tar
if [[ "$os" = "darwin" && "$maketarball" = 1 ]]; then
    command -v gtar >/dev/null 2>&1 || die "please install gtar (brew install gnu-tar)"
    tar=gtar
fi

v=$(cargo metadata --format-version=1 --no-deps | jq -r '.packages[] | select(.name=="icp-cli-network-launcher") | .version')
source=$(cargo metadata --format-version=1 --no-deps | jq -r '.packages[] | select(.name=="icp-cli-network-launcher") | .dependencies[] | select(.name=="pocket-ic") | .source')
reg='([0-9.]+)(-((r[0-9]+$)|([0-9-]+(\.(r[0-9]+)))))?'
if [[ "$v" =~ $reg ]]; then
    pkgver=${BASH_REMATCH[1]}
    icdate=${BASH_REMATCH[5]:-}
    patchrel=${BASH_REMATCH[4]:-${BASH_REMATCH[7]:-}}
else
    die "could not parse package version $v - should be 1.2.3[-r1] or 1.2.3-2026-01-29-16-08[.r1]"
fi
if [[ "$v" = *"-"* ]]; then
    [[ "$source" = "git+"* ]] || die "package.version is patch but pocket-ic dependency is not git"
else
    [[ "$source" != "git+"* ]] || die "package.version is not patch but pocket-ic dependency is git"
fi
name="icp-cli-network-launcher-${arch}-${os}-v${v}"
outdir="${1-"dist/${name}"}"

cargo build --release
mkdir -p "${outdir}"
cp "target/release/icp-cli-network-launcher" "${outdir}/"
if [[ -z "$icdate" ]]; then
    icver=$(sed 's/-/_/3' <<<"${icdate}")
    curl --proto '=https' -sSfL --tlsv1.2 "https://github.com/dfinity/pocketic/releases/download/${pkgver}/pocket-ic-${arch}-${os}.gz" -o "${outdir}/pocket-ic.gz" ${GITHUB_TOKEN:+ -H "Authorization: Bearer ${GITHUB_TOKEN}" }
else
    curl --proto '=https' -sSfL --tlsv1.2 "https://github.com/dfinity/ic/releases/download/release-${icver}-base/pocket-ic-${arch}-${os}.gz" -o "${outdir}/pocket-ic.gz" ${GITHUB_TOKEN:+ -H "Authorization: Bearer ${GITHUB_TOKEN}" }
fi
gunzip -f "${outdir}/pocket-ic.gz"
chmod a+x "${outdir}/pocket-ic"

if [[ "$maketarball" = 1 ]]; then
    "$tar" -C dist -czf "dist/${name}.tar.gz" "${name}"
fi
