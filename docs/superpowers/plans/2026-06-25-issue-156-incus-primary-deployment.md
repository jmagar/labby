## Summary

Replace Docker with an **Incus system container** as the primary, supported way to self-host the Labby gateway. Docker is the wrong shape for this workload: the gateway spawns arbitrary stdio MCP servers and runs the agent CLIs (`claude`, `codex`, `gemini`), so its dependency closure is user-defined at runtime, not knowable at image-build time. The current Docker deployment papers over that with many bind mounts, credential-lockstep file mounts, hostname hacks, and per-binary mounts. Almost all of that exists to defeat the host/container boundary, which means Docker is the wrong boundary.

An Incus system container behaves like a lightweight VM sharing the host kernel: full systemd, normal package installation, SSH access, and a persistent rootfs. The migration is mostly deletion of compose-specific workaround config, not a one-to-one translation.

This issue is now self-contained. It also has a local Beads plan for execution tracking:

- Epic: `lab-fh1wv` — Make Incus the primary supported Labby gateway deployment path
- Children: `lab-fh1wv.1` through `lab-fh1wv.6`
- `bd swarm validate lab-fh1wv` passes.
- Ready waves: `.1` -> `.2` -> `.3/.4/.6` -> `.5`; max parallelism 3.

## What Already Exists

Build on this; do not rebuild it.

- `scripts/install.sh` is already the bootstrap we want. Its job is getting `labby` on PATH; everything after is owned by the binary via `labby setup`. It fetches the prebuilt release and SHA-verifies where possible, with a cargo fallback. It supports `LAB_INSTALL_DIR`, which is required for installing inside the Incus container to `/usr/local/bin`.
- `labby setup host-service install|status|restart|uninstall` in `crates/labby/src/dispatch/setup/host_service.rs` is already a complete service manager: atomic unit write, `systemd-analyze verify`, `/ready` readiness poll, PID ownership validation, Docker `labby-master` conflict preflight, and `systemctl show` status parsing. It is currently built around `systemd --user`; this issue changes that default.
- `.github/workflows/release.yml` already builds the Linux release artifact and bundles the gateway-admin frontend.
- Prior host-service plan and implementation notes live in `docs/superpowers/plans/2026-06-22-host-labby-gateway.md` and `docs/sessions/2026-06-22-host-labby-gateway-work-it.md`.

## Hard Constraint: amd64 Debian 13 Incus

Both install/release constraints currently make amd64 the supported substrate. `rquickjs-sys`, used by Code Mode's QuickJS engine, does not cross-compile cleanly in the current release path. The supported self-hosted deployment substrate for this issue is therefore:

- **amd64**
- **Debian 13**
- **Incus system container**

Document this explicitly. ARM users do not have the same prebuilt path today and may fall back to a slower cargo build. The optional distrobuilder image is amd64-only as well.

## Runtime Floor

Labby is a gateway for running stdio MCP servers and agent CLIs, so the runtime environment is the floor by definition.

Bake/install:

- `node`
- `python`
- `uv`
- `git`
- `gh`
- `openssh-client`
- `claude`
- `codex`

Do not bake:

- `ffmpeg` and other per-upstream leaf libraries. They are handled by just-in-time dependency diagnostics and explicit operator action.

Privilege split:

- `[root]` apt: `git`, `openssh-client`, `gh`, `ca-certificates`, `curl`, `zsh`
- `[lab]` user-space: `node`, Python via `uv python install`, `claude`, `codex`

Node decision:

- Do **not** use `fnm`. It depends on shell hooks and silently falls off PATH in systemd/non-login contexts.
- Use NodeSource apt or the official static tarball into `/usr/local`; either path must put `node`, `npm`, and `npx` on PATH for the `lab` user and systemd service.
- `mise` is acceptable only if a manager is genuinely wanted, but it is unnecessary for one pinned Node version.

## Architecture

There are two honest layers:

1. `scripts/incus-bootstrap.sh` runs on the host. It creates/configures the Incus container, passes `/dev/net/tun`, installs `labby` inside the box, then runs the in-box provisioning flow.
2. `labby setup --provision` runs inside the box, or on bare metal when desired. It owns the environment: install the floor, create the `lab` user, write the system unit, and enable/start the service.

The long-running `labby serve` process stays small, unprivileged, and responsible only for its own runtime. Privileged one-time setup stays out of `serve`.

## Locked Decisions

- Incus system container is the primary supported self-hosted deployment path.
- Supported substrate is amd64 Debian 13.
- Docker remains only an explicit compatibility/dev-container/prod-like smoke path until deprecation docs and checks are updated.
- Long-running `labby serve` stays unprivileged.
- Privileged setup is explicit, bounded, and consent-gated.
- The system service runs as `User=labby` / `Group=labby`, not `systemd --user`.
- Tailscale runs inside the container and requires `/dev/net/tun` passthrough.
- Adding upstreams must not silently run apt. Missing leaf dependencies are diagnosed or declared and require explicit operator consent.
- Mutating `setup --provision` execution is local CLI-only. Do not expose root apt/useradd/systemctl execution through MCP, HTTP, Code Mode, or remote admin actions.
- Distrobuilder is optional and downstream of the universal install/provision path.

## Security Model

Threat model: supply-chain-compromised upstream dependency or opportunistic automated abuse, not a targeted kernel 0-day attacker.

Required controls:

- Unprivileged Incus container.
- `security.idmap.isolated=true`.
- `security.nesting=false`.
- Keep Incus default AppArmor profile.
- Exactly one host passthrough device: `/dev/net/tun`.
- Hardened systemd unit constrains `labby` and all spawned child processes.
- Downloads/scratch dir should be `noexec,nosuid,nodev` where practical.
- Optional LAN egress hardening can block RFC1918 LAN ranges while allowing internet and tailnet dependencies.
- Future per-child sandboxing can shrink blast radius from whole container to one upstream, but that is not v1.

## Engineering Review Guardrails

Architecture review:

- The substrate split is sound: host bootstrap, in-box provision, unprivileged `labby serve`.
- Reuse `host_service.rs` primitives instead of replacing them.
- `--install-self` currently means copy to `~/.local/bin`; the system unit wants `/usr/local/bin/labby`. Do not overload semantics accidentally.
- Tailscale ownership must be clear: bootstrap passes TUN/auth context; provision may install/join, but one path owns it.
- Known-upstream dependency hints are advisory diagnostics, not install authority.

Simplicity review:

- Convert `host_service.rs` in place. Do not build `SystemdBackend`, `PackageManager`, `ProvisionProvider`, boxed executor traits, or a multi-distro abstraction.
- Target one supported substrate: amd64 Debian 13.
- Keep `incus-bootstrap.sh` as shell, not Rust.
- Keep Docker as explicit smoke/dev path, not a second first-class deployment model.
- Do not start distrobuilder before the plain install-script plus provision path works.

Security review:

- Provisioning must stay local CLI-only.
- Dependency diagnostics can leak secrets; add a final operator-visible sanitizer.
- Incus production bootstrap install path must fail closed: require checksum, pin release/version, ignore unsafe inherited `LAB_INSTALL_REPO`, and make source-build fallback explicit.
- Avoid writable PATH persistence risk under `User=labby`: no writable directories before trusted binaries.
- `TS_AUTHKEY` must never be printed, persisted to systemd env files, captured in traces, or left in process args longer than necessary.
- Incus isolation requirements are acceptance criteria, not docs-only.

Performance/reliability review:

- `/ready` alone is insufficient; system unit can be ready while stdio upstreams fail.
- Smoke at least one `npx` and one `uvx` upstream under the hardened service.
- Provisioning must be resumable and idempotent after partial failure.
- Status ownership checks may need `owner=unknown` when non-root callers cannot see `ss -ltnp` process ownership.
- Dependency diagnostics must reuse existing bounded upstream stderr capture and circuit-breaker paths; do not add serial bulk probes.
- Avoid any path that can turn import/provision into `N * 15s` serial probes across many upstreams.

## Research Findings

- systemd docs support the system-unit direction: `WantedBy=multi-user.target` ties the service to normal headless/server boot. Hardening directives such as `ProtectSystem=strict`, `ProtectHome`, `NoNewPrivileges`, `ReadWritePaths`, and `RestrictSUIDSGID` affect child MCP and agent processes and must be smoke-tested.
- Incus docs distinguish `incus init` from `incus launch`; `incus launch` creates and starts the instance. Incus `unix-char` devices expose host character devices like `/dev/net/tun` inside the instance.
- Tailscale supports unattended joins with `tailscale up --auth-key=...`; auth keys can also be supplied through a `file:` path. Tailscale SSH is a tailnet SSH authorization layer, so `--ssh` needs explicit operator awareness and ACL implications in docs.
- MCP tool discovery does not expose system package requirements. Launcher token inspection can validate only runtime floor classes such as `uvx`, `npx`, `python`, and `ssh`; leaf deps need curated hints or stderr diagnosis.
- `.lavra/memory/recall.sh` was absent in this worktree, so Lavra local memory recall was not available. Research used repo evidence plus current primary docs.

External docs referenced:

- Incus instance creation: https://linuxcontainers.org/incus/docs/main/howto/instances_create/
- Incus `unix-char` devices: https://linuxcontainers.org/incus/docs/main/reference/devices_unix_char/
- systemd exec/unit docs: https://www.freedesktop.org/software/systemd/man/systemd.exec.html and https://www.freedesktop.org/software/systemd/man/systemd.unit.html
- Tailscale CLI up: https://tailscale.com/docs/reference/tailscale-cli/up
- Tailscale SSH: https://tailscale.com/docs/features/tailscale-ssh

## Work Item 1 / `lab-fh1wv.1` — Convert Host Service to Hardened System Unit

### What

Convert the existing `labby setup host-service` implementation from a `systemd --user` service to a hardened system unit suitable for a headless Incus container, while preserving a non-default workstation/user-service escape hatch only if needed.

### Context

`host_service.rs` currently owns unit rendering, install/status/restart/uninstall, preflight port checks, Docker `labby-master` conflict detection, readiness polling, `systemctl show` parsing, PID ownership validation, and atomic unit writes. Reuse those pieces. The core issue is that `systemd --user` is wrong for a headless container because it relies on user D-Bus/session state or linger.

### Required Changes

- `unit_dir` / `unit_path` -> `/etc/systemd/system/labby.service`.
- `run_systemctl` drops the `--user` flag for default system mode.
- Unit gains `User=labby`, `Group=labby`, and `WantedBy=multi-user.target`.
- Drop `%h` user-manager path tricks; use absolute paths.
- `ExecStart=/usr/local/bin/labby serve`.
- Install binary to `/usr/local/bin/labby`, root-owned and executable by `lab`.
- Optionally keep previous `systemd --user` mode behind an explicit non-default flag/subcommand, or drop it entirely if no real need remains.

### Hardening Baseline

Start from this, then relax only if required by smoke tests:

```ini
[Service]
User=labby
Group=labby
ExecStart=/usr/local/bin/labby serve
WorkingDirectory=/home/labby
Environment=HOME=/home/labby
Environment=XDG_CACHE_HOME=/home/labby/.cache
Environment=XDG_CONFIG_HOME=/home/labby/.config
Environment=XDG_DATA_HOME=/home/labby/.local/share
NoNewPrivileges=true
ProtectSystem=strict
ReadWritePaths=/home/labby/.lab /home/labby/.local /home/labby/.cache /home/labby/.config /home/labby/.npm /home/labby/.codex /home/labby/.claude /home/labby/.gemini /home/labby/downloads
ProtectHome=read-only
PrivateTmp=true
RestrictNamespaces=true
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
RestrictSUIDSGID=true
LockPersonality=true
ProtectKernelTunables=true
ProtectKernelModules=true
CapabilityBoundingSet=
SystemCallFilter=@system-service
TasksMax=1000
MemoryMax=4G
Restart=on-failure
RestartSec=3
```

### Locked Decisions

- Default target is `/etc/systemd/system/labby.service` using normal `systemctl`, not `systemctl --user`.
- Unit runs `ExecStart=/usr/local/bin/labby serve` with `User=labby` and `Group=labby`.
- Unit is wanted by `multi-user.target`.
- Keep the gateway runtime unprivileged; root is only for unit write/install and package provisioning.
- Add hardening directives from this issue, but test against real stdio upstream behavior before shipping.
- Keep existing readiness and port/PID ownership checks.

### Discretion

- Whether previous user-service mode remains behind an explicit flag/subcommand or is split into a separate helper.
- Exact hardening directive set if a directive breaks required stdio/agent CLI behavior; document any deliberate relaxation.

### Testing

- [ ] Unit-rendering tests assert `User=labby`, `Group=labby`, `/usr/local/bin/labby serve`, `WantedBy=multi-user.target`, and no `%h` user-manager paths in default system mode.
- [ ] Command-construction tests prove default calls omit `--user`.
- [ ] Existing preflight/readiness/status parsing tests still pass.
- [ ] Hardened unit is verified with `systemd-analyze verify` where available.
- [ ] Smoke proves a restarted service owns the `/ready` listener.
- [ ] Smoke at least one `npx` and one `uvx` stdio upstream under the hardened service.

### Validation

- [ ] `labby setup host-service unit` renders the system unit by default.
- [ ] `labby setup host-service install --install-self -y` installs to the system path or routes through the provision flow clearly.
- [ ] Service survives a headless reboot/restart path without `loginctl enable-linger` or `XDG_RUNTIME_DIR`.
- [ ] `/ready` owner check passes when run as root during install/provision.
- [ ] `status` distinguishes `ready=true, owner=unknown` from `ready=false` when non-root callers cannot see PID ownership.

### Files

- `crates/labby/src/dispatch/setup/host_service.rs`
- `crates/labby/src/cli/setup.rs`
- `crates/labby/src/output/*` if CLI status rendering needs updates

### Dependencies

- None. This is first in the critical path.

### Notes from Research and Review

- Add a rendered-unit test that asserts every writable runtime/cache/home path needed by spawned stdio upstreams is either inside allowed labby-user paths or explicitly listed in `ReadWritePaths`; over-hardening should fail in smoke, not production.
- Acceptance must include `npx` and `uvx` stdio upstream smoke under the unit, explicit `HOME`/`XDG_*`/`PATH`/`WorkingDirectory` for `/home/labby`, and writable cache/config paths needed by spawned agent and MCP processes.
- Avoid writable directories ahead of trusted binaries in PATH.

## Work Item 2 / `lab-fh1wv.2` — Add `labby setup --provision`

### What

Add `labby setup --provision` as the one in-box provisioning command that installs the runtime floor, creates the `lab` user if needed, writes/enables the system service, and supports plan/consent/dry-run behavior.

### Context

Provisioning belongs in the binary and runs inside the Incus container or on bare metal. It must not run from the long-lived `serve` path. Existing `setup` CLI and setup dispatch are the right ownership area; keep CLI thin and put behavior in `crates/labby/src/dispatch/setup/`.

### Command Contract

- `labby setup --provision --dry-run`: print the full plan; mutate nothing.
- `labby setup --provision`: print the plan and prompt `y/N`.
- `labby setup --provision --yes`: non-interactive execution for bootstrap/CI.
- `labby setup --provision --skip-deps`: service-only path for baked image/prepared box.

### Plan Output Contract

Plan output must be bounded and explicit about privilege:

```text
labby setup --provision will:
  [root] apt install: git openssh-client gh ca-certificates curl zsh
  [lab ] install node v24.x  (NodeSource / static, on PATH)
  [lab ] install uv + python (user-space)
  [lab ] install claude + codex (npm, user-space)
  [root] write /etc/systemd/system/labby.service
  [root] useradd labby (if absent)
  [root] systemctl enable --now labby

It will NOT:
  - install or modify Incus
  - touch any package outside the list above
  - modify anything on the host outside this container
  - transmit anything off-box

Proceed? [y/N]
```

### Locked Decisions

- Root actions: apt install only the bounded floor (`git`, `openssh-client`, `gh`, `ca-certificates`, `curl`, `zsh`), create `lab` user, write `/etc/systemd/system/labby.service`, enable/start service.
- Lab-user actions: install node, uv + Python, `claude`, `codex`.
- Do not use `fnm` for Node; use NodeSource apt or official static tarball.
- Support `--dry-run`, default prompt, `--yes`, and `--skip-deps`.
- Output plan is bounded and tagged `[root]` / `[lab ]`, including an `It will NOT` section.
- Check-first/idempotent: commands no-op when required versions/tools are already present.
- Mutating provision execution is local CLI-only. Do not expose it through MCP/API/Code Mode.

### Discretion

- Exact module split under `crates/labby/src/dispatch/setup/`.
- Whether implementation shells out to package managers directly or uses small typed command builders, as long as tests cover plan construction and idempotency decisions.

### Implementation Pattern

Use a typed plan/executor, not an opaque shell script:

- `ProvisionPlan { actions, non_actions }`
- Each action should have `check`, `plan`, `execute`, `verify`, and `rollback_hint` semantics.
- Dry-run calls checks and renders the plan only.
- `--yes` verifies after every step and is safely resumable.
- Add a global provisioning lock.
- Use bounded command timeouts.
- Redact stderr.
- Keep apt package list closed.
- Use fake executor tests for dry-run/idempotency.

### Testing

- [ ] Dry-run test proves no mutating command executor is called.
- [ ] Plan rendering snapshot includes root/lab actions and explicit non-actions.
- [ ] Idempotency tests cover already-present node/python/uv/claude/codex and package floor checks.
- [ ] Confirmation behavior refuses by default and proceeds with `--yes`.
- [ ] Failure surfaces command stderr without leaking secrets.
- [ ] Tests prove mutating `setup --provision` is not in MCP/API catalogs.

### Validation

- [ ] `labby setup --provision --dry-run` exits successfully and mutates nothing.
- [ ] `labby setup --provision --yes --skip-deps` can write/enable only the service on a prepared box.
- [ ] Re-running `--provision --yes` on a provisioned box is a no-op except service status verification.
- [ ] Node/npm/npx are verified from the `lab` user before enabling service.
- [ ] `systemctl reset-failed labby.service` is used when fixing dependencies after restart-loop failures.

### Files

- `crates/labby/src/cli/setup.rs`
- `crates/labby/src/dispatch/setup.rs`
- `crates/labby/src/dispatch/setup/provision.rs`
- `crates/labby/src/dispatch/setup/host_service.rs`
- `docs/generated/*` after help regeneration

### Dependencies

- Depends on Work Item 1.

### Notes from Research and Review

- Provisioning should keep Node off shell-hook managers because systemd/non-login environments do not run interactive shell hooks.
- Incus production bootstrap should fail closed on install integrity: require release checksum, pin release/version, ignore unsafe inherited `LAB_INSTALL_REPO`, and make source-build fallback explicit.
- Partial failure is expected: apt lock, network outage, npm registry outage, expired Tailscale auth key, interrupted install. Rerun must be safe.

## Work Item 3 / `lab-fh1wv.3` — Add Incus Bootstrap and Tailscale Join

### What

Create `scripts/incus-bootstrap.sh` to launch the Debian 13 Incus system container, pass `/dev/net/tun`, install `labby` inside the container, run `labby setup --provision --yes`, and print the remaining manual login steps.

### Context

The bootstrap is the only host-side piece. It may create/configure the container and TUN device, but it must not silently install or initialize Incus itself. Tailscale runs inside the container and gets its own tailnet IP; the TUN device is mandatory and must be visible in output and docs.

### Bootstrap Shape

```sh
#!/bin/sh
set -eu
if ! command -v incus >/dev/null; then
  echo "Incus is not installed. Options:"
  echo "  [I] install it (apt, needs root) + run 'incus admin init'"
  echo "  [P] print the commands and exit"
  # read choice; never force host mutation
fi
incus launch images:debian/13 labby
incus config device add labby tun unix-char path=/dev/net/tun
incus file push scripts/install.sh labby/tmp/labby-install.sh
incus exec labby -- env \
  LAB_INSTALL_DIR=/usr/local/bin \
  LAB_INSTALL_VERSION=vX.Y.Z \
  LAB_REQUIRE_CHECKSUM=1 \
  LAB_ALLOW_SOURCE_FALLBACK=0 \
  sh /tmp/labby-install.sh
incus exec labby -- rm -f /tmp/labby-install.sh
incus exec labby -- labby setup --provision --yes
cat <<'DONE'
Done. Manual steps remain:
  1. incus exec labby -- su - lab
  2. claude login && codex login && gemini
  3. verify: incus exec labby -- systemctl status labby
DONE
```

The final implementation should avoid `sh -c` for user-controlled values. Use typed command construction wherever values can vary.

### Locked Decisions

- Detect missing Incus and offer install/print options; do not force privileged host setup silently.
- Launch `images:debian/13` amd64 container named `labby` unless overridden.
- Add TUN passthrough with `incus config device add labby tun unix-char path=/dev/net/tun`.
- Run `scripts/install.sh` inside the box with `LAB_INSTALL_DIR=/usr/local/bin`.
- Run `labby setup --provision --yes` inside the box.
- Accept `TS_AUTHKEY`; when present, provision/join Tailscale with `tailscale up --auth-key=... --ssh` only when explicitly enabled.
- Print manual `claude login`, `codex login`, and `gemini`/Gemini login guidance after service setup.

### Discretion

- CLI flags for container name, image alias, and dry-run output.
- Whether Tailscale install belongs in provision or bootstrap, provided the public contract is one bootstrap path and one provision path.

### Idempotency Requirements

Bootstrap must detect and handle:

- existing container
- correct image architecture
- TUN device already present or missing
- installed `/usr/local/bin/labby`
- provision state
- service status
- Tailscale state

A partial first run must not make reruns useless. For example, if first run creates the container but fails before adding TUN, rerun must add TUN and continue.

### Tailscale Requirements

- Tailscale runs inside the container and gets its own tailnet IP.
- `/dev/net/tun` passthrough is mandatory.
- `TS_AUTHKEY` must never be printed, persisted in systemd env files, captured in shell traces, or left in process args longer than necessary.
- Prefer ephemeral, preauthorized, tag-scoped auth keys with short expiry.
- `--ssh` exposes Tailscale SSH behavior; docs and plan output must state ACL/security implications.

### Incus Isolation Requirements

Bootstrap should verify or enforce:

- isolated idmap
- non-nested container
- non-privileged profile
- default AppArmor
- exactly one host passthrough device: `/dev/net/tun`

### Testing

- [ ] Shellcheck or equivalent syntax validation for `scripts/incus-bootstrap.sh`.
- [ ] Dry-run/print-only path can be tested without Incus mutation.
- [ ] Script refuses or prompts clearly when Incus is missing.
- [ ] Generated command sequence includes TUN device and in-box install/provision commands.
- [ ] Idempotency tests or dry-run fixtures cover existing container and existing TUN device.

### Validation

- [ ] Fresh Incus container reaches running `labby.service` with zero Docker bind mounts.
- [ ] Container survives `incus restart labby` and gateway comes back.
- [ ] Tailscale reports a 100.x tailnet IP when auth key is supplied.
- [ ] `incus restart labby` followed by `/ready` and service owner check passes.

### Files

- `scripts/incus-bootstrap.sh`
- `scripts/install.sh`
- `crates/labby/src/dispatch/setup/provision.rs` if Tailscale join is in provision
- `docs/runtime/*` for operator runbook references

### Dependencies

- Depends on Work Item 2.

### Notes from Research and Review

- Incus `unix-char` is the right mechanism for `/dev/net/tun`.
- Tailscale auth-key join is viable but must be handled as sensitive input.
- The bootstrap must not auto-install or auto-initialize Incus without explicit operator choice.

## Work Item 4 / `lab-fh1wv.4` — Just-in-Time Upstream Dependency Diagnostics

### What

Add the v1 mechanism for missing per-upstream leaf dependency handling: validate launcher runtimes from gateway config, diagnose failed first spawns from stderr, optionally consult a small known-upstreams manifest, and surface suggested fixes without silently running apt.

### Context

MCP servers do not declare system package requirements. A server announces tools, not system dependencies. Launcher commands can reveal only runtime class (`npx`, `uvx`, `python`, `ssh`), which should already be covered by the runtime floor. Leaf libraries like `ffmpeg` must be declared in curated metadata or diagnosed from process stderr.

### Design

- Runtime detection: first command token maps to runtime floor (`uvx`/`uv` -> uv, `npx`/`node` -> node, `python` -> python, `ssh` -> openssh). This validates the floor; it does not discover leaf libraries.
- Leaf diagnosis: when an upstream fails on first spawn, classify existing bounded stderr/circuit-breaker health state and produce a structured hint.
- Optional curated hints: a small `known-upstreams` mapping can exist for popular upstreams, but it must be advisory and test-backed.
- No silent apt. Any package installation is a separate explicit consent action.

### Locked Decisions

- Adding/importing an upstream must not silently mutate system packages.
- General mechanism is diagnose-first: failed spawn stderr is surfaced with a suggested fix when patterns match.
- A thin curated manifest for common upstreams is allowed but not required to block v1.
- Hook near `gateway.add` / import approval and `doctor` so failures are visible immediately and during audits.
- Redact secrets and cap stderr tail output.

### Discretion

- Manifest filename/location and exact schema.
- Initial known upstream set; keep it small and test-backed.
- Whether `apt install` execution is a follow-up action or consent-gated option in the add response.

### Required Diagnostic Shape

Expose structured redacted values only, never raw stderr:

```text
RedactedDiagnostic {
  code,
  package_hint,
  redacted_tail,
  truncated,
}
```

or equivalent:

```text
DependencyHint {
  package,
  reason,
  install_command,
  stderr_tail,
}
```

### Redaction Requirements

Operator-visible diagnostics need a final sanitizer for:

- loaded secret values
- token-like strings
- `Authorization` headers
- `TS_AUTHKEY`
- OAuth codes
- sensitive paths

Cap visible diagnostic tails by bytes and lines.

### Performance Requirements

- Reuse existing bounded upstream stderr capture and circuit-breaker paths.
- Do not add another raw process-output capture path.
- Do not add serial bulk probes outside existing single-flight/discovery-timeout/circuit-breaker behavior.
- Bulk import of many bad upstreams must not become `N * 15s` serial latency.

### Testing

- [ ] Unit tests map launcher commands to runtime-floor validation.
- [ ] Stderr pattern tests cover `ffmpeg: command not found`, `ENOENT`, and unknown failures.
- [ ] Add/import response tests include capped stderr and suggested fix without mutating packages.
- [ ] Doctor tests report missing leaf deps as actionable warnings.
- [ ] Redaction tests prove env values/tokens are not printed.
- [ ] Tests cover false-positive prevention: app-level auth/PATH errors should not become wrong apt advice.

### Validation

- [ ] Adding a `uvx` upstream with a missing leaf dependency surfaces the stderr tail and package suggestion.
- [ ] Unknown stderr remains a normal unhealthy upstream report without false apt advice.
- [ ] Existing healthy upstream add/import flows are unchanged.

### Files

- `crates/labby-gateway/src/*` or `crates/labby/src/dispatch/gateway/*` depending on current gateway ownership
- `crates/labby/src/dispatch/doctor/*`
- `crates/labby-runtime/src/*` if shared DTOs are needed
- `docs/dev/ERRORS.md` if new error/warning kinds are introduced

### Dependencies

- Depends on Work Item 2.

### Notes from Research and Review

- Existing gateway validation and doctor health/error reporting are likely the right hooks.
- The current stderr path is already bounded; classify from that path rather than capturing stderr separately.
- Keep package installation as separate explicit consent so `gateway.add` remains unsurprising.

## Work Item 5 / `lab-fh1wv.5` — Rewrite Deployment Docs Around Incus Primary Path

### What

Update public/operator docs, README references, generated help, and agent instructions so Incus is the primary deployment path, Docker is explicit/non-primary, and manual post-provision login/TUN requirements are visible.

### Context

The repo currently documents host service as the normal local/node-a runtime and Docker as a supported prod-like smoke path. This issue changes the supported self-hosted substrate to an Incus system container, while retaining host-service development shortcuts and explicit Docker smoke where needed.

### Locked Decisions

- Lead with amd64 Debian 13 Incus system container.
- Document `/dev/net/tun` as required device passthrough for in-container Tailscale.
- Document manual agent CLI login steps in bootstrap output and docs, not just a wiki page.
- State that Docker is no longer the primary gateway deployment shape; keep explicit dev-container/prod-like smoke instructions only where still supported.
- Update docs against the actual CLI surface after implementation.

### Discretion

- Exact doc layout under `docs/runtime/`, `docs/deploy/`, or README anchors.
- Whether Docker compose files are marked deprecated in comments or moved to clearly named smoke paths.

### Required Doc Split

Docs must clearly distinguish three surfaces:

1. Local development host-service shortcut.
2. Primary self-hosted Incus system-container path.
3. Explicit Docker smoke/dev-container compatibility path.

Do not mix these into one deployment story.

### Required Post-Provision Checklist

Bootstrap output and docs should share the same checklist:

1. Verify service.
2. Verify tailnet IP when Tailscale is enabled.
3. Run `claude login`, `codex login`, and `gemini` login/setup.
4. Verify gateway readiness.

### Testing

- [ ] Generated CLI help refreshed if setup commands changed.
- [ ] Markdown links checked for changed docs.
- [ ] README no longer implies Docker is the default production/self-host path.
- [ ] `CLAUDE.md` updates preserve source-of-truth symlink rules if memory files are edited.

### Validation

- [ ] A new operator can follow docs from Incus install/prep through running gateway.
- [ ] Docs include rollback and verification commands.
- [ ] Docs explicitly call out amd64-only and Code Mode/QuickJS reason.
- [ ] Docs should not declare Docker obsolete before Incus smoke is repeatable and rollback is documented.

### Files

- `README.md`
- `CLAUDE.md`
- `docs/README.md`
- `docs/runtime/HOST_GATEWAY.md` or new Incus deployment doc
- `docs/generated/*`
- `docker-compose.yml`
- `docker-compose.prod.yml`

### Dependencies

- Depends on Work Item 3.

### Notes from Research and Review

- Keep Docker preflight/migration guard until Docker docs are actually retired.
- Avoid stale generated help by refreshing only after CLI flags stabilize.

## Work Item 6 / `lab-fh1wv.6` — Optional Incus Distrobuilder Release Image

### What

Add the optional distrobuilder image flow after the provisioning path is stable. This is a convenience accelerator, not the foundation.

### Context

The universal path is install script plus `labby setup --provision` inside a Debian 13 container or bare-metal host. The image should call the same provisioning dependency routine so a baked image and freshly provisioned box converge.

### Locked Decisions

- amd64-only image.
- Build on self-hosted Controller runner because distrobuilder needs privileged/overlayfs behavior that hosted runners fight.
- Upload unified Incus image tarball as a GitHub Release asset.
- Image cadence can be weekly or dependency-bump driven; do not require every Labby release to rebuild the base image.
- No second source of truth for packages; reuse provision dependency logic.

### Discretion

- Exact `labby-base.yaml` layout and release asset naming.
- Whether first pass is manual workflow dispatch before scheduled builds.

### Testing

- [ ] Distrobuilder config validates locally/on runner.
- [ ] Imported release asset boots and converges with `labby setup --provision` behavior.
- [ ] Release workflow does not block normal binary release if image build is skipped or manually gated.

### Validation

- [ ] `incus image import` of the release asset yields a usable base image.
- [ ] First boot or first provision installs/runs current `labby` consistently.

### Files

- `.github/workflows/release.yml`
- `packaging/incus/labby-base.yaml` or equivalent
- `scripts/incus-bootstrap.sh` if it supports imported image path
- `docs/runtime/*`

### Dependencies

- Depends on Work Item 2. Defer until the system unit, provision command, and bootstrap path are proven.

### Notes from Research and Review

- Distrobuilder must remain downstream of provisioning.
- Do not let image package lists diverge from `setup --provision`.
- It is acceptable for this to be weekly or manually dispatched rather than tied to every binary release.

## Build Order

1. Harden `unit_text()` and convert host service to default system unit with `User=labby` in `host_service.rs`.
2. Add `labby setup --provision` with bounded floor install, consent, dry-run, `--yes`, and `--skip-deps`.
3. Add `scripts/incus-bootstrap.sh` with Incus launch, TUN passthrough, in-box install/provision, and post-provision login output.
4. Update docs around amd64 Debian 13 Incus primary substrate, TUN, and manual agent CLI login steps.
5. Add just-in-time dependency diagnostics.
6. Add optional distrobuilder image only once cold-start time actually matters.

## Acceptance Criteria

- [ ] `incus launch images:debian/13 labby` -> `scripts/incus-bootstrap.sh` -> running gateway with zero Docker bind mounts.
- [ ] Service survives `incus restart labby` with no linger and no user D-Bus requirement.
- [ ] `labby setup --provision --dry-run` prints a complete bounded plan and mutates nothing.
- [ ] Re-running `labby setup --provision --yes` is a no-op on an already provisioned box except status verification.
- [ ] Tailscale comes up inside the container with its own tailnet IP when `TS_AUTHKEY` is supplied and `/dev/net/tun` is present.
- [ ] systemd hardening directives are applied and all existing stdio upstreams still spawn.
- [ ] At least one `npx` and one `uvx` upstream smoke successfully under the hardened service.
- [ ] Adding an upstream whose leaf dependency is missing surfaces a redacted stderr/fix hint in the add/import or doctor response.
- [ ] Operator-visible diagnostics are redacted for secrets, tokens, auth headers, Tailscale auth keys, OAuth codes, and sensitive paths.
- [ ] Bootstrap install path fails closed on missing checksum/unpinned release unless source-build fallback is explicitly requested.
- [ ] Bootstrap is idempotent across partial failures.
- [ ] Docs clearly state amd64 Debian 13 Incus as the supported substrate, Docker's reduced role, TUN requirements, rollback path, and manual login steps.
- [ ] All changes validate with the all-features build/test/lint path appropriate to touched code.
- [ ] Optional distrobuilder image, if implemented, converges with fresh `setup --provision` behavior.

## Open Questions

- Keep `systemd --user` path as a non-default workstation escape hatch, or drop entirely?
- Prune Docker `labby-master` detection once Docker is unsupported, or keep as a migration courtesy?
- Start v1 dependency hints with a tiny manifest plus stderr diagnosis, or diagnosis-only first?
- Choose NodeSource apt vs official static tarball for Node.
- Decide whether Tailscale installation/join lives in `setup --provision` or the bootstrap script; keep one owner.
