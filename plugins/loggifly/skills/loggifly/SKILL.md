---
name: loggifly
description: LoggiFly — Docker container log alerting via keyword/regex patterns. Use when the user wants to set up or edit log-based alerts, monitor container logs for keywords, or configure LoggiFly notifications. LoggiFly is operated through its config.yaml, not a query API.
---

# LoggiFly

[LoggiFly](https://github.com/clemcer/loggifly) watches Docker container (and Swarm service) logs and fires notifications or container actions when log lines match keywords/regex. It is **not** a queryable REST service — you operate it by editing its `config.yaml`, which it hot-reloads.

## How it's operated

LoggiFly runs as a container with `config.yaml` mounted at `/config/config.yaml`. Edit that file and save — with `settings.reload_config: true` (default) LoggiFly reloads automatically. A few global settings can also come from environment variables (see the [env-vars reference](https://clemcer.github.io/loggifly/guide/environment-variables)).

Find the config on the host (it's the bind-mount behind `/config`):

```bash
# inspect the running container's mounts to locate config.yaml on the host
docker inspect loggifly --format '{{range .Mounts}}{{.Source}} -> {{.Destination}}{{println}}{{end}}'
docker logs --tail 50 loggifly        # LoggiFly logs config-reload + match events here
```

## config.yaml structure (v2)

```yaml
version: 2

global:
  keywords:                 # applied to EVERY matched target
    - critical
    - keyword: "out of memory"
  defaults:                 # inheritable defaults (overridable per source/rule/keyword)
    trigger_cooldown: 0
    attach_logfile: false
    title_template: "{{ container_name }}: {{ keywords }}"

settings:                   # global-only, non-inheritable
  log_level: INFO
  reload_config: true       # auto-reload config.yaml on change
  system_notifications: true

notifications:              # configure at least one
  ntfy:
    url: "http://your-ntfy-server"
    topic: "loggifly"
    token: "ntfy-token"
  apprise:
    url: "discord://webhook-id/token"   # any Apprise-compatible URL
  webhook:
    url: "https://webhook.example.com/endpoint"

containers:                 # source config for Docker containers
  rules:                    # a container is monitored if it matches >=1 rule
    - container_name: nginx           # shorthand match (glob ok)
      keywords:
        - error
        - regex: "upstream.*failed"
    - match:                          # full match syntax
        include: { container_names: ["web-*", "*-api"] }
        exclude: { container_names: ["*-test"] }
      keywords:
        - keyword: panic
          ntfy_priority: 5
          attach_logfile: true
          container_action: restart   # restart/stop/start on match (restart@other-container also works)
      container_events:               # lifecycle events: start|stop|die|crash|oom|unhealthy|...
        - event: crash
          container_action: restart

swarm:                      # optional: same shape, uses service_name/stack_name; actions need @target
  rules:
    - service_name: my-service
      keywords: [timeout]
```

Key concepts:
- **keywords** can be plain strings, `{ keyword: ... }`, or `{ regex: ... }`; settings cascade global → source → rule → keyword.
- **container_action** (`restart`/`stop`/`start`, or `restart@other`) acts on the container on match.
- **trigger_on** (`count` + `timeframe`) delays a trigger until N matches in a window; **all_of** requires all members to match the same line.
- **templates** (`title_template`, `message_template`) are Jinja2 with vars like `{{ container_name }}`, `{{ keywords }}`, `{{ log_entry }}`.

Authoritative references: [full config reference](https://github.com/clemcer/loggifly/blob/main/docs/configs/config_reference.yaml) · [config guide](https://clemcer.github.io/loggifly/guide/config/).

## Verify it's running

```bash
docker ps --filter name=loggifly
docker logs --tail 30 loggifly      # look for "config reloaded" and match/notification events
```

## When NOT to use this skill

- The user is asking about a different homelab service — load that service's skill instead.
- The user wants ad-hoc log search across containers (not standing alerts) — use Docker/`docker logs` or a log-aggregation tool, not LoggiFly config.
