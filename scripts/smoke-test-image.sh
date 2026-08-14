#!/usr/bin/env bash
# Runs a launcher Docker image the way icp-cli does — bind-mounted status dir,
# SIGINT to shut it down — and checks that startup and shutdown both come out
# clean. Container mode has failure modes no test on the host can reach: the
# launcher runs as PID 1, its status dir is a mount point, and the bundled
# pocket-ic binary is only ever started for real here.
#
# Usage (from anywhere): ./scripts/smoke-test-image.sh [image]
#
#   The image defaults to $IMAGE, which is how CI passes it. Build one first:
#
#     docker build -t launcher:smoke .
#     ./scripts/smoke-test-image.sh launcher:smoke
#
# Exits nonzero on the first failed check, after printing the container's log.

set -euo pipefail

image="${1:-${IMAGE:-}}"
if [[ -z "$image" ]]; then
    echo "Usage: $0 [image]  (or set IMAGE)" >&2
    exit 2
fi

# How long pocket-ic gets to boot a full topology, on a machine that may be
# loaded. Startup is normally seconds.
STARTUP_TIMEOUT_SECS=120
# Well past the launcher's own shutdown grace period, so a slow-but-working
# shutdown cannot be SIGKILLed into a false failure (exit code 137).
STOP_TIMEOUT_SECS=60

# ::error:: turns the message into a GitHub Actions annotation, and is harmless
# noise anywhere else.
die() {
    if [[ -n "${GITHUB_ACTIONS:-}" ]]; then
        echo "::error::$1" >&2
    else
        echo "Error: $1" >&2
    fi
    exit 1
}

# A `-v` bind mount is exactly how icp-cli passes the status dir, and what makes
# the container's `--status-dir` a mount point that can never be removed.
status_dir="$(mktemp -d)"
container="launcher-smoke-$$"
cleanup() {
    docker rm -f "$container" >/dev/null 2>&1 || true
    rm -rf "$status_dir"
}
trap cleanup EXIT

echo "Smoke testing $image with a bind-mounted status dir at $status_dir"
docker run -d --name "$container" -v "$status_dir:/app/status" "$image" >/dev/null

# status.json is written once the network is ready, so its appearance is the
# startup assertion.
for _ in $(seq 1 "$((STARTUP_TIMEOUT_SECS / 2))"); do
    [[ -f "$status_dir/status.json" ]] && break
    sleep 2
done
if [[ ! -f "$status_dir/status.json" ]]; then
    docker logs "$container" || true
    die "the launcher never wrote status.json (waited ${STARTUP_TIMEOUT_SECS}s)"
fi
echo "Network came up; shutting it down"

# STOPSIGNAL is SIGINT, which is the shutdown path icp-cli uses.
docker stop -t "$STOP_TIMEOUT_SECS" "$container" >/dev/null
logs="$(docker logs "$container" 2>&1)"
printf '%s\n' "$logs"

code="$(docker inspect -f '{{.State.ExitCode}}' "$container")"
[[ "$code" == 0 ]] || die "the launcher exited with $code on SIGINT"
if printf '%s\n' "$logs" | grep -Eq '^(Error|Warning)'; then
    die "the launcher reported a problem on shutdown"
fi
# The mount point itself survives and must: emptying it is what tells icp-cli
# the network stopped.
leftovers="$(ls -A "$status_dir")"
[[ -z "$leftovers" ]] || die "the status dir was not emptied, leaving: $leftovers"

echo "Smoke test passed: clean startup, clean shutdown, status dir emptied"
