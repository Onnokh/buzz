# Research: buzz-node agent execution

Date: 2026-08-02

## Conclusion

`buzz-node` currently implements the relay-native execution protocol and a
durable in-memory/on-disk fake runtime. It does not yet launch a real agent,
provider backend, ACP harness, container, or subprocess. Therefore a successful
deploy receipt from the node currently proves protocol/reconciliation success,
not that an agent process is running or able to answer.

## Evidence

- `crates/buzz-node/src/lib.rs:275-280` explicitly documents
  `FakeWorkloadRuntime` as a minimal fake runtime and says it “does not launch a
  process”; it describes a later provider-backed runtime as the replacement.
- `crates/buzz-node/src/lib.rs:389-450` shows that deploy inserts a
  `WorkloadSpec` with `Running` lifecycle and that start/stop/restart/remove
  only mutate that stored lifecycle.
- `crates/buzz-node/src/lib.rs:867-937` dispatches `Deploy` and lifecycle
  commands directly to that fake runtime. There is no process, container,
  `buzz-acp`, `buzz-agent`, Goose, Codex, or provider invocation in this path.
- `crates/buzz-node/src/lib.rs:288-370` implements provider authentication as
  `FakeCredentialStore`: it returns generic instructions, then encrypts and
  stores the submitted response. It does not perform a provider login or make
  credentials available to a launched runtime.
- `crates/buzz-core/src/execution.rs:285-304` defines the deploy payload as a
  safe projection. It intentionally excludes Docker/container details and raw
  environment secrets, so the node-side runtime still needs a separate mapping
  from this projection to its local execution configuration.
- `desktop/src/features/agents/useAgentManagement.ts` routes an
  execution-node selection through the managed-agent deployment command; the
  Desktop-side protocol wiring is present.
- `desktop/src-tauri/src/commands/execution_nodes.rs:220-240` builds the safe
  workload and sends the encrypted deploy command. The corresponding provider
  authentication commands are at `:279-337`.

## Existing real provider path

The Desktop local/legacy provider implementation is a different path:

- `desktop/src-tauri/src/managed_agents/backend.rs:11-39` launches a provider
  binary and exchanges JSON over stdin/stdout.
- `desktop/src-tauri/src/managed_agents/backend.rs:371-390` sends a real
  provider `deploy` request and requires a provider-assigned `agent_id`.
- `desktop/src-tauri/src/managed_agents/runtime.rs:569-583` launches the ACP
  command and supplies the relay, identity, runtime, and environment settings.

`buzz-node` does not call or share this Desktop provider-launch code. The
missing implementation is therefore the node-side provider/runtime adapter,
including credential handling, process supervision, ACP startup, relay identity
configuration, lifecycle/restart behavior, and readiness/error reporting.

## Separate prerequisite: pairing

Even after a real runtime exists, the node must be paired with the Desktop
owner. `crates/buzz-node/src/main.rs:142-218` subscribes for addressed command
events, while `crates/buzz-node/src/lib.rs:624-647` drops commands whose owner
public key is absent from the node's `OwnerStore`. A node can appear in the
execution-node discovery list without proving that this owner authorization is
present.

## Scope of this research

No product implementation was changed. The only new file is this research note;
the pre-existing README cleanup remains uncommitted separately.
