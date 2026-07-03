#!/usr/bin/env bash
set -euo pipefail

IMAGE="registry.confusticate.com/advanced-memory-mcp"
TAG="${1:-latest}"
PLATFORM="${PLATFORM:-linux/amd64}"

# Refuse to build unless the working tree is clean and fully pushed to origin,
# so a pushed image always corresponds to a commit reachable on the remote.
if [[ -n "$(git status --porcelain)" ]]; then
    echo "error: working tree has uncommitted changes; commit or stash them first." >&2
    git status --short >&2
    exit 1
fi

upstream="$(git rev-parse --abbrev-ref --symbolic-full-name '@{upstream}' 2>/dev/null || true)"
if [[ -z "${upstream}" ]]; then
    echo "error: current branch has no upstream; push it to origin first." >&2
    exit 1
fi

git fetch --quiet
if [[ -n "$(git rev-list "${upstream}..HEAD")" ]]; then
    echo "error: HEAD has commits not pushed to ${upstream}; push them first." >&2
    exit 1
fi

docker build --platform "${PLATFORM}" --no-cache -t "${IMAGE}:${TAG}" .
docker push "${IMAGE}:${TAG}"
