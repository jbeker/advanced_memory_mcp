FROM python:3.13-slim

COPY --from=ghcr.io/astral-sh/uv:latest /uv /uvx /bin/

WORKDIR /app

COPY pyproject.toml uv.lock ./
RUN uv sync --frozen --no-dev --no-install-project

COPY server.py knowledge_graph.py ./
RUN uv sync --frozen --no-dev

ENV MCP_DATA_DIR=/data
ENV MCP_HOST=0.0.0.0
ENV MCP_PORT=8765
ENV MCP_MODE=read-only
ENV MCP_TRANSPORT=sse

VOLUME /data
EXPOSE 8765

ENTRYPOINT ["uv", "run", "advanced-memory-mcp"]
