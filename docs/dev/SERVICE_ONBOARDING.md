# Service Onboarding

This is the end-to-end checklist for bringing a service online in `lab`.

The current flow is manual wiring plus generated-doc verification. Older docs
referenced `labby scaffold service` and `labby audit onboarding`; those commands
are not part of the current CLI surface unless they are restored in code.

```bash
cargo check --workspace --all-features
cargo run -p labby --all-features -- docs generate
cargo run -p labby --all-features -- docs check
```

## Required Steps

1. Start from the upstream API spec or notes in `docs/upstream-api/`.
2. Add pure client logic and serde types under `crates/lab-apis/src/<service>/`.
3. Add the shared dispatch module under `crates/lab/src/dispatch/<service>/`.
4. Keep CLI, MCP, and HTTP adapters thin; they must call dispatch instead of
   reimplementing service behavior.
5. Register the service in metadata, registry construction, and only the
   CLI/MCP/API/web surfaces it actually exposes.
6. Add a Cargo feature only when the service is an intended standalone product
   slice or true `lab-apis` passthrough. Do not add one feature per internal
   module by default.
7. Add or update `docs/coverage/<service>.md` when the service has a coverage
   contract.
8. Regenerate docs and run the all-features build/test path before handoff.

## Source Documents

- [DISPATCH.md](./DISPATCH.md) owns the shared dispatch-layer contract.
- [ERRORS.md](./ERRORS.md) owns stable error envelopes and status mapping.
- [OBSERVABILITY.md](./OBSERVABILITY.md) owns logging, correlation, and redaction.
- [SCAFFOLD_AND_AUDIT.md](./SCAFFOLD_AND_AUDIT.md) records the deferred
  scaffold/audit contract if those commands are restored.
