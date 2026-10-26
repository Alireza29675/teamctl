# Build from the repository root:
# docker build -f containers/Dockerfile -t teamctl-agent:local .
FROM rust:1.90-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --locked --release -p teamctl -p team-mcp

FROM node:22-bookworm-slim
ARG CLAUDE_VERSION=2.1.299
ARG CODEX_VERSION=0.154.0
ARG OPENCODE_VERSION=1.18.3
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates git openssh-client procps tini \
    && rm -rf /var/lib/apt/lists/* \
    && npm install --global \
    @anthropic-ai/claude-code@${CLAUDE_VERSION} \
    @openai/codex@${CODEX_VERSION} \
    opencode-ai@${OPENCODE_VERSION} \
    && npm cache clean --force
COPY --from=build /build/target/release/teamctl /usr/local/bin/teamctl
COPY --from=build /build/target/release/team-mcp /usr/local/bin/team-mcp
WORKDIR /workspace
ENTRYPOINT ["teamctl"]

