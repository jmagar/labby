---
source: https://weisser-zwerg.dev/posts/incus-codex-jail/
title: Incus System-Container Jail for the Codex Coding Agent
scraped_at: 2026-06-27
---

# Incus Codex Jail Reference

This note preserves the project-relevant patterns from the article without
vendoring the full scraped page into the repo.

## Patterns Used

- Prefer an Incus system container when an agent runtime needs a normal Linux
  user space, package managers, service management, and persistent caches.
- Treat the container as an isolation boundary for agent work, not as a full
  security sandbox. Host/network/device access still needs explicit policy.
- Keep sensitive host paths and credentials out of the container by default.
- Make the container's declarative shape visible as an artifact so it can be
  reviewed and updated separately from imperative bootstrap logic.
- Use explicit network and device decisions. Labby's supported gateway profile
  currently includes `/dev/net/tun` for Tailscale and documents the tradeoff.
- Keep resource and rollback procedures practical: the operator should be able
  to inspect, stop, and delete the container without reverse-engineering the
  bootstrap script.

## Labby Application

Labby applies these patterns through `config/incus/labby-gateway-profile.yaml`,
`scripts/incus-bootstrap.sh`, and `docs/runtime/INCUS.md`. The profile owns the
declarative Incus configuration; the script owns create/update/validate,
binary install, provisioning, and optional Tailscale join.
