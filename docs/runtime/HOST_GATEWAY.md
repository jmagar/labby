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
port `8765`:

```bash
docker compose -f docker-compose.yml stop labby-master
just host-service-install
curl -fsS http://127.0.0.1:8765/ready
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
pid=$(systemctl --user show labby.service --property=MainPID --value)
readlink "/proc/$pid/exe"
docker inspect -f '{{.State.Running}}' labby-master 2>/dev/null || true
```

Expected: `/proc/$pid/exe` points at `/home/jmagar/.local/bin/labby`, and the
Docker container is not the process answering the public route.

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
pid=$(pgrep -u "$USER" -f 'labby serve' | head -n1)
readlink "/proc/$pid/exe"
```

If the path ends in `(deleted)`, restart the service:

```bash
systemctl --user restart labby.service
```
