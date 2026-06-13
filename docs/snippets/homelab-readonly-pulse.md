# Homelab Read-Only Pulse

Use this snippet for a small read-only health pulse across the homelab. It intentionally avoids write, notification-send, destructive, and permission-fragile actions.

Live smoke-tested tools before authoring:

- `time::get_current_time`
- `dozzle::list_hosts`
- `dozzle::list_containers`
- `cortex::cortex` with `action: "search"`
- `unrust::unraid` with `action: "server"`
- `unrust::unraid` with `action: "info"`
- `unrust::unraid` with `action: "notifications"`
- `rustify::gotify` with `action: "health"`

Actions tested and deliberately excluded because they failed from Code Mode in this session:

- `rustscale::tailscale` with `action: "devices"`
- `rustifi::unifi` with `action: "health"`
- `arcane-mcp::arcane` with `action: "environment", subaction: "list"`

Run with:

```bash
labby gateway code exec --json --code "$(awk '/^```js$/{flag=1;next}/^```$/{if(flag){exit}}flag' docs/snippets/homelab-readonly-pulse.md)"
```

```js
async () => {
  const input = {
    timezone: "America/New_York",
    logQuery: "error",
    logLimit: 5,
    notificationLimit: 5,
    containerSample: 12
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
    timed("gotify_health", "rustify::gotify", { action: "health" })
  ]);

  return {
    snippet: "homelab_readonly_pulse",
    input,
    ok: calls.every((call) => call.ok),
    calls
  };
}
```
