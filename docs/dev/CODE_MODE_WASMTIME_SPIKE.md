# Code Mode Wasmtime Spike

Date: 2026-07-02

## Version Pins

| Crate | Exact version | Why compatible | Source |
|---|---:|---|---|
| javy-codegen | =4.0.0 | Current crates.io release. Provides `Generator::new(plugin)`, `Generator::linking(LinkingKind::Dynamic)`, and async `Generator::generate(&JS)` for dynamic JS-to-Wasm modules. | `cargo search` on 2026-07-02; `~/.cargo/registry/src/.../javy-codegen-4.0.0/src/lib.rs:183-228,283-363,450-468`; <https://docs.rs/javy-codegen/4.0.0> |
| wasmtime | =46.0.1 | Current crates.io release. Declares `rust-version = 1.94.0`, compatible with this repo's Rust 1.94.1 toolchain. Provides the fuel, epoch, engine, module, store, memory, and resource-limiter APIs needed by later tasks. | `cargo search` / `cargo info` on 2026-07-02; `~/.cargo/registry/src/.../wasmtime-46.0.1/src/config.rs:612,733`; `runtime/store.rs:1014,1041,1105,1136,1168`; `engine.rs:854`; <https://docs.rs/wasmtime/46.0.1> |
| wasmtime-wizer | =46.0.1 | Current crates.io release. Declares `rust-version = 1.94.0`, compatible with this repo. Exposes a Rust `Wizer` API with the `wasmtime` feature; no CLI shell invocation is required by the library path. | `cargo search` / `cargo info` on 2026-07-02; `~/.cargo/registry/src/.../wasmtime-wizer-46.0.1/src/lib.rs:128-166`; `src/wasmtime.rs:7-27`; <https://docs.rs/wasmtime-wizer/46.0.1> |
| deterministic-wasi-ctx | =4.0.0 | Required compatibility pin for `javy-codegen 4.0.0`. Without an exact pin, Cargo selects `4.0.4`, which depends on Wasmtime/WASI 46 while `javy-codegen` depends on Wasmtime/WASI 42, producing a type mismatch in `WasiCtxBuilder`. | Scratch compile proof below; `~/.cargo/registry/src/.../deterministic-wasi-ctx-4.0.0/Cargo.toml`; `javy-codegen-4.0.0/Cargo.toml` |

## Dynamic Linking API

Verified from `javy-codegen 4.0.0` source:

- `Plugin::new(bytes: Cow<'static, [u8]>) -> anyhow::Result<Plugin>`
- `Plugin::new_from_path<P: AsRef<Path>>(path: P) -> anyhow::Result<Plugin>`
- `Plugin::as_bytes(&self) -> &[u8]`
- `Generator::new(plugin: Plugin) -> Generator`
- `Generator::linking(&mut self, LinkingKind::Dynamic) -> &mut Self`
- `Generator::source_embedding(&mut self, SourceEmbedding) -> &mut Self`
- `Generator::producer_version(&mut self, String) -> &mut Self`
- `Generator::deterministic(&mut self, bool) -> &mut Self`
- `Generator::generate(&mut self, js: &JS) -> anyhow::Result<Vec<u8>>` is async.

Dynamic mode adds imports in the plugin namespace for:

- `cabi_realloc(i32, i32, i32, i32) -> i32`
- `invoke(i32, i32, i32, i32, i32) -> ()`
- imported memory named `memory`

The generated dynamic module exports `_start`.

## Wizer Invocation

`wasmtime-wizer 46.0.1` exposes a Rust library API when built with the `wasmtime` feature:

```rust
Wizer::new()
    .init_func("initialize-runtime")
    .run(&mut store, plugin_bytes, async |store, module| {
        // instantiate module and return wasmtime::Instance
    })
    .await
```

The `run` signature is:

```rust
pub async fn run<T: Send>(
    &self,
    store: &mut wasmtime::Store<T>,
    wasm: &[u8],
    instantiate: impl AsyncFnOnce(&mut wasmtime::Store<T>, &wasmtime::Module)
        -> wasmtime::Result<wasmtime::Instance>,
) -> wasmtime::Result<Vec<u8>>
```

Implication for `build.rs`: a shell command is not required, but the build script must drive this async API. Use an explicit runtime/block-on helper rather than assuming synchronous Wizer calls exist.

## Wasmtime Limits

Verified from `wasmtime 46.0.1` source:

- Engine configuration:
  - `wasmtime::Config::consume_fuel(&mut self, bool) -> &mut Self`
  - `wasmtime::Config::epoch_interruption(&mut self, bool) -> &mut Self`
  - `wasmtime::Config::async_support(&mut self, bool) -> &mut Self`
- Per-store fuel:
  - `wasmtime::Store::set_fuel(&mut self, u64) -> wasmtime::Result<()>`
  - `wasmtime::Store::get_fuel(&self) -> wasmtime::Result<u64>`
  - Wasmtime docs state stores start with 0 fuel when fuel is enabled and will trap when fuel is consumed.
- Epoch interruption:
  - `wasmtime::Engine::increment_epoch(&self)`
  - `wasmtime::Store::set_epoch_deadline(&mut self, u64)`
  - `wasmtime::Store::epoch_deadline_trap(&mut self)`
  - `wasmtime::Store::epoch_deadline_callback(...)`
  - Wasmtime docs state a deadline must be configured or the store immediately traps.
- Memory limiting:
  - `wasmtime::Store::limiter(...)`
  - `wasmtime::ResourceLimiter::memory_growing(&mut self, current: usize, desired: usize, maximum: Option<usize>) -> wasmtime::Result<bool>`
  - `ResourceLimiter` docs state it limits WebAssembly instance resources, not every embedder allocation.

## Local Build Proof

Initial scratch proof:

```bash
rm -rf /tmp/labby-wasmtime-spike
mkdir -p /tmp/labby-wasmtime-spike
cd /tmp/labby-wasmtime-spike
cargo init --bin
cargo add javy-codegen@=4.0.0 wasmtime@=46.0.1 wasmtime-wizer@=46.0.1
cargo check
```

Result: failed. Cargo selected `deterministic-wasi-ctx 4.0.4`, which pulled `wasmtime-wasi 46.0.1`. `javy-codegen 4.0.0` itself depends on `wasmtime-wasi 42`, and compilation failed at `javy-codegen/src/lib.rs:244` because `deterministic_wasi_ctx::add_determinism_to_wasi_ctx_builder` expected the 46.x `WasiCtxBuilder` while Javy passed the 42.x type.

Fixed scratch proof:

```bash
cd /tmp/labby-wasmtime-spike
cargo update -p deterministic-wasi-ctx --precise 4.0.0
cargo check
```

Result: passed.

```text
Finished `dev` profile [unoptimized + debuginfo] target(s) in 33.41s
```

Open question for implementation: `javy-codegen 4.0.0` internally uses Wasmtime/WASI/Wizer 42 to build generated modules, while Lab's runtime side will use Wasmtime 46 to compile and run emitted Wasm bytes. The scratch build proves the crates can coexist, but Task 3 must still prove the emitted plugin/snippet modules compile under a `wasmtime 46.0.1` `Engine`.

## Security Gate Result

Status: blocked.

After adding the exact pins to `crates/labby-codemode/Cargo.toml`, the real repo check passed:

```bash
cargo check -p labby-codemode --all-features
```

Result:

```text
Finished `dev` profile [unoptimized] target(s) in 3m 46s
```

The required security gates failed:

```bash
cargo deny check
cargo audit
```

New blocker introduced by the spike dependency set:

- `javy-codegen 4.0.0` depends on `wasmtime = "42"`, `wasmtime-wasi = "42"`, and `wasmtime-wizer = "42"`.
- Current RustSec advisories flag the resulting `wasmtime 42.0.2` and `wasmtime-wasi 42.0.2` lockfile entries:
  - `RUSTSEC-2026-0114` / GHSA-p8xm-42r7-89xg: Wasmtime table allocation panic.
  - `RUSTSEC-2026-0149` / GHSA-2r75-cxrj-cmph: WASI `path_open(TRUNCATE)` bypasses `FilePerms::WRITE`.
  - `RUSTSEC-2026-0182` / GHSA-3p27-qvp9-27qf: WASIp1 `fd_renumber` leak.
  - `RUSTSEC-2026-0188` / GHSA-4ch3-9j33-3pmj: WASI hard links and renames bypass destination `FilePerms`.

`cargo audit` also reported pre-existing workspace advisories for `quinn-proto`
and `rsa`, plus the existing allowed `paste` unmaintained warning, but the
Wasmtime/WASI 42 findings are directly introduced by this spike path and block
this plan's stated security gate.

Conclusion: do not proceed to Task 3 with `javy-codegen 4.0.0` as a production dependency unless the security policy is deliberately changed, Javy publishes a compatible release on a patched Wasmtime/WASI line, or the architecture changes to avoid compiling the vulnerable Wasmtime/WASI 42 dependency tree into this workspace.

The production workspace should not retain the spike dependencies after this
check. Keep this document as the durable artifact, and keep `Cargo.toml` /
`Cargo.lock` clean until a dependency path passes both compilation and the
security gates.
