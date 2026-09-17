#!/usr/bin/env bash
# The only screenshot authority: a pristine, immutable Linux browser filesystem.
# Tests and real rustX processes stay on the host; only Chromium runs here.
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ $(uname -s) != Linux || $(uname -m) != x86_64 ]]; then
  echo 'Browser references require a Linux x86_64 host with Docker or Podman.' >&2
  exit 1
fi
engine=${CONTAINER_ENGINE:-docker}
image=mcr.microsoft.com/playwright:v1.63.0-noble@sha256:bc6ab0d6d44ff4826e4cb8c1e6d801e185bfc42bb0753f8e2a30efc70db054c7
if [[ $(node -p "require('@playwright/test/package.json').version") != 1.63.0 ]]; then
  echo 'Update the pinned browser image together with Playwright and its references.' >&2
  exit 1
fi

# No host font directories or browser caches enter the container. The installed,
# lockfile-controlled Playwright JS is shared read-only, with no npm install.
# label=disable permits that read-only mount on SELinux hosts without relabeling it.
container=rustx-browser-$$
cleanup() { "$engine" rm -f "$container" >/dev/null 2>&1 || true; }
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
"$engine" run --detach --name "$container" --init --network host --ipc host \
  --platform linux/amd64 --security-opt label=disable \
  --mount "type=bind,source=$PWD/node_modules,target=/opt/rustx-node_modules,readonly" \
  "$image" sh -ec '
    if [ "$(fc-match --format="%{family}" Helvetica)" != "Liberation Sans" ]; then
      echo "Rendering prerequisite failed: Helvetica must resolve to Liberation Sans" >&2
      exit 1
    fi
    exec node /opt/rustx-node_modules/@playwright/test/cli.js run-server --host 127.0.0.1 --port 0
  ' >/dev/null

# Wait only for server readiness, never retry a test or a screenshot comparison.
for attempt in {1..100}; do
  endpoint=$("$engine" logs "$container" 2>&1 | sed -n 's/^Listening on //p')
  if [[ -n $endpoint ]]; then
    echo "Screenshot authority: $image"
    RUSTX_BROWSER_WS_ENDPOINT=$endpoint pnpm exec playwright test "$@"
    exit 0
  fi
  if [[ $("$engine" inspect --format '{{.State.Running}}' "$container") != true ]]; then
    break
  fi
  sleep 0.1
done
"$engine" logs "$container" >&2
echo 'Pinned browser server failed to become ready.' >&2
exit 1
