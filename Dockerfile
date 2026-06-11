FROM rust:1-slim AS builder

WORKDIR /app

# Cache the dependency build separately from source changes.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && \
    echo 'fn main() {}' > src/main.rs && \
    echo '' > src/lib.rs && \
    cargo build --release && \
    rm -rf src

# Templates and static assets are embedded at compile time.
COPY src ./src
COPY webui/templates ./webui/templates
COPY webui/static ./webui/static
RUN touch src/main.rs src/lib.rs && cargo build --release

FROM debian:bookworm-slim

COPY --from=builder /app/target/release/advanced-memory-mcp /usr/local/bin/advanced-memory-mcp

ENV MCP_DATA_DIR=/data
ENV MCP_HOST=0.0.0.0
ENV MCP_PORT=8765
ENV MCP_TOKEN_CONFIG=/config/tokens.json
ENV MCP_TYPE_CONFIG=/config/entity_types.json
ENV MCP_TRANSPORT=http

VOLUME /data
VOLUME /config
EXPOSE 8765

HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD ["advanced-memory-mcp", "--health-check"]

ENTRYPOINT ["advanced-memory-mcp"]
