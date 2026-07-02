# Labby Gateway Runtime

The recommended self-hosted Labby gateway deployment is the **amd64 Ubuntu 24.04
Incus system container** described in [INCUS.md](./INCUS.md). Bare metal is the
secondary supported shape when Labby owns a dedicated host or VM. Docker remains
available for development, compatibility, and image-smoke work, but it is no
longer the recommended self-host boundary.

Labby launches stdio MCP servers and agent CLIs at runtime, so the gateway needs
a persistent system environment with normal package installation, systemd, SSH,
user caches, and runtime-managed tools. Incus provides that shape without
running Labby directly on the host.

## Recommended Paths

| Path | Use when | Entry point |
|------|----------|-------------|
| Incus system container | Normal self-hosted gateway deployment | `scripts/incus-bootstrap.sh --version vX.Y.Z` |
| Bare metal / dedicated VM | The host itself is the gateway appliance | `labby setup --provision --yes` |
| Docker | Development, compatibility, image smoke, ACP adapter work | `just dev-container` / `just dev-container-debug` |

See [INCUS.md](./INCUS.md) for the full Incus runbook, bare-metal variant,
Tailscale setup, rollback, and dependency diagnostics.

## In-Box Provisioning

Inside the container, `labby setup --provision` owns the bounded environment
floor:

```bash
labby setup --provision --dry-run
labby setup --provision
labby setup --provision --yes
labby setup --provision --yes --skip-deps
```

To auto-join the gateway to Tailscale during provisioning, provide an
ephemeral, preauthorized auth key:

```bash
TS_AUTHKEY=tskey-auth-... labby setup --provision --yes
```

The key is passed to `tailscale up` through a root-only runtime file and removed
after the join. Leave `TS_AUTHKEY` unset to skip Tailscale.

The plan is explicit about privilege. Root actions are limited to the apt
package floor (`git`, `openssh-client`, `gh`, `ca-certificates`, `curl`,
`xz-utils`, `zsh`), `lab` user creation, writing
`/etc/systemd/system/labby.service`, and enabling/restarting the service.
User-space actions run as `lab` and install Node v24.x, `uv` plus Python,
`claude`, `codex`, and `gemini`.

Provisioning does not install or initialize Incus, silently install leaf
packages such as `ffmpeg`, or expose root package/user/systemd mutation through
MCP, HTTP, Code Mode, or remote admin actions.

Supply-chain trust is intentionally explicit: the Labby release install path
requires the GitHub release checksum, and Node downloads are verified against
the upstream SHA256 manifest. `uv`, Tailscale, and the agent CLIs still trust
their upstream installer/package channels (`astral.sh`, `tailscale.com`, and
npm) during provisioning. Use this path only when those upstreams are acceptable
for the container boundary; otherwise pre-bake those runtimes into a controlled
image and run `labby setup --provision --yes --skip-deps`.

## System Service

The default service is a hardened system unit:

```bash
labby setup host-service unit
labby setup host-service install --install-self -y
systemctl status labby --no-pager
```

`host-service install` writes or updates `/etc/systemd/system/labby.service`,
installs the current binary when `--install-self` is provided, enables the unit,
and restarts the service.

The unit runs:

```text
User=labby
Group=labby
ExecStart=/usr/local/bin/labby serve
WorkingDirectory=/home/labby
WantedBy=multi-user.target
```

It also applies the hardening baseline from the implementation plan, including
`ProtectSystem=strict`, `NoNewPrivileges=true`, `PrivateTmp=true`, restricted
address families, and explicit `ReadWritePaths` for the `lab` user's runtime
state.

Readiness verification requires both an active `labby.service` unit and a
successful loopback `/ready` response. This prevents stale processes from
masking failed service restarts.

## Post-Provision Checklist

After bootstrap or provisioning:

```bash
incus exec labby -- systemctl status labby --no-pager
incus exec labby -- curl -fsS http://127.0.0.1:8765/ready
incus exec labby -- su - lab
```

Then run the interactive agent logins inside that `lab` shell:

```bash
claude login
codex login
gemini
```

When Tailscale is enabled:

```bash
incus exec labby -- tailscale ip -4
```

## Local Development Shortcut

For source checkouts, a local host service can still be useful while iterating,
but it now uses the same system-unit semantics as the Incus runtime. Build and
install the binary into `/usr/local/bin`, then restart the service:

```bash
cargo build --workspace --all-features --bin labby
sudo install -D -m 755 target/debug/labby /usr/local/bin/labby
sudo labby setup host-service restart -y
```

Do not rely on `systemd --user`, linger, or `XDG_RUNTIME_DIR` for the supported
self-hosted runtime.

## Explicit Docker Smoke Path

Docker remains an explicit compatibility and development-image smoke path:

```bash
just dev-container
just dev-container-debug
```

Stop Docker before starting the system service because both runtimes bind the
configured Labby HTTP port:

```bash
docker compose -f docker-compose.yml stop labby-master
labby setup host-service install --install-self -y
```

## Rollback

Rollback from the Incus path by stopping or deleting the container:

```bash
incus stop labby
incus delete labby
```

Rollback to Docker for a compatibility smoke only:

```bash
systemctl disable --now labby.service
docker compose -f docker-compose.yml up -d labby-master --no-deps
curl -fsS http://127.0.0.1:8765/ready
```

## Dependency Diagnostics

The gateway runtime floor covers `npx`, `uvx`, `python`, and `ssh`. Missing leaf
dependencies are diagnosed from the existing bounded upstream stderr/health path
and reported as redacted hints. For example, an upstream that fails with
`ffmpeg: command not found` reports an explicit `sudo apt install ffmpeg` hint,
but Labby does not run that command automatically.
