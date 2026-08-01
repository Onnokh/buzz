# Local proof image for provider-backed agents. It deliberately uses the same
# Sprig multicall binary as the Desktop harness, so the Docker provider tests
# deployment ownership without introducing a second agent implementation.
FROM rust:1.95-bookworm AS build

WORKDIR /build
COPY . .
RUN cargo build --release --locked -p sprig

FROM node:22-bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 1001 buzz \
    && useradd --system --uid 1001 --gid 1001 --home-dir /var/lib/buzz \
        --create-home --shell /usr/sbin/nologin buzz

# Subscription-backed runtimes. The ACP adapters include the runtimes they
# drive; the standalone CLIs are also installed so operators can complete a
# one-time OAuth login with `docker exec` and persist it in the home volume.
RUN npm install --global \
      @agentclientprotocol/codex-acp \
      @agentclientprotocol/claude-agent-acp \
      @openai/codex \
      @anthropic-ai/claude-code \
    && npm cache clean --force

COPY --from=build /build/target/release/sprig /usr/local/bin/sprig
RUN for personality in buzz-acp buzz-agent buzz-dev-mcp buzz git-credential-nostr git-sign-nostr; do \
      ln -s /usr/local/bin/sprig "/usr/local/bin/${personality}"; \
    done

ENV HOME=/var/lib/buzz
USER buzz:buzz
WORKDIR /var/lib/buzz
ENTRYPOINT ["/usr/local/bin/buzz-acp"]
