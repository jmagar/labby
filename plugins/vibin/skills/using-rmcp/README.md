# rmcp Development Guide

This skill covers building, modifying, and debugging MCP (Model Context Protocol) servers and clients in Rust using the rmcp crate. It applies when the codebase imports `rmcp`; when defining tools, resources, or prompts with `#[tool]`, `#[tool_router]`, or `#[prompt_router]`; when choosing or wiring transports (stdio, TCP, Unix socket, HTTP Streamable); when implementing `ServerHandler` or `ClientHandler`; when sending progress notifications; or when a user asks to "add an MCP tool", "create an MCP server", "connect to an MCP server", or "implement a handler".

## Usage

Invoke this skill when the user request matches the trigger conditions in `SKILL.md`. The skill body is the source of truth for workflow steps and operational constraints.

## Files

- `SKILL.md` - agent workflow and trigger guidance
- `agents/` - OpenAI runtime metadata
- `references/` - progressively loaded reference material
- `README.md` - packaging overview
- `CHANGELOG.md` - packaging change history
