#!/usr/bin/env bash
set -euo pipefail

IMAGE="registry.confusticate.com/advanced-memory-mcp"
TAG="${1:-latest}"

docker build --no-cache -t "${IMAGE}:${TAG}" .
docker push "${IMAGE}:${TAG}"
