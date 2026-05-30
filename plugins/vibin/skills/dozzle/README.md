# Dozzle

Direct Dozzle API workflow for the real-time Docker container log viewer. Uses `DOZZLE_URL` and optional `DOZZLE_SESSION_COOKIE`; covers Dozzle auth/MCP guidance; does not route through Lab MCP or `lab dozzle`.

## Usage

Invoke this skill when the user request matches the trigger conditions in `SKILL.md`. The skill body is the source of truth for workflow steps and operational constraints.

## Files

- `SKILL.md` - agent workflow and trigger guidance
- `agents/` - OpenAI runtime metadata
- `references/` - progressively loaded reference material
- `README.md` - packaging overview
- `CHANGELOG.md` - packaging change history
