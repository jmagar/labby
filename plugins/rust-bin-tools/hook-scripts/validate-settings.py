#!/usr/bin/env python3
"""PostToolUse hook: validate Claude/Codex/Gemini settings files against schema."""
import json, sys, os, subprocess

data = json.load(sys.stdin)
path = data.get('tool_input', {}).get('file_path', '')
base = os.path.basename(path)

project_dir = os.environ.get('CLAUDE_PROJECT_DIR', os.getcwd())

is_settings = (
    (base == 'config.toml' and '.codex/' in path) or
    (base == 'settings.json' and ('.claude/' in path or '.gemini/' in path)) or
    (base == 'settings.local.json' and '.claude/' in path)
)
if is_settings:
    # Prefer CLAUDE_PLUGIN_ROOT (set by Claude Code when running a plugin hook) so this
    # works regardless of CWD or whether CLAUDE_PROJECT_DIR is set.  Fall back to the
    # project-relative path for local dev runs where the plugin is not installed.
    plugin_root = os.environ.get('CLAUDE_PLUGIN_ROOT', '')
    if plugin_root:
        validate_script = os.path.join(plugin_root, 'skills/agent-config/scripts/validate-settings.sh')
    else:
        validate_script = os.path.join(project_dir, '.claude/skills/agent-config/scripts/validate-settings.sh')

    if not os.path.exists(validate_script):
        sys.exit(0)  # validation script not available in this context; skip silently

    try:
        r = subprocess.run([validate_script, path], capture_output=True, text=True)
    except OSError:
        sys.exit(0)
    if r.returncode != 0:
        print(f'⚠ Settings/config schema validation FAILED (exit {r.returncode}) — fix before relying on the file.')
        if r.stdout:
            print(r.stdout)
        if r.stderr:
            print(r.stderr)
