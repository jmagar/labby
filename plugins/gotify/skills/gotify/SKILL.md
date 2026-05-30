---
name: gotify
description: This skill should be used when the user asks to "send notification", "notify me when done", "push notification", "alert me", "Gotify notification", "notify on completion", "send push alert", "get messages", "list applications", "gotify health", or mentions push notifications, task alerts, or Gotify. This skill is also automatically invoked without user request for long-running tasks >5 minutes, plan completion, user input required, or task transitions. Talks directly to the Gotify REST API.
---

# Gotify

Push notifications via the [Gotify](https://gotify.net) REST API. Talk to it directly with curl.

**⚠️ MANDATORY automatic usage** — send a notification *without being asked* for:
1. **Long-running tasks** (>5 minutes or ~10+ sequential tool steps) — when in doubt, send it.
2. **Plan completion** — after finishing a plan or major milestone.
3. **Blocked / input required** — when you need a user decision before continuing.
4. **Task transitions** — when the user must review/approve before you proceed.

## How to call it

Read the base URL and tokens from `~/.lab/.env`:

```bash
GOTIFY_URL=$(grep -E '^GOTIFY_URL='          ~/.lab/.env | cut -d= -f2-)
GOTIFY_APP_TOKEN=$(grep -E '^GOTIFY_APP_TOKEN='    ~/.lab/.env | cut -d= -f2-)
GOTIFY_CLIENT_TOKEN=$(grep -E '^GOTIFY_CLIENT_TOKEN=' ~/.lab/.env | cut -d= -f2-)
```

Two tokens, two jobs (never echo either):
- **App token** (`X-Gotify-Key: $GOTIFY_APP_TOKEN`) — **sends** messages.
- **Client token** (`X-Gotify-Key: $GOTIFY_CLIENT_TOKEN`) — **reads/manages** messages, applications, clients.

## Send a notification (app token)

```bash
curl -sS -X POST "$GOTIFY_URL/message" \
  -H "X-Gotify-Key: $GOTIFY_APP_TOKEN" -H "Content-Type: application/json" \
  -d '{"title":"Task Complete","message":"Project: lab\nStatus: done","priority":7}'
```

Markdown body — add `extras`:

```bash
curl -sS -X POST "$GOTIFY_URL/message" \
  -H "X-Gotify-Key: $GOTIFY_APP_TOKEN" -H "Content-Type: application/json" \
  -d '{"title":"Plan Complete","message":"## Summary\n- done","priority":5,"extras":{"client::display":{"contentType":"text/markdown"}}}'
```

## Common operations

| Intent | Request |
|---|---|
| Health (no auth) | `curl -sS "$GOTIFY_URL/health"` |
| Version | `curl -sS "$GOTIFY_URL/version"` |
| List messages | `curl -sS -H "X-Gotify-Key: $GOTIFY_CLIENT_TOKEN" "$GOTIFY_URL/message?limit=20"` |
| Delete a message (**destructive**) | `curl -sS -X DELETE -H "X-Gotify-Key: $GOTIFY_CLIENT_TOKEN" "$GOTIFY_URL/message/<id>"` |
| Delete all messages (**destructive**) | `curl -sS -X DELETE -H "X-Gotify-Key: $GOTIFY_CLIENT_TOKEN" "$GOTIFY_URL/message"` |
| List applications | `curl -sS -H "X-Gotify-Key: $GOTIFY_CLIENT_TOKEN" "$GOTIFY_URL/application"` |
| Create application | `curl -sS -X POST -H "X-Gotify-Key: $GOTIFY_CLIENT_TOKEN" -H 'Content-Type: application/json' "$GOTIFY_URL/application" -d '{"name":"homelab-alerts","description":"..."}'` |
| Delete application (**destructive**) | `curl -sS -X DELETE -H "X-Gotify-Key: $GOTIFY_CLIENT_TOKEN" "$GOTIFY_URL/application/<id>"` |
| List clients | `curl -sS -H "X-Gotify-Key: $GOTIFY_CLIENT_TOKEN" "$GOTIFY_URL/client"` |

## Notification content

Include in each notification: project (`basename "$PWD"`), the specific task, and the status / next action. A session stamp helps: `date -u +session-%Y-%m-%d-%H-%M`.

## Priority reference

| Range | Level | Use for |
|---|---|---|
| 0–3 | Low | Info / FYI |
| 4–7 | Normal | Task updates, completions |
| 8–10 | High | Blocked, errors, urgent |

## Destructive actions

Deleting messages, applications, or clients is irreversible. Confirm with the user before deleting anything; sending a message is fine without asking when the mandatory triggers above apply.

## Configuration

`GOTIFY_URL`, `GOTIFY_APP_TOKEN`, and `GOTIFY_CLIENT_TOKEN` live in `~/.lab/.env`. If a token is empty, report a configuration error. Verify connectivity:

```bash
curl -sS "$GOTIFY_URL/health" -w '\nHTTP %{http_code}\n'
```

## When NOT to use this skill

- The user wants a different notification backend (Apprise, ntfy) — load that skill instead.
- The user is asking about a non-notification homelab service — load that service's skill.
