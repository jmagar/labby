# Adguard

Lab's Adguard integration — DNS-level ad blocking and network filtering. Use when the user wants to manage their Adguard instance, or invokes `lab adguard` / `mcp__lab__adguard`. Calls the MCP tool first, falls back to the CLI if MCP is unavailable.

## Usage

Invoke this skill when the user request matches the trigger conditions in `SKILL.md`. The skill body is the source of truth for workflow steps and operational constraints.

## Files

- `SKILL.md` - agent workflow and trigger guidance
- `agents/` - OpenAI runtime metadata
- `references/` - progressively loaded reference material
- `README.md` - packaging overview
- `CHANGELOG.md` - packaging change history
