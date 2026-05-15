# cli/ — clap subcommands (thin shims)

Files in this directory are **thin shims**: parse human CLI args with `clap`, call the shared dispatch layer or a typed dispatch-backed helper, and format results via `output.rs`. That's it.

## The shim rule

A CLI command should be ~20 lines. If yours is longer, the extra lines almost certainly belong in `lab-apis/src/<service>/client.rs`. Business logic in a CLI command is a bug. The two-crate split exists to enforce this — `clap` doesn't even exist in `lab-apis`.

## Destructive actions

Any CLI command backed by an action with `ActionSpec.destructive == true` must honor three flags:

| Flag | Behavior |
|------|----------|
| `-y` / `--yes` | Skip the interactive confirm prompt. Required for non-interactive use. |
| `--no-confirm` | Alias for `-y`. |
| `--dry-run` | Print what would happen; do not call the client. |

Without `-y` and on a TTY, prompt the user before executing. Without `-y` and **not** on a TTY, refuse with a clear error.

## Batch commands

Services that support batch operations expose a positional-variadic pattern:

```bash
lab radarr add 603 604 605          # → radarr_client.add_many(&[603, 604, 605])
lab sonarr delete 12 13 14 --yes    # destructive, requires -y
```

The CLI collects the args and calls the `_many` variant on the client. The client decides whether to parallelize or serialize.

## Output

- Default: human-readable tables via the local renderer in `output.rs`; keep table wrapper/rendering types in `lab`, never in `lab-apis`.
- `--json`: serialize the underlying `lab-apis` type directly with `serde_json`. No wrapper.
- Never `println!` for debug output. Use `tracing` — it respects `--verbose`.

## Error wrapping

Use `anyhow::Result` at the command-function boundary. Convert `lab-apis` errors with `?` — the `From` impls are already wired. Do not swallow `ApiError::kind()` context; let the error bubble so the top-level handler can print it.

## Feature gating

Each command module is `#[cfg(feature = "<service>")]`. Registration in `cli.rs` follows the same pattern. Never hard-depend on a service from the top-level CLI enum — use conditional compilation.
