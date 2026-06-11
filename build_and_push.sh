#!/usr/bin/env bash
set -euo pipefail

IMAGE="registry.confusticate.com/advanced-memory-mcp"
TAG="${1:-latest}"
PLATFORM="${PLATFORM:-linux/amd64}"

docker build --platform "${PLATFORM}" --no-cache -t "${IMAGE}:${TAG}" .
docker push "${IMAGE}:${TAG}"
