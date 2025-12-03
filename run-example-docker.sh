#!/usr/bin/env bash
set -e
[[ $# -eq 1 ]] || { echo "Usage: '$0 <status-dir>'" >&2; exit 1; }
d=$(mktemp -d)
docker build -t pocket-ic-launcher-example -f ./example.Dockerfile .
container=$(docker run -d -v "$d:/app/status" -P pocket-ic-launcher-example)
trap "docker stop $container >/dev/null" EXIT

until [[ -e "$d/status.json" || "$(docker container inspect -f '{{.State.Running}}' $container)" != "true" ]]; do
    sleep 0.5
done
if [[ ! -e "$d/status.json" ]]; then
    echo "Error: container exited before creating status.json" >&2
    docker logs $container >&2
    exit 1
fi
config_port=$(docker port "$container" 4942/tcp | cut -d: -f2)
gateway_port=$(docker port "$container" 4943/tcp | cut -d: -f2)
jq '.config_port = $config | .gateway_port = $gateway' --arg config "$config_port" --arg gateway "$gateway_port" "$d/status.json" > "$1/status.json"
trap - EXIT
docker attach $container
