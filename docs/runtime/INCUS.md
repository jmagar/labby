# Incus Gateway Deployment

Incus is the recommended self-hosted Labby gateway deployment. Bare metal is the
secondary supported shape when you already want Labby to own a whole host or VM.
Docker is retained for development, compatibility, and image-smoke work; it is
not the recommended self-host boundary for the agent gateway.

Labby launches stdio MCP servers and agent CLIs at runtime. That workload needs
a persistent system environment with normal package installation, systemd, SSH,
user caches, and runtime-managed tools. Incus gives Labby that shape while still
keeping the gateway inside a container boundary.

The supported Incus substrate is currently:

- amd64 / x86_64 for the normal release install path
- arm64 / aarch64 only with `--local-binary` or `--allow-source-fallback`
- Ubuntu 24.04 (`images:ubuntu/24.04`)
- Incus system container
- `config/incus/labby-gateway-profile.yaml` applied as the `labby-gateway`
  Incus profile
- privileged container with nesting disabled and AppArmor unconfined for the
  gateway workload
- `/dev/net/tun` passthrough when Tailscale is enabled

The amd64 release-path constraint exists because the release binary includes
Code Mode's QuickJS engine (`rquickjs-sys`), which does not cross-compile
cleanly in the current release path. ARM hosts can build from source or push a
local binary, but they should not expect the same prebuilt cold-start path.

## Deployment Choices

Use Incus for normal self-hosting:

```bash
scripts/incus-bootstrap.sh --version vX.Y.Z
```

Use bare metal when the host itself is the gateway appliance:

```bash
sudo install -D -m 755 target/release/labby /usr/local/bin/labby
sudo labby setup --provision --yes
```

Use Docker only for explicit development or image smoke:

```bash
just dev-container
just dev-container-debug
```

## Host Preparation

Install and initialize Incus explicitly on the host first. The bootstrap script
does not install or initialize Incus:

```bash
sudo apt install incus
sudo incus admin init
```

Host networking and storage still matter. On node-a, live testing required
`devices=on` for the backing ZFS dataset and explicit forwarding/NAT rules for
`incusbr0` because Docker's FORWARD/NAT policy blocked container outbound
networking.

## Bootstrap

Run the bootstrap from a checkout with a pinned release tag:

```bash
scripts/incus-bootstrap.sh --version vX.Y.Z
```

The declarative Incus shape lives in
`config/incus/labby-gateway-profile.yaml`. The bootstrap script creates or
updates that profile, launches `images:ubuntu/24.04` with it, and attaches it to
an existing container when needed. The profile owns `security.privileged=true`,
`security.nesting=false`, AppArmor unconfined via `raw.lxc`, and the
`/dev/net/tun` `unix-char` passthrough.

The script is idempotent. It creates or reuses the `labby` container, validates
that the container is amd64 Ubuntu 24.04, verifies the expanded profile-provided
TUN device, installs `/usr/local/bin/labby`, then runs:

```bash
incus exec labby -- labby setup --provision --yes
```

For PR validation before a release exists, push a local binary instead:

```bash
cargo build --workspace --all-features --bin labby
scripts/incus-bootstrap.sh --local-binary target/debug/labby
```

The release path should still use `--version vX.Y.Z`.

Release archives are currently published for amd64 Linux. On arm64 hosts, use
`--local-binary` with a locally built `labby` binary, or opt into the slower
source build fallback with `--allow-source-fallback` / `LAB_ALLOW_SOURCE_FALLBACK=1`.

## Tailscale

Tailscale runs inside the container and gets its own tailnet identity. `/dev/net/tun`
passthrough is required.

To join during bootstrap, provide an ephemeral, preauthorized, tag-scoped auth
key:

```bash
TS_AUTHKEY=tskey-... scripts/incus-bootstrap.sh --version vX.Y.Z
```

Add `--tailscale-ssh` only when you intentionally want Tailscale SSH enabled for
the container. Tailscale SSH is governed by tailnet ACLs; enabling it changes
who can reach the container over SSH.

## In-Box Provisioning

`labby setup --provision` is the in-box converger for both Incus and bare metal:

```bash
labby setup --provision --dry-run
labby setup --provision
labby setup --provision --yes
labby setup --provision --yes --skip-deps
```

The plan is explicit about privilege. Root actions are limited to:

- apt install of the bounded floor: `git`, `openssh-client`, `gh`,
  `ca-certificates`, `curl`, `xz-utils`, `zsh`
- `lab` user creation
- writing `/etc/systemd/system/labby.service`
- enabling and restarting `labby.service`

User-space actions run as `lab` and install:

- Node v24.x, including `node`, `npm`, and `npx`
- `uv`, `uvx`, and a managed Python exposed as `python` and `python3`
- `claude`, `codex`, and `gemini`

Provisioning does not install or initialize Incus, silently install leaf
packages such as `ffmpeg`, or expose root package/user/systemd mutation through
MCP, HTTP, Code Mode, or remote admin actions.

Supply-chain trust is intentionally explicit: the Labby release install path
requires the GitHub release checksum, and Node downloads are verified against
the upstream SHA256 manifest. `uv`, Tailscale, and the agent CLIs still trust
their upstream installer/package channels during provisioning. If that is too
broad for your environment, pre-bake those runtimes into a controlled image and
run:

```bash
labby setup --provision --yes --skip-deps
```

## System Service

The converged service is a hardened system unit:

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

It also applies hardening such as `ProtectSystem=strict`,
`NoNewPrivileges=true`, `PrivateTmp=true`, restricted address families, and
explicit `ReadWritePaths` for the `lab` user's runtime state.

Readiness requires both an active `labby.service` unit and a successful loopback
`/ready` response. This prevents stale processes from masking failed service
restarts.

## Post-Provision Checklist

After bootstrap or provisioning:

```bash
incus exec labby -- systemctl status labby --no-pager
incus exec labby -- curl -fsS http://127.0.0.1:8765/ready
incus exec labby -- su - lab
```

Run interactive agent setup inside that `lab` shell:

```bash
claude login
codex login
gemini
```

When Tailscale is enabled:

```bash
incus exec labby -- tailscale ip -4
```

## Bare-Metal Variant

Bare metal uses the same in-box provisioning and system unit without the Incus
container boundary. It is appropriate for a dedicated gateway VM or host.

```bash
sudo install -D -m 755 target/release/labby /usr/local/bin/labby
sudo labby setup --provision --yes
sudo systemctl status labby --no-pager
curl -fsS http://127.0.0.1:8765/ready
```

Use the same manual `lab` user agent logins and the same dependency diagnostic
model. Do not use the older `systemd --user`, linger, or `XDG_RUNTIME_DIR`
runtime as the recommended self-host path.

## Rollback

Rollback from Incus by stopping or deleting the container:

```bash
incus stop labby
incus delete labby
```

Rollback from bare metal:

```bash
sudo systemctl disable --now labby.service
sudo rm -f /etc/systemd/system/labby.service
sudo systemctl daemon-reload
```

Docker can still be used for compatibility smoke:

```bash
docker compose -f docker-compose.yml up -d labby-master --no-deps
curl -fsS http://127.0.0.1:8765/ready
```

## Dependency Diagnostics

The runtime floor covers `npx`, `uvx`, `python`, and `ssh`. Missing leaf
dependencies are diagnosed from the existing bounded upstream stderr/health path
and reported as redacted hints. For example, an upstream that fails with
`ffmpeg: command not found` reports an explicit `sudo apt install ffmpeg` hint,
but Labby does not run that command automatically.
