# testing

App-testing and MCP-tooling skills in one plugin.

## Bundled skills

| Skill | Purpose |
|---|---|
| `web-app-testing` | Live end-to-end QA of a web app in a real browser (Playwright over CDP) |
| `android-app-testing` | Live QA of an Android APK on an emulator/device via adb |
| `desktop-app-testing` | Live QA of a Windows desktop app in the agent-os VM |
| `mcpjam-ui-testing` | Validate MCP-UI / MCP Apps implementations with MCPJam |
| `mcporter` | Discover, inspect, and smoke-test MCP servers from the shell |
| `claude-in-mobile` | Drive Android/iOS/desktop devices via claude-in-mobile |

## Configuration

None at the plugin level — each skill drives external tooling (Playwright, adb, the agent-os Windows-MCP, mcporter, MCPJam) and documents its own prerequisites.
