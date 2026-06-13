# Service Layer Migration

The original service-layer migration plan targeted first-party upstream
integrations that are no longer present in this checkout's Cargo feature table.
Treat older references to services such as Radarr, ByteStash, and UniFi as
historical context, not current implementation guidance.

Current shared-dispatch guidance lives in:

- [DISPATCH.md](./DISPATCH.md)
- [SERVICE_ONBOARDING.md](./SERVICE_ONBOARDING.md)
- [SERVICES.md](./SERVICES.md)

The current product shape is:

- shared execution belongs under `crates/lab/src/dispatch/`
- CLI, MCP, HTTP, and web adapters stay thin over dispatch
- standalone product slices are `gateway`, `marketplace`, `fs`, `deploy`, and
  `acp_registry`
- base control-plane services such as `doctor`, `setup`, `logs`, `device`,
  `stash`, and `acp` compile without individual feature flags

If first-party upstream integrations are reintroduced, start from
`SERVICE_ONBOARDING.md`, update Cargo features intentionally, regenerate docs,
and prove both the narrow feature slice and the all-features build.
