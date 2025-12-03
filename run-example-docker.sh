#!/usr/bin/env bash
set -e
[[ $# -eq 1 ]] || { echo "Usage: '$0 <status-dir>'" >&2; exit 1; }
d=$(mktemp -d)
container=$(head -c 16 /dev/urandom | xxd -p)
docker build -t pocket-ic-launcher-example -f ./example.Dockerfile .
rewrite_json() {
    until [[ -e "$d/status.json" ]]; do
        sleep 0.2
    done
    config_port=$(docker port "$container" 4942/tcp | cut -d: -f2)
    gateway_port=$(docker port "$container" 4943/tcp | cut -d: -f2)
    jq '.config_port = $config | .gateway_port = $gateway' \
        --argjson config "$config_port" --argjson gateway "$gateway_port" "$d/status.json" > "$1/status.json"
}
rewrite_json "$@" &
trap 'kill %rewrite_json' EXIT
docker run -v "$d:/app/status" -P --name "$container" pocket-ic-launcher-example
