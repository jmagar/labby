# Generated Docs

Files in this directory are generated from code-owned metadata.

Regenerate all artifacts with:

```bash
just docs-generate
```

Verify tracked artifacts are fresh with:

```bash
just docs-check
```

`docs-check` compares the declared generated artifact manifest and enforces generated-doc invariants such as feature-matrix consistency and safety linting. It does not run Markdown link checks, live service health, or onboarding audit policy gates.

| Artifact | Source |
| --- | --- |
| `service-catalog.md/json` | `labby docs generate` |
| `action-catalog.md/json` | `labby docs generate` |
| `env-reference.md/json` | `labby docs generate` |
| `api-routes.md/json` | `labby docs generate` |
| `openapi.json` | `labby docs generate` |
| `feature-matrix.md/json` | `labby docs generate` |
| `mcp-help.md/json` | `labby docs generate` |
| `cli-help.md` | `labby docs generate` |
