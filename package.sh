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
if [[ "$v" = *"+"* ]]; then
    [[ "$source" = "git+"* ]] || die "package.version is patch but pocket-ic dependency is not git"
    revstr=${source#"git+https://github.com/dfinity/ic?"}
    [[ "$revstr" =~ 'rev='[0-9a-f]{40} ]] || die "use the full hash in the pocket-ic version"
    sha=${revstr#"rev="}
else
    [[ "$source" != "git+"* ]] || die "package.version is not patch but pocket-ic dependency is git"
fi
name="icp-cli-network-launcher-${arch}-${os}-v${v}"
outdir="${1-"dist/${name}"}"

cargo build --release
mkdir -p "${outdir}"
cp "target/release/icp-cli-network-launcher" "${outdir}/"
if [[ -z "$sha" ]]; then
    curl --proto '=https' -sSfL --tlsv1.2 "https://github.com/dfinity/pocketic/releases/download/${v}/pocket-ic-${arch}-${os}.gz" -o "${outdir}/pocket-ic.gz" ${GITHUB_TOKEN:+ -H "Authorization: Bearer ${GITHUB_TOKEN}" }
else
    curl --proto '=https' -sSfL --tlsv1.2 "https://download.dfinity.systems/ic/${sha}/binaries/${arch}-${os}/pocket-ic.gz" -o "${outdir}/pocket-ic.gz"
fi
gunzip -f "${outdir}/pocket-ic.gz"
chmod a+x "${outdir}/pocket-ic"

if [[ "$maketarball" = 1 ]]; then
    "$tar" -C dist -czf "dist/${name}.tar.gz" "${name}"
fi
