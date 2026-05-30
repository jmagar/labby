# squirts

## Identity

Squirts is a homelab remote host. Fill in the durable role statement here once
the live service map is confirmed.

## Access

- SSH alias: `squirts`
- Use non-interactive SSH: `ssh -o BatchMode=yes squirts <command>`.
- Prefer read-only inspection before changing services or files.

## Network

- Tailscale name: `squirts`
- Record LAN IPs, public routes, and reverse-proxy domains here after verifying
  them live.

## Important Paths

- Add canonical service config paths here.
- Add backup or rollback locations here.

## Guardrails

- Validate service configuration before restarting anything.
- Capture before/after evidence for operational changes.
- Do not edit certificate material, firewall policy, or auth config without an
  explicit request and a rollback path.

## Common Workflows

### Inspect Host

1. Load Plexus context with `remote-context.py squirts`.
2. Check uptime, disk, memory, failed systemd units, Docker containers, and
   recent syslog events.
3. Decide whether a service-specific skill should take over.

## Related Skills

- `homelab-map`
- `tailscale`
- `unraid`
- `create-swag-config`
