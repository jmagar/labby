# Scaffold And Audit

`labby scaffold service` and `labby audit onboarding` are a deferred guardrail
contract, not commands in the current CLI surface.

If these commands are restored, scaffold should create the expected module
skeleton, then audit should verify that the service is wired into every
declared surface and registry.

```bash
labby scaffold service <service>
labby audit onboarding <service>
```

## Scaffold Contract

The scaffolded shape must follow the repo's module and layer rules:

- `lab-apis` owns upstream clients, serde types, and service errors.
- `crates/lab/src/dispatch/<service>/` owns action catalog, params, client
  resolution, and shared execution.
- CLI, MCP, and HTTP adapters stay thin and call dispatch.
- No `mod.rs` files are introduced.

## Audit Contract

The onboarding audit should catch missing or drifted wiring across:

- Cargo features and `lab-apis` passthrough features
- `PluginMeta` and environment metadata
- dispatch action catalog and schema
- CLI registration
- MCP/API registration
- generated docs and service coverage docs

Until the commands exist again, use manual review plus generated-doc checks:

```bash
cargo run -p labby --all-features -- docs generate
cargo run -p labby --all-features -- docs check
cargo check --workspace --all-features
```

A service is not online until its declared surfaces, generated docs, and the
normal all-features build path pass.
