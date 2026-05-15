#!/usr/bin/env python3
"""PostToolUse hook: auto-format Rust/TOML files and sync CLAUDE.md/skills/manifests."""
import json, sys, os, subprocess

data = json.load(sys.stdin)
path = data.get('tool_input', {}).get('file_path', '')
base = os.path.basename(path)

plugin_root = os.environ.get('CLAUDE_PLUGIN_ROOT', '')
project_dir = os.environ.get('CLAUDE_PROJECT_DIR', os.getcwd())

def script(rel):
    # Prefer CLAUDE_PLUGIN_ROOT so plugin-internal scripts resolve correctly
    # regardless of CWD. Fall back to project-relative path for local dev.
    if plugin_root:
        p = os.path.join(plugin_root, 'skills', rel.split('skills/', 1)[-1] if 'skills/' in rel else rel)
    else:
        p = os.path.join(project_dir, rel)
    return p

def run(cmd, **kwargs):
    """Run a subprocess, returning None silently if the executable is missing."""
    try:
        return subprocess.run(cmd, **kwargs)
    except OSError:
        return None

if path.endswith('.rs'):
    run(['cargo', 'fmt', '--', path], capture_output=True)
elif path.endswith('.toml'):
    run(['taplo', 'fmt', path], capture_output=True)
elif base == 'CLAUDE.md':
    s = script('.claude/skills/sync-claude-mds/scripts/sync-claude-mds.sh')
    if os.path.exists(s):
        run([s, '--quiet'], capture_output=True)
elif '.claude/skills/' in path:
    s = script('.claude/skills/sync-skills/scripts/sync-skills.sh')
    if os.path.exists(s):
        run([s, '--quiet'], capture_output=True)
elif base == 'server.json':
    s = script('.claude/skills/mcp-registry-publish/scripts/validate-server-json.sh')
    if os.path.exists(s):
        r = run([s, path], capture_output=True, text=True)
        if r and r.returncode != 0:
            print(f'⚠ server.json validation FAILED (exit {r.returncode}) — see details below. This is a warning; fix before `mcp-publisher publish`.')
            print(r.stdout)
else:
    is_plugin_manifest = (
        base in ('marketplace.json', 'plugin.json') and
        ('.claude-plugin/' in path or '.codex-plugin/' in path or '/.agents/plugins/' in path)
    ) or base == 'gemini-extension.json'
    if is_plugin_manifest:
        s = script('.claude/skills/agent-config/scripts/validate-manifest.sh')
        if os.path.exists(s):
            r = run([s, path], capture_output=True, text=True)
            if r and r.returncode != 0:
                print(f'⚠ Plugin manifest schema validation FAILED (exit {r.returncode}) — fix before publishing.')
                if r.stdout:
                    print(r.stdout)
        if '.claude-plugin/' in path:
            import shutil
            if shutil.which('claude'):
                target = os.path.dirname(os.path.dirname(path)) or '.'
                r2 = run(['claude', 'plugin', 'validate', target], capture_output=True, text=True)
                if r2 and r2.returncode != 0:
                    print(f'⚠ `claude plugin validate {target}` FAILED (exit {r2.returncode}) — authoritative validator.')
                    if r2.stdout:
                        print(r2.stdout)
                    if r2.stderr:
                        print(r2.stderr)
