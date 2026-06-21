# Plugin Coverage

The plugins checked into this repo with their registered components. Each lives at `plugins/<name>/` and declares itself via `.claude-plugin/plugin.json`.

Only `labby` (coupled to the `labby` binary) and `scripts` (shared tooling) are bundled here. The other Lab/Labby plugins moved to the dedicated marketplace repo, [dendrite](https://github.com/jmagar/dendrite); see that repo for their coverage.

**Categories:** agents · bin · commands · hooks · monitors · output-styles · scripts · skills · themes · .mcp.json · .lsp.json · settings.json

---

## labby

| Type | Detail |
|------|--------|
| manifest | `.claude-plugin/plugin.json` |
| hook | `hooks/hooks.json` |
| skill | `skills/creating-snippets/SKILL.md` |
| skill | `skills/using-labby/SKILL.md` |
| .mcp.json | `lab` -> `${user_config.server_url}/mcp` |
| README.md | ✓ |
| CHANGELOG.md | ✓ |

---

## scripts

No registered components found.
