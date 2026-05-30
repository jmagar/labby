# Adguard MCP Reference

This skill uses the Lab MCP integration for `adguard`. The authoritative action catalog is live and should be discovered at call time.

## Discovery

Use the service MCP tool first:

```json
{ "action": "help" }
{ "action": "schema", "params": { "action": "<name>" } }
```

The service skill's `## Highlights` section lists the common actions, but do not treat this file as a frozen schema. Prefer live `help` and per-action `schema` output before invoking uncommon or mutating actions.

## Safety

If the skill body says an action is destructive or mutating, confirm with the user before invoking it.
