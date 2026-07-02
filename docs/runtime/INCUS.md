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
- unprivileged container with nesting disabled
- `/dev/net/tun` passthrough when Tailscale is enabled, validated by creating
  a throwaway TUN interface during bootstrap

The amd64 release-path constraint exists because the release binary includes
Code Mode's QuickJS engine (`rquickjs-sys`), which does not cross-compile
cleanly in the current release path. ARM hosts can build from source or push a
local binary, but they should not expect the same prebuilt cold-start path.

## Deployment Choices

Use Incus for normal self-hosting:

```bash
curl -fsSL https://raw.githubusercontent.com/jmagar/labby/main/scripts/install.sh | sh
labby setup
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

Install and initialize Incus explicitly on the host first. The bootstrap command
does not install or initialize Incus:

```bash
sudo apt install incus
sudo incus admin init
```

Host networking and storage still matter. If containers cannot reach the
network, check the host bridge/NAT rules and Docker's FORWARD/NAT policy before
debugging Labby itself.

The bootstrap can use a ZFS, Btrfs, or dir-backed Incus storage pool. By default
it creates a dedicated ZFS pool named `labby-zfs`; set
`LABBY_INCUS_STORAGE_DRIVER=btrfs` or pass `--storage-driver btrfs` for a Btrfs
pool named `labby-btrfs`, and use `dir` for the simplest fallback pool named
`labby-dir`. Override the pool name with `--storage-pool`, the storage source
with `--storage-source`, or the legacy ZFS dataset source with
`LABBY_INCUS_ZFS_SOURCE` / `--zfs-source`.

## Bootstrap

Install Labby, then run the host-side Incus bootstrap:

```bash
curl -fsSL https://raw.githubusercontent.com/jmagar/labby/main/scripts/install.sh | sh
labby setup
```

The declarative Incus shape lives in
`config/incus/labby-gateway-profile.yaml`, and the default snapshot policy lives
in `config/incus/labby-backup.yaml`. `labby setup` embeds those vetted artifacts
in the binary, materializes them into a temporary workspace, and runs the same
host bootstrap logic from there. The bootstrap creates or updates the profile,
launches `images:ubuntu/24.04` with it, then applies the snapshot policy with
Incus instance config. The profile owns
`security.privileged=false`, `security.nesting=false`, a root disk on the
selected storage pool, `/dev/net/tun` access through a raw LXC bind mount, and
an AppArmor signal peer rule required by newer Ubuntu hosts so systemd can stop
services inside the unprivileged container cleanly.

Existing containers are idempotently converged too. If an existing container's
root disk already comes from a different Incus storage pool, the bootstrap
derives a rootless runtime profile from the same YAML and attaches that instead
of trying to replace the immutable root disk. The derived profile defaults to
`labby-gateway-runtime` and can be renamed with `--runtime-profile-name`.

The script is idempotent. It creates or reuses the `labby` container, validates
that the container is amd64 Ubuntu 24.04, verifies the expanded profile-provided
TUN device, installs `/usr/local/bin/labby`, then runs:

```bash
incus exec labby -- labby setup --provision --yes
```

Override the snapshot policy with `--backup-config PATH` or
`LABBY_INCUS_BACKUP_CONFIG=PATH`. Disable policy application with
`--no-backup-config`. The backup YAML maps directly to Incus `snapshots.*`
instance config keys, so Incus owns scheduling and expiry; Labby does not run a
cron or timer for normal snapshot retention. Bootstrap prefers the Rust-backed
`labby setup incusbackup apply --name <container> --config <path>` validator
when a new enough host `labby` is on `PATH`, and falls back to the constrained
shell parser only for older hosts.

Bootstrap does not migrate host Labby config, copy arbitrary local MCP
artifacts, bind-mount host workspaces, or rewrite `config.toml`. Incus is the
primary deployment boundary, so the supported runtime shape is a durable system
container that owns its own `/home/labby/.labby` state. For an existing single-user
host setup, seed `/home/labby/.labby` once, fix any host-specific paths once, then
preserve that container with Incus snapshots/backups.

The web app also serves the installer at `https://labby.tootie.tv/install.sh`
for convenience. The canonical pipe-to-shell source remains the GitHub-hosted
script at
`https://raw.githubusercontent.com/jmagar/labby/main/scripts/install.sh`.

For PR validation before a release exists, push a local binary instead:

```bash
cargo build --workspace --all-features --bin labby
target/debug/labby incus setup --local-binary target/debug/labby
```

By default, `labby setup` installs the latest Labby release. Use the explicit
`labby incus setup --version vX.Y.Z` form when you need reproducibility, or set
`LAB_INSTALL_VERSION` for the checkout-local bootstrap script.

The checkout-local `scripts/incus-bootstrap.sh` remains available for
contributor debugging and CI image smoke tests, but the supported operator entry
point is the binary-owned `labby setup` command. The explicit
`labby incus setup` subcommand owns advanced bootstrap flags such as
`--local-binary`, `--skip-install`, and storage overrides. For day-to-day local
binary deploys into an existing container, use `labby incus sync`.

The distrobuilder image definition lives at `config/incus/labby-image.yaml`.
Release CI builds it as a prebuilt Incus container image:
`labby-incus-x86_64-unknown-linux-gnu.tar.xz` plus a `.sha256` file. Import it
locally and launch it with the normal profile/provision converger:

```bash
sha256sum -c labby-incus-x86_64-unknown-linux-gnu.tar.xz.sha256
incus image import labby-incus-x86_64-unknown-linux-gnu.tar.xz \
  --alias labby-gateway-vX.Y.Z
scripts/incus-bootstrap.sh \
  --image local:labby-gateway-vX.Y.Z \
  --skip-install
```

The image bakes in the release `labby` binary, the bounded apt floor, and the
agent runtime/toolchain floor: Node, uv-managed Python, Rust, Go, Claude Code,
Codex, Gemini CLI, ffmpeg, Android platform tooling (`adb`, Android SDK platform
tools, and build tools), and the Tailscale client. `config/incus/labby-image.yaml`
is the source of truth for both the apt package list and the named provisioning
action scripts; bare-metal `labby setup --provision` derives its install and
verification steps from the same YAML so image builds and non-image provisioning
do not drift. The image does not bake secrets, Tailscale auth, OAuth/login state,
operator config, or tailnet join state; those remain runtime state owned by the
container after the one-time seed. The image build script explicitly strips
common secret environment variables before invoking distrobuilder, and the CI
smoke test fails if the exported image contains Labby env files, Tailscale
state/authkey files, or common secret env vars.

Release archives are currently published for amd64 Linux. On arm64 hosts, use
`--local-binary` with a locally built `labby` binary, or opt into the slower
source build fallback with `--allow-source-fallback` / `LAB_ALLOW_SOURCE_FALLBACK=1`.

## Golden Snapshots

ZFS- and Btrfs-backed Incus storage make configured golden containers cheap to
snapshot and clone. After a successful provision run:

```bash
incus stop labby-golden
incus snapshot create labby-golden configured-v1
incus copy labby-golden/configured-v1 labby-test-1
```

Do not start multiple clones that carry the same Tailscale machine state at the
same time. For parallel clone testing, reset and rejoin Tailscale in each clone
with a fresh ephemeral key before running networked checks.

Configured gateway state is intentionally inside the Incus container, not a host
bind mount. Use snapshots and normal Incus backup/export workflows for rollback
and recovery. ZFS and Btrfs are the preferred storage drivers for cheap
copy-on-write snapshots and clones; the dir driver is useful as a universal
fallback but does not provide the same storage-level efficiency. Deleting a
container deletes that container filesystem unless you first snapshot, copy, or
export it.

## Tailscale

Tailscale runs inside the container and gets its own tailnet identity. `/dev/net/tun`
passthrough is required.

To join during bootstrap, provide an ephemeral, preauthorized, tag-scoped auth
key:

```bash
TS_AUTHKEY=tskey-... scripts/incus-bootstrap.sh --version vX.Y.Z
```

With the binary-owned bootstrap:

```bash
TS_AUTHKEY=tskey-... labby setup
```

The same `TS_AUTHKEY` variable is honored by `labby setup --provision --yes`
for bare-metal or already-running container provisioning.

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

- apt install of the bounded floor derived from `config/incus/labby-image.yaml`,
  including core CLI/runtime packages plus `ffmpeg`, `adb`, and Android SDK
  command-line tooling
- `lab` user creation
- writing `/etc/systemd/system/labby.service`
- enabling and restarting `labby.service`

User-space actions run as `lab` and install:

- Node, including `node`, `npm`, and `npx`
- `uv`, `uvx`, and a managed Python exposed as `python` and `python3`
- Rust and Go
- `claude`, `codex`, and `gemini`
- Tailscale when not already installed

Provisioning does not install or initialize Incus or expose root
package/user/systemd mutation through MCP, HTTP, Code Mode, or remote admin
actions.

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
User=labby
Group=labby
ExecStart=/usr/local/bin/labby serve
WorkingDirectory=/home/labby
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

For the first cutover from an existing host-native setup, copy the current
Labby state into the container manually:

```bash
incus exec labby -- install -d -m 0700 -o labby -g labby /home/labby/.labby
incus file push ~/.labby/.env labby/home/labby/.labby/.env
incus file push ~/.labby/config.toml labby/home/labby/.labby/config.toml
incus exec labby -- chown labby:labby /home/labby/.labby/.env /home/labby/.labby/config.toml
incus exec labby -- chmod 600 /home/labby/.labby/.env /home/labby/.labby/config.toml
incus exec labby -- systemctl restart labby
incus exec labby -- curl -fsS http://127.0.0.1:8765/ready
```

That is an operator cutover step, not bootstrap behavior. If copied config
contains host-only paths such as `/home/jmagar/...`, update them once to
container-local paths or reinstall those MCP servers inside the `lab` account.

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

The runtime floor covers `npx`, `uvx`, `python`, `ssh`, `ffmpeg`, `adb`, and
the baked agent toolchains. Missing additional leaf dependencies are diagnosed
from the existing bounded upstream stderr/health path and reported as redacted
hints instead of being installed automatically by the gateway runtime.
