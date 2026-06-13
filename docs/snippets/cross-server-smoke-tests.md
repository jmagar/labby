# Cross-Server Snippet Smoke Tests

Generated while authoring the cross-server snippets on 2026-06-13.

## Catalog

The live Code Mode proxy exposed 307 callable tools across 35 upstream namespaces. The generated proxy omitted SWAG and Google upstreams during this run because gateway-subject OAuth credentials needed reauthorization.

## Passed Tools Used By Snippets

### `cross-server-docs-brief.md`

- `time::get_current_time`
- `context7::resolve-library-id` with `{ libraryName, query }`
- `context7::query-docs`
- `searxng::searxng_web_search`
- `docs-mcp-cloudflare-com::search_cloudflare_documentation`
- `github::search_repositories`
- `axon::axon` with `action: "search"`

### `repo-context-triage.md`

- `time::get_current_time`
- `filesystem::read_file`
- `lumen::semantic_search`
- `octocode::localSearchCode` with `queries[].pattern`
- `github::search_issues`
- `github::get_file_contents`

### `homelab-readonly-pulse.md`

- `time::get_current_time`
- `dozzle::list_hosts`
- `dozzle::list_containers`
- `cortex::cortex` with `action: "search"`
- `unrust::unraid` with `action: "server"`
- `unrust::unraid` with `action: "info"`
- `unrust::unraid` with `action: "notifications"`
- `rustify::gotify` with `action: "health"`

## Passed But Not Used

- `rustarr::rustarr` with `action: "help"`
- `rustscale::tailscale` with `action: "help"`
- `rustifi::unifi` with `action: "help"`
- `unrust::unraid` with `action: "help"` and `action: "array"`
- `rustify::gotify` with `action: "help"`
- `arcane-mcp::arcane` with `action: "help"`
- `apprise-mcp::apprise` with `action: "help"`
- `lumen::health_check`
- `shadcn::search_items_in_registries`
- `shadcn::get_audit_checklist`
- `open-design::list_projects`
- `the-agent-times::get_latest_articles`
- `repomix::file_system_read_file`

## Failed Or Skipped

- `context7::resolve-library-id` failed with only `libraryName` or only `query`; it passed when both fields were supplied.
- `axon::axon` with `action: "stats"` failed against local Qdrant; `action: "search"` passed with a stale-binary warning.
- `rustscale::tailscale` with `action: "devices"` failed with `upstream_error`.
- `rustifi::unifi` with `action: "health"` failed with `upstream_error`.
- `arcane-mcp::arcane` with `action: "environment", subaction: "list"` failed because the API key lacked permission.
- `octocode::localSearchCode` failed until `queries[].pattern` was supplied.
- `shadcn::search_items_in_registries` executed but reported no configured registries, so it was not useful as a reusable cross-server snippet input.

## Verification Commands

Extract and execute a snippet from its markdown file:

```bash
labby gateway code exec --json --code "$(awk '/^```js$/{flag=1;next}/^```$/{if(flag){exit}}flag' docs/snippets/cross-server-docs-brief.md)"
labby gateway code exec --json --code "$(awk '/^```js$/{flag=1;next}/^```$/{if(flag){exit}}flag' docs/snippets/repo-context-triage.md)"
labby gateway code exec --json --code "$(awk '/^```js$/{flag=1;next}/^```$/{if(flag){exit}}flag' docs/snippets/homelab-readonly-pulse.md)"
```
