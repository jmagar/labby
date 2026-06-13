---
name: homelab-readonly-pulse
description: Read-only homelab pulse across time, containers, Unraid, Gotify, and Synapse2
tags: [homelab, readonly, ops]
inputs:
  timezone:
    type: string
    default: America/New_York
    required: false
    description: IANA timezone for the timestamp call
  log_query:
    type: string
    default: error
    required: false
    description: Cortex log search query
  log_limit:
    type: integer
    default: 5
    required: false
    description: Maximum logs to include
  notification_limit:
    type: integer
    default: 5
    required: false
    description: Maximum Unraid notifications to include
  container_sample:
    type: integer
    default: 12
    required: false
    description: Maximum containers to sample
  identity_hosts:
    type: array
    default: ["dookie", "squirts"]
    required: false
    description: Host aliases for read-only identity checks
---

# Homelab Read-Only Pulse

Use this snippet for a small health pulse across the homelab. It avoids notification-send and state-changing actions. The Synapse2 `scout exec` checks use allowlisted read-only commands and require the local Labby upstream env override `SYNAPSE_MCP_ALLOW_DESTRUCTIVE=true`.

## Tutorial: How This Snippet Is Built

This snippet is a read-only operations dashboard assembled from existing MCP tools. It does not invent new checks; it chooses safe tool actions and normalizes their outputs into one pulse.

| Step | Tool | Why it is included | Parameters the user fills |
|---|---|---|---|
| Timestamp | `time::get_current_time` | Anchors the report in local time | `timezone` |
| Docker hosts | `dozzle::list_hosts` | Shows which Docker hosts are visible | none |
| Docker containers | `dozzle::list_containers` | Counts container states and samples names/images | none; snippet input controls sample size |
| Logs | `cortex::cortex` | Pulls recent matching logs | `action`, `params.query`, `params.limit` |
| Unraid server | `unrust::unraid` | Checks Unraid server reachability | `action` |
| Unraid info | `unrust::unraid` | Adds host/OS/CPU summary | `action` |
| Unraid notifications | `unrust::unraid` | Captures warning/alert counts | `action`, `params.limit` |
| Gotify health | `rustify::gotify` | Confirms notification service health without sending | `action` |
| Synapse nodes | `synapse2::scout` | Lists configured Synapse hosts | `action` |
| Synapse host status | `synapse2::flux` | Gets per-host Docker/system status | `action`, `subaction` |
| Synapse identity | `synapse2::scout` | Runs allowlisted `hostname` checks | `action`, `host`, `command` |

The authoring pattern is still simple: pick read-only tools, fill their schema fields, then decide which result fields are worth keeping. The snippet intentionally drops noisy raw payload fields and returns summaries like container counts, Synapse host status, Unraid notification counts, and per-call timings.

## Why The Inputs Exist

- `timezone` feeds the timestamp tool.
- `log_query` and `log_limit` feed the Cortex `search` action.
- `notification_limit` limits Unraid notification detail.
- `container_sample` controls how many containers are included in the sample after counting all containers.
- `identity_hosts` expands into one read-only Synapse `hostname` command per host.

These defaults let the snippet run with no arguments. Users only change inputs when they want to focus the pulse, for example `--param log_query=oauth` or `--param identity_hosts='["dookie","squirts","tootie"]'`.

## What Validation Should Catch

The builder should catch type and action-shape mistakes before execution:

- `time::get_current_time.timezone` must be a string.
- `cortex::cortex.params.limit` must be an integer.
- `unrust::unraid.action` must be one of the supported action strings.
- `synapse2::scout.host` must be a string when creating per-host exec calls.
- `identity_hosts` must be an array before expanding it into multiple calls.

This is also where read-only intent should be obvious in the UI. The selected actions should be shown with destructive metadata from the gateway catalog, and the builder should make it clear that this snippet avoids send/delete/mutate actions.

Live smoke-tested tools before authoring:

- `time::get_current_time`
- `dozzle::list_hosts`
- `dozzle::list_containers`
- `cortex::cortex` with `action: "search"`
- `unrust::unraid` with `action: "server"`
- `unrust::unraid` with `action: "info"`
- `unrust::unraid` with `action: "notifications"`
- `rustify::gotify` with `action: "health"`
- `synapse2::scout` with `action: "nodes"`
- `synapse2::flux` with `action: "host", subaction: "status"`
- `synapse2::scout` with `action: "exec"` and allowlisted read-only `hostname`

Actions tested and deliberately excluded because they failed from Code Mode in this session:

- `rustscale::tailscale` with `action: "devices"`
- `rustifi::unifi` with `action: "health"`
- `arcane-mcp::arcane` with `action: "environment", subaction: "list"`

Run with:

```bash
labby gateway code exec --json --code "$(awk '/^```js$/{flag=1;next}/^```$/{if(flag){exit}}flag' docs/snippets/homelab-readonly-pulse.md)"
```

```js
async (overrides = {}) => {
  const input = {
    timezone: "America/New_York",
    logQuery: overrides.log_query ?? "error",
    logLimit: overrides.log_limit ?? 5,
    notificationLimit: overrides.notification_limit ?? 5,
    containerSample: overrides.container_sample ?? 12,
    identityHosts: overrides.identity_hosts ?? ["dookie", "squirts"],
    ...overrides
  };

  const timed = async (label, id, params, transform = (x) => x) => {
    const started = Date.now();
    try {
      const result = await callTool(id, params);
      return {
        label,
        id,
        ok: true,
        ms: Date.now() - started,
        result: transform(result)
      };
    } catch (error) {
      return {
        label,
        id,
        ok: false,
        ms: Date.now() - started,
        error: String(error)
      };
    }
  };

  const calls = await Promise.all([
    timed("timestamp", "time::get_current_time", { timezone: input.timezone }),
    timed(
      "docker_hosts",
      "dozzle::list_hosts",
      {},
      (hosts) => hosts.map((host) => ({
        name: host.name,
        available: host.available,
        type: host.type,
        dockerVersion: host.dockerVersion,
        cpu: host.nCPU,
        memTotal: host.memTotal
      }))
    ),
    timed(
      "docker_containers",
      "dozzle::list_containers",
      {},
      (containers) => {
        const byState = {};
        const byHost = {};
        for (const container of containers) {
          byState[container.state] = (byState[container.state] || 0) + 1;
          byHost[container.host] = (byHost[container.host] || 0) + 1;
        }
        return {
          total: containers.length,
          byState,
          byHost,
          sample: containers.slice(0, input.containerSample).map((container) => ({
            name: container.name,
            image: container.image,
            state: container.state,
            host: container.host
          }))
        };
      }
    ),
    timed(
      "recent_logs",
      "cortex::cortex",
      { action: "search", params: { query: input.logQuery, limit: input.logLimit } },
      (result) => ({
        count: result.count,
        logs: (result.logs || []).slice(0, input.logLimit).map((log) => ({
          timestamp: log.timestamp,
          hostname: log.hostname,
          app_name: log.app_name,
          severity: log.severity,
          message: log.message
        }))
      })
    ),
    timed("unraid_server", "unrust::unraid", { action: "server" }),
    timed(
      "unraid_info",
      "unrust::unraid",
      { action: "info" },
      (result) => ({
        hostname: result.info?.os?.hostname,
        distro: result.info?.os?.distro,
        release: result.info?.os?.release,
        kernel: result.info?.os?.kernel,
        cpu: result.info?.cpu?.brand,
        cores: result.info?.cpu?.cores,
        threads: result.info?.cpu?.threads
      })
    ),
    timed(
      "unraid_notifications",
      "unrust::unraid",
      { action: "notifications", params: { limit: input.notificationLimit } },
      (result) => ({
        overview: result.notifications?.overview,
        warningsAndAlerts: {
          has_more: result.notifications?.warningsAndAlerts?.has_more,
          items: (result.notifications?.warningsAndAlerts?.items || [])
            .slice(0, input.notificationLimit)
            .map((item) => ({
              title: item.title,
              subject: item.subject,
              importance: item.importance,
              timestamp: item.timestamp
            }))
        }
      })
    ),
    timed("gotify_health", "rustify::gotify", { action: "health" }),
    timed(
      "synapse_nodes",
      "synapse2::scout",
      { action: "nodes" },
      (result) => ({
        total: result.hosts?.length || 0,
        hosts: (result.hosts || []).map((host) => ({
          name: host.name,
          host: host.host,
          protocol: host.protocol,
          sshUser: host.sshUser,
          sshPort: host.sshPort,
          dockerSocketPath: host.dockerSocketPath
        }))
      })
    ),
    timed(
      "synapse_host_status",
      "synapse2::flux",
      { action: "host", subaction: "status" },
      (result) => ({
        count: result.count,
        partial: result.partial,
        errors: result.errors || {},
        status: (result.status || []).map((host) => ({
          name: host.name,
          connected: host.connected,
          dockerVersion: host.dockerVersion,
          containerCount: host.containerCount,
          runningCount: host.runningCount,
          failedServiceCount: host.failedServiceCount
        }))
      })
    ),
    ...(input.identityHosts || []).map((host) =>
      timed(
        `synapse_identity_${host}`,
        "synapse2::scout",
        { action: "exec", host, command: "hostname" },
        (result) => ({
          host: result.host,
          command: result.command,
          exit_code: result.exit_code,
          stdout: String(result.stdout || "").trim(),
          stderr: String(result.stderr || "").trim()
        })
      )
    )
  ]);

  const byLabel = Object.fromEntries(calls.map((call) => [call.label, call]));
  const synapseStatus = byLabel.synapse_host_status?.result;
  const dockerContainers = byLabel.docker_containers?.result;
  const unraidNotifications = byLabel.unraid_notifications?.result;

  return {
    snippet: "homelab_readonly_pulse",
    input,
    ok: calls.every((call) => call.ok),
    summary: {
      docker_hosts: byLabel.docker_hosts?.result?.length,
      docker_containers: dockerContainers?.total,
      docker_container_states: dockerContainers?.byState,
      synapse_hosts: byLabel.synapse_nodes?.result?.total,
      synapse_partial: synapseStatus?.partial || false,
      synapse_errors: synapseStatus?.errors || {},
      unraid_unread_notifications: unraidNotifications?.overview?.unread?.total,
      unraid_warning_notifications: unraidNotifications?.overview?.unread?.warning,
      gotify_health: byLabel.gotify_health?.result?.health
    },
    calls
  };
}
```
