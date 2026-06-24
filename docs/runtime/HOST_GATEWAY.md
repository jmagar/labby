# Host Labby Gateway

The preferred Labby gateway runtime is a host user service:

```bash
~/.local/bin/labby serve
```

It runs as `labby.service` under `systemd --user`. Bind host, port, auth, and
upstream gateway configuration continue to come from `~/.lab/.env` and Labby
config. Do not bake public bind settings into the systemd unit.

## Install

```bash
just host-service-install
systemctl --user --no-pager --full status labby.service
```

## Migrate From The Docker Dev Container

Stop the container before starting the host service because both runtimes bind
the configured local MCP HTTP port:

```bash
docker compose -f docker-compose.yml stop labby-master
just host-service-install
labby setup host-service status --json
labby gateway list
```

## Update The Running Host Gateway

```bash
just host-sync
labby setup host-service status --json
labby gateway code exec --json --code 'async () => 1'
```

## Verify The Public MCP Route

```bash
set -a
. ~/.lab/.env
set +a
TOKEN="$LAB_MCP_HTTP_TOKEN"
mcporter list https://lab.tootie.tv/mcp \
  --header Authorization="Bearer $TOKEN" \
  --status \
  --exit-code
```

Then call Code Mode through the same public MCP route:

```bash
mcporter call \
  --http-url https://lab.tootie.tv/mcp \
  --header Authorization="Bearer $TOKEN" \
  --tool codemode \
  --args '{"code":"async () => 1"}' \
  --output json
```

Expected result includes:

```json
{"result":1}
```

Also prove the public route is backed by the host service:

```bash
host_pid=$(systemctl --user show labby.service --property=MainPID --value)
public_pid=$(curl -fsS https://lab.tootie.tv/health | jq -r .pid)
test "$public_pid" = "$host_pid"
readlink "/proc/$host_pid/exe"
docker inspect -f '{{.State.Running}}' labby-master
```

Expected: the public health PID matches `labby.service` `MainPID`,
`/proc/$host_pid/exe` points at `/home/jmagar/.local/bin/labby`, and the Docker
container reports `false`.

## Roll Back To Docker

```bash
systemctl --user disable --now labby.service
docker compose -f docker-compose.yml up -d labby-master --no-deps
curl -fsS http://127.0.0.1:8765/ready
```

## Known Failure Mode: Deleted Executable

If Code Mode reports `failed to spawn Code Mode runner` after replacing a
running binary, check:

```bash
labby setup host-service status --json | jq -r .process_exe
```

If the path ends in `(deleted)`, restart the service:

```bash
systemctl --user restart labby.service
```

Advanced operators can point Code Mode at an alternate validated Labby binary
before restarting the service. The path must be absolute, executable, owned by
the current user or root, and not group/world-writable:

```bash
install -D -m 755 target/release-fast/labby ~/.local/bin/labby.next
env_file="$HOME/.lab/.env"
tmp="$(mktemp)"
grep -v '^LAB_CODE_MODE_RUNNER_EXE=' "$env_file" > "$tmp"
printf 'LAB_CODE_MODE_RUNNER_EXE=%s\n' "$HOME/.local/bin/labby.next" >> "$tmp"
install -m 600 "$tmp" "$env_file"
rm -f "$tmp"
systemctl --user restart labby.service
labby gateway code exec --json --code 'async () => 1'
```

Remove `LAB_CODE_MODE_RUNNER_EXE` from `~/.lab/.env` after the normal
`~/.local/bin/labby` path is healthy again:

```bash
sed -i '/^LAB_CODE_MODE_RUNNER_EXE=/d' ~/.lab/.env
systemctl --user restart labby.service
```
