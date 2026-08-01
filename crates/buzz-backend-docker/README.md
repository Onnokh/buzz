# buzz-backend-docker

Local Docker backend provider for Buzz managed agents.

Build the proof image and provider:

    docker build -f docker/buzz-acp/Dockerfile -t buzz-acp:local .
    cargo build --release -p buzz-backend-docker

Install the provider where Desktop discovers user providers:

    mkdir -p ~/.local/bin
    ln -sf "$PWD/target/release/buzz-backend-docker" ~/.local/bin/buzz-backend-docker

Restart Desktop. The agent create dialog will show docker under Run on.
Choose the existing harness/runtime, LLM provider, and model as usual, then
choose Docker. The provider creates a detached container named from the agent
pubkey and returns its Docker container ID as the backend agent ID.

The default image is buzz-acp:local. It includes the Codex and Claude ACP
adapters plus their CLIs. Select `codex-acp` or `claude-agent-acp` as the
agent harness to use a subscription-backed runtime; `buzz-agent` is the
API-key-based runtime.

Each deployed agent gets a stable Docker home volume by default. Complete the
subscription login once from the server after the container is created:

    docker exec -it buzz-agent-<pubkey-prefix> codex login --device-auth

For Claude, run the Claude CLI interactively in the same container and complete
the Claude subscription login. The credentials remain in the named home volume
and are available after restarts. No OpenAI or Anthropic API key is required.

The relay URL must be reachable from the container. For the deployed relay in
this setup use `wss://buzz.onkie.dev`; when the relay runs on the Docker host,
use a URL such as `ws://host.docker.internal:3000`.
