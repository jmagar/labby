#!/usr/bin/env python3
"""PostToolUse hook: remind to run sync-stack-llms after adding a package."""
import json, sys, re

try:
    data = json.load(sys.stdin)
except json.JSONDecodeError:
    sys.exit(0)
cmd = data.get('tool_input', {}).get('command', '')
# Match both plain invocations and rtk-prefixed ones (e.g. `rtk cargo add ...`)
m = re.search(r'\b(?:rtk\s+)?(cargo|pnpm|npm|yarn)\s+add\s+(--[\w-]+\s+)*([@\w][\w@./-]*)', cmd)
if m:
    pkg = m.group(3)
    print(f'NOTE: `{pkg}` was just added. Invoke the `sync-stack-llms` subagent to add a docs/references/ entry if the package has significant API surface.')
