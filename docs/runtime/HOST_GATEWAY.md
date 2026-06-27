# Labby Gateway Runtime

The primary supported self-hosted Labby gateway runtime is an **amd64 Ubuntu 24.04
Incus system container**. Labby launches stdio MCP servers and agent CLIs at
runtime, so the deployment needs a persistent system environment with normal
package installation, systemd, SSH, and user caches. Docker remains useful for
explicit development and image-smoke work, but it is no longer the default
self-hosting boundary.

The supported substrate is currently amd64 because the release binary includes
Code Mode's QuickJS engine (`rquickjs-sys`), which does not cross-compile
cleanly in the current release path. ARM hosts can still build from source, but
they should not expect the same prebuilt cold-start path.

## Primary: Incus System Container

Install and initialize Incus explicitly on the host first. The bootstrap script
does not install or initialize Incus for you:

```bash
sudo apt install incus
sudo incus admin init
```

Then launch and provision the gateway container with a pinned Labby release:

```bash
scripts/incus-bootstrap.sh --version vX.Y.Z
```

The bootstrap is idempotent. It creates or reuses the `labby` container, launches
`images:ubuntu/24.04`, configures a privileged non-nesting system container,
passes exactly one host device (`/dev/net/tun`), installs
`/usr/local/bin/labby`, and runs:

```bash
incus exec labby -- labby setup --provision --yes
```

`/dev/net/tun` is required when Tailscale runs inside the container. To join the
tailnet during bootstrap, provide an ephemeral, preauthorized, tag-scoped auth
key:

```bash
TS_AUTHKEY=tskey-... scripts/incus-bootstrap.sh --version vX.Y.Z
```

Add `--tailscale-ssh` only when you intentionally want Tailscale SSH enabled for
the container. Tailscale SSH is governed by tailnet ACLs; enabling it changes who
can reach the container over SSH.

The container is privileged because this host's unprivileged Incus containers
deny signal delivery even for same-UID processes. `systemctl restart
labby.service` needs normal signal semantics; without them, a stale Labby
process can keep answering `/ready` while systemd reports the unit as failed.
The service itself still runs as the unprivileged `lab` user inside the
container.

For local PR validation before a release exists, push a built checkout binary
instead of downloading a GitHub release:

```bash
cargo build --workspace --all-features --bin labby
scripts/incus-bootstrap.sh --local-binary target/debug/labby
```

The release path should still use `--version vX.Y.Z`.

## In-Box Provisioning

Inside the container, `labby setup --provision` owns the bounded environment
floor:

```bash
labby setup --provision --dry-run
labby setup --provision
labby setup --provision --yes
labby setup --provision --yes --skip-deps
```

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

The unit runs:

```text
User=lab
Group=lab
ExecStart=/usr/local/bin/labby serve
WorkingDirectory=/home/lab
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
