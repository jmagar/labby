# Changelog

All notable changes to this project will be documented in this file.

Historical entries use distinct `example*` pseudonyms where retired service
identifiers were removed. Commit links remain the authoritative historical record.

## Unreleased

### Added

- **auth/gateway:** add a central Google credential broker that reuses one
  encrypted, subject-scoped provider credential across inbound Labby OAuth and
  compatible Google Workspace MCP upstreams. Shared upstreams validate issuer,
  client binding, account identity, and required scopes; support incremental
  scope upgrades; serialize refreshes; expose redacted status; and require an
  explicit confirmed action for shared revocation. Auth schema v8 adds encrypted
  access-token metadata, granted scopes, client binding, issuer, and lifecycle
  timestamps while preserving dedicated OAuth as the default.
- **docs:** add the complete Google credential broker specification, security
  contract, schema, API/CLI/UI contracts, rollout, rollback, and verification
  guide.
- **proxy:** `labby proxy <stdio-server>` now exposes one stdio MCP server
  faithfully over loopback or an owned Tailscale Serve HTTPS port. Zero-flag
  defaults support tailnet policy, while setup can persist separate bearer or
  stable-issuer OAuth policy, random or fixed ports, endpoint path, child
  environment inheritance, and cleanup preferences.
- **setup/doctor:** `labby setup proxy` stores non-secrets in `config.toml`,
  generates or accepts bearer secrets in the hardened `.env`, and remains
  idempotent. Zero-route `labby doctor proxy` validates local proxy
  prerequisites without removing the existing routed reverse-proxy doctor.
- **oauth:** ephemeral exact-resource leases register each OAuth proxy URL,
  including port and path, through admin gateway actions; the proxy renews and
  releases leases while the daemon expires crash leftovers by TTL.
- **docs:** added the stdio MCP proxy operator guide plus generated proxy
  configuration, environment, service, action, CLI, API, and OpenAPI inventory
  coverage with drift tests.

### Fixed

- **security:** pin patched npm dependencies across the root toolchain, Gateway Admin, and Palette to clear all open Dependabot alerts and current `brace-expansion` and Hono audit findings.
- **auth:** replace per-client copies of Google refresh tokens with one encrypted, subject-scoped provider credential. Google `invalid_grant` responses now compare-and-delete the rejected credential, revoke every dependent local grant, and force the next authorization through fresh consent instead of trapping ChatGPT in an hourly reconnect loop.
- **auth:** reuse the sole allowed Google account credential across DCR/CIMD clients, preventing dynamic client churn from minting hundreds of duplicate Google refresh tokens and evicting older credentials.
- **agents:** expose one versioned, course-correcting error contract across MCP tools and protocol errors, direct upstream proxying, HTTP/OpenAPI, JSON CLI failures, Code Mode, and its trace inspector.
- **code-mode:** preserve completed MCP tool failures as a versioned, model-actionable error contract with tool identity, recovery guidance, side-effect risk, all sanitized content blocks, structured evidence, and original upstream error kinds.
- **gateway:** classify valid MCP tool failures as non-retryable `tool_error`
  responses instead of retryable upstream 502s.
- **gateway:** make Gateway Status `refresh` actively reprobe route-visible MCP
  tool catalogs, so healthy long-lived HTTP upstreams expose newly added tools
  without requiring a Labby restart or reconnect. Status projection now also
  rereads an initially empty catalog after a concurrent healthy connect and marks
  still-unmaterialized lazy catalogs with `catalog_warming`, so a provisional
  `connected: true` / zero-capability snapshot is never presented as authoritative.
- **code-mode:** reconcile failed Microsandbox cleanup records against the live
  labeled guest inventory before opening the creation circuit permanently, and
  repair active-runner accounting when a previously failed cleanup is proven
  absent. Live unresolved guests remain fail closed.
- **setup:** preflight Microsandbox image compatibility before host-service
  install/restart. Legacy mutable aliases and short pinned references are
  migrated from the service user's cached digest to a canonical immutable OCI
  reference before the healthy service is stopped; unprovable migrations fail
  before restart.
- **gateway:** bound concurrent calls per upstream, deduplicate identical in-flight
  Code Mode executions, and exponentially quarantine persistently broken peers.
- **code-mode:** cancel outstanding host work and evict runners that fail to settle
  promptly after every upstream call has completed.
- **stdio:** log child PID, connection generation, exit code or signal, redacted
  stderr tail, and the exact in-flight requests invalidated by child termination.


### Licensing

- Relicense Dinglebear-owned original work under AGPL-3.0-only and document separate commercial licensing; third-party material retains its original terms.

## [1.16.0](https://github.com/dinglebear-ai/labby/compare/v1.15.1...v1.16.0) (2026-09-07)


### Added

* **depot:** add full administration control plane ([#567](https://github.com/dinglebear-ai/labby/issues/567)) ([c83333c](https://github.com/dinglebear-ai/labby/commit/c83333c8bc1704a804801cdb2c480b09d1ddc438))

## [1.15.1](https://github.com/dinglebear-ai/labby/compare/v1.15.0...v1.15.1) (2026-09-05)


### Fixed

* **web:** restore Depot discovery and shell controls ([#559](https://github.com/dinglebear-ai/labby/issues/559)) ([e195956](https://github.com/dinglebear-ai/labby/commit/e195956db73f05e44d5a990cf3ee7b7cec0f71d8))

## [1.15.0](https://github.com/dinglebear-ai/labby/compare/v1.14.1...v1.15.0) (2026-09-05)


### Added

* **api:** expose runtime-backed integration identity ([#543](https://github.com/dinglebear-ai/labby/issues/543)) ([96d7b82](https://github.com/dinglebear-ai/labby/commit/96d7b824c2dc0ef16d7d0e4225a92cd1c72c0e91))
* **artifacts:** add agent artifact materialization ([#554](https://github.com/dinglebear-ai/labby/issues/554)) ([2af29be](https://github.com/dinglebear-ai/labby/commit/2af29bec298940e10b868ca32feb7aa8b159cc40))
* **artifacts:** add hook artifact materialization ([#555](https://github.com/dinglebear-ai/labby/issues/555)) ([966984a](https://github.com/dinglebear-ai/labby/commit/966984af49e96d0604b372ad4889d7baa955c5a7))
* **artifacts:** add prompt artifact materialization ([#553](https://github.com/dinglebear-ai/labby/issues/553)) ([f2613f3](https://github.com/dinglebear-ai/labby/commit/f2613f3f195ee7d5d4b248986c20e73600dd5500))
* **auth:** add provider-neutral Authelia OAuth ([#558](https://github.com/dinglebear-ai/labby/issues/558)) ([141980d](https://github.com/dinglebear-ai/labby/commit/141980d6a7112c2c2f06164ab761acfa709dd7be))
* **depot:** polish artifact discovery ([#545](https://github.com/dinglebear-ai/labby/issues/545)) ([3513032](https://github.com/dinglebear-ai/labby/commit/35130322e9292b7529ef0ce8485532f8fc48cb2d))
* **depot:** unify the Labby and Depot control plane ([#519](https://github.com/dinglebear-ai/labby/issues/519)) ([3f2e0dc](https://github.com/dinglebear-ai/labby/commit/3f2e0dc54727827e40cea5e341e035bf34de55e6))
* **gateway:** add durable delegated exact execution ([#561](https://github.com/dinglebear-ai/labby/issues/561)) ([57b403f](https://github.com/dinglebear-ai/labby/commit/57b403f4d1d3dd2e594087fe3af7b3d795028569))
* **gateway:** add execution loadout foundation ([#556](https://github.com/dinglebear-ai/labby/issues/556)) ([a901e33](https://github.com/dinglebear-ai/labby/commit/a901e3358ea5e65eec1a09a4630abc6874a60643))
* integrate trusted Core provider without vendored rmcp ([#533](https://github.com/dinglebear-ai/labby/issues/533)) ([cc175a8](https://github.com/dinglebear-ai/labby/commit/cc175a8e6ba0e484387a86b8e25d167e09e4e6d8))
* **loadouts:** publish canonical execution catalogs ([#562](https://github.com/dinglebear-ai/labby/issues/562)) ([2d0ba9d](https://github.com/dinglebear-ai/labby/commit/2d0ba9dff59dd6bea0d912d68f15da4e875aca99))
* **mcp:** make Labby-owned apps opt-in ([#483](https://github.com/dinglebear-ai/labby/issues/483)) ([893a9fc](https://github.com/dinglebear-ai/labby/commit/893a9fc93e92aaacd1df74556036dd9caf44b741))
* **palette:** preserve explicit execution mode receipts ([#544](https://github.com/dinglebear-ai/labby/issues/544)) ([09b010a](https://github.com/dinglebear-ai/labby/commit/09b010a00ade9fd47c3f9bf3c54ffffe56b4533f))
* **snippets:** preserve descriptive tool declaration metadata ([#547](https://github.com/dinglebear-ai/labby/issues/547)) ([e9613e5](https://github.com/dinglebear-ai/labby/commit/e9613e5cc0ec12133522060ffbc9bf5341a19f2f))
* **web:** optimize console controls for mobile ([#549](https://github.com/dinglebear-ai/labby/issues/549)) ([19296fd](https://github.com/dinglebear-ai/labby/commit/19296fdebd80cf073ea0d06c63fb47a1884a619c))


### Fixed

* **auth:** recover generation-safe OAuth refresh and redaction ([#539](https://github.com/dinglebear-ai/labby/issues/539)) ([3244faa](https://github.com/dinglebear-ai/labby/commit/3244faa0bed794bca67a48b756c1cbfcb5b603d3))
* **auth:** remediate Authelia review findings ([f79d9fb](https://github.com/dinglebear-ai/labby/commit/f79d9fb07f603deced5231e264528664414999fa))
* **ci:** clear remaining strict warning blockers ([#528](https://github.com/dinglebear-ai/labby/issues/528)) ([b33d473](https://github.com/dinglebear-ai/labby/commit/b33d473a727acccefad283744ec597e03dcee002))
* **ci:** restore Cargo manifest qualification ([#538](https://github.com/dinglebear-ai/labby/issues/538)) ([8c27c5f](https://github.com/dinglebear-ai/labby/commit/8c27c5f0f78cd1d32bd55bdb2ded83e942649497))
* **ci:** restore feature-slice builds ([#526](https://github.com/dinglebear-ai/labby/issues/526)) ([e95ea22](https://github.com/dinglebear-ai/labby/commit/e95ea22cf3804e0f3b77cff6752d331039913192))
* **gateway:** connect lazy upstreams before Code Mode resource reads ([#551](https://github.com/dinglebear-ai/labby/issues/551)) ([322839d](https://github.com/dinglebear-ai/labby/commit/322839d3157fd7c0fe9264c4e0ecddc658c7ea0f))
* **loadouts:** enforce tenant catalog authority ([#563](https://github.com/dinglebear-ai/labby/issues/563)) ([1a04d46](https://github.com/dinglebear-ai/labby/commit/1a04d4678d12d5e0ed6469f865a563e985337b50))
* **mcp:** execute project-bound Skill Library actions ([#534](https://github.com/dinglebear-ai/labby/issues/534)) ([23538b7](https://github.com/dinglebear-ai/labby/commit/23538b7d714bad134f15b2ae502cf790403776cb))
* **palette:** bound exact launcher searches to published snapshots ([#540](https://github.com/dinglebear-ai/labby/issues/540)) ([af99b84](https://github.com/dinglebear-ai/labby/commit/af99b8414aa0f3386fa9a11ef367e1b56a3523db))
* **palette:** recover desktop endpoint status and macOS window behavior ([#548](https://github.com/dinglebear-ai/labby/issues/548)) ([fb84451](https://github.com/dinglebear-ai/labby/commit/fb8445148c792403008d54da5c6ccf214d419805))
* restore hosted mainline CI ([40625dd](https://github.com/dinglebear-ai/labby/commit/40625dd20f52c66b67117817d1c97e4b6c532524))
* **runtime:** remove obsolete macOS path helper ([#527](https://github.com/dinglebear-ai/labby/issues/527)) ([0e6d508](https://github.com/dinglebear-ai/labby/commit/0e6d508e3023881a21c3fa358088f3f0de536596))
* **skills:** preserve nested identities and bounded websocket payloads ([#537](https://github.com/dinglebear-ai/labby/issues/537)) ([061d2ae](https://github.com/dinglebear-ai/labby/commit/061d2aea297b3581126999a4d7caaa20fa2b7591))
* **skills:** preserve strict gateway params ([#501](https://github.com/dinglebear-ai/labby/issues/501)) ([6514263](https://github.com/dinglebear-ai/labby/commit/6514263a68ed4ebcb219e3bcd8bf5d8160a2bbde))
* **test:** recover bounded live and browser harness cleanup ([#542](https://github.com/dinglebear-ai/labby/issues/542)) ([6fe70f5](https://github.com/dinglebear-ai/labby/commit/6fe70f5e81f0daf0639c3dc4585faa0fa519194b))

## [1.14.1](https://github.com/dinglebear-ai/labby/compare/v1.14.0...v1.14.1) (2026-08-24)


### Fixed

* **codemode:** restore Windows warning-clean build ([#474](https://github.com/dinglebear-ai/labby/issues/474)) ([176495d](https://github.com/dinglebear-ai/labby/commit/176495de6837c1683c22e5b4f41bc092c4d6ec17))
* **gateway:** stop Loadouts page renderer crash and harden parallel tests ([#489](https://github.com/dinglebear-ai/labby/issues/489)) ([172e078](https://github.com/dinglebear-ai/labby/commit/172e07896aef3fadc037f77e55be1499b62e02ad))

## [1.14.0](https://github.com/dinglebear-ai/labby/compare/v1.13.3...v1.14.0) (2026-08-21)


### Added

* **artifacts:** add lifecycle planning and provider seam ([#464](https://github.com/dinglebear-ai/labby/issues/464)) ([636f35b](https://github.com/dinglebear-ai/labby/commit/636f35bb4286101a03e7beedd3a5d839c66ec3e4))
* **artifacts:** add open personal Artifact core and v1 interchange ([#462](https://github.com/dinglebear-ai/labby/issues/462)) ([8723910](https://github.com/dinglebear-ai/labby/commit/87239104c84874694748ed7135919e11a8d76d4b))
* **codemode:** add opt-in Microsandbox runner isolation ([#434](https://github.com/dinglebear-ai/labby/issues/434)) ([0e21d04](https://github.com/dinglebear-ai/labby/commit/0e21d0474211f88371773244f5bbb021e710630a))
* **codemode:** add safety-aware tool discovery ([#435](https://github.com/dinglebear-ai/labby/issues/435)) ([a806085](https://github.com/dinglebear-ai/labby/commit/a806085f9be32ca8364f9515199896e7e111727e))
* **codemode:** expose upstream resource reads ([#458](https://github.com/dinglebear-ai/labby/issues/458)) ([d18adf8](https://github.com/dinglebear-ai/labby/commit/d18adf85460d023643c64e1bcacbd6bcf67ba743))
* **gateway:** add transport recovery guardrails ([#452](https://github.com/dinglebear-ai/labby/issues/452)) ([4715a2e](https://github.com/dinglebear-ai/labby/commit/4715a2e0578e8d8138b70d81add01445c51ad0a2))
* **gateway:** first-class Skills over MCP and Loadouts ([#448](https://github.com/dinglebear-ai/labby/issues/448)) ([bdf914c](https://github.com/dinglebear-ai/labby/commit/bdf914c1519c1cdc40f7391376c2316f0f299a40))
* **gateway:** make usage analytics first class ([#459](https://github.com/dinglebear-ai/labby/issues/459)) ([52061b6](https://github.com/dinglebear-ai/labby/commit/52061b650533a223c63fb8fea182b775ea807fe4))
* **mcp:** add MCP app visibility switchboard ([#445](https://github.com/dinglebear-ai/labby/issues/445)) ([9977a56](https://github.com/dinglebear-ai/labby/commit/9977a561933302a2d81a8f3bb8c6db38c587c854))
* **mcp:** polish MCP app integration ([#437](https://github.com/dinglebear-ai/labby/issues/437)) ([d938c25](https://github.com/dinglebear-ai/labby/commit/d938c255de4184c4e388f44e701ce57e7c806823))
* **skills:** add shared MCP compatibility facade ([#456](https://github.com/dinglebear-ai/labby/issues/456)) ([ea07f36](https://github.com/dinglebear-ai/labby/commit/ea07f3609926879da146ea6e3e3afdd65784a73a))
* **web:** align operator workflows with gateway mock ([#430](https://github.com/dinglebear-ai/labby/issues/430)) ([329c4fb](https://github.com/dinglebear-ai/labby/commit/329c4fb61d25e40f89a525ff3bb4b1eaa5058e94))


### Fixed

* address findings from recent merged PR review ([#469](https://github.com/dinglebear-ai/labby/issues/469)) ([3a61196](https://github.com/dinglebear-ai/labby/commit/3a6119682824987b60b674b4bedaaf403b64f4ae))
* align protected subset cold-start discovery ([#436](https://github.com/dinglebear-ai/labby/issues/436)) ([76abf66](https://github.com/dinglebear-ai/labby/commit/76abf663a173026bcd5467f2b8f573217763d1ad))
* **ci:** install the pinned Rust 1.97.1 toolchain ([#451](https://github.com/dinglebear-ai/labby/issues/451)) ([2532186](https://github.com/dinglebear-ai/labby/commit/2532186f1fb762038b310cf6b392299030e7b5a0))
* **codemode:** make source size limit configurable ([#468](https://github.com/dinglebear-ai/labby/issues/468)) ([ea9873c](https://github.com/dinglebear-ai/labby/commit/ea9873c1b5f62d9c85c8d97445577d0ee821871b))
* **codemode:** preserve outer timeout during settlement ([#443](https://github.com/dinglebear-ai/labby/issues/443)) ([8d0a39b](https://github.com/dinglebear-ai/labby/commit/8d0a39bd498c8fad5db1b41df0514d94497ff1eb))
* **codemode:** recover microsandbox cleanup circuit ([#463](https://github.com/dinglebear-ai/labby/issues/463)) ([bb30616](https://github.com/dinglebear-ai/labby/commit/bb30616f43600df92485391ef0986baa21cacae1))
* **codemode:** retry dead pooled runner once ([#441](https://github.com/dinglebear-ai/labby/issues/441)) ([69ab053](https://github.com/dinglebear-ai/labby/commit/69ab0533b96d18fccc45e0a01ab3dd564a430aa1))
* **deps:** clear remaining npm security alerts ([#450](https://github.com/dinglebear-ai/labby/issues/450)) ([80e94c5](https://github.com/dinglebear-ai/labby/commit/80e94c5501a5a1c82377b5ea01f1b35a5d24c65b))
* **gateway:** reprobe tools on status refresh ([#453](https://github.com/dinglebear-ai/labby/issues/453)) ([85cbedb](https://github.com/dinglebear-ai/labby/commit/85cbedb926edf80ff4588ea2d76f57914f78780e))
* harden Code Mode settlement and SEP-2243 header recovery ([#446](https://github.com/dinglebear-ai/labby/issues/446)) ([f7613b9](https://github.com/dinglebear-ai/labby/commit/f7613b9f109c9130d5a16acedd58cb4c9427f1e4))
* harden Microsandbox recovery and gateway warmup status ([#466](https://github.com/dinglebear-ai/labby/issues/466)) ([90d3f29](https://github.com/dinglebear-ai/labby/commit/90d3f29bdfd5a61b02e7c649977c345e5a94c05c))
* **mcp:** stop paginated catalog retry storms ([#442](https://github.com/dinglebear-ai/labby/issues/442)) ([310296e](https://github.com/dinglebear-ai/labby/commit/310296e57a72956b7ccff40cb92d1f0bedf9176c))
* **skills:** relay native Skills through gateway ([#467](https://github.com/dinglebear-ai/labby/issues/467)) ([90cab5b](https://github.com/dinglebear-ai/labby/commit/90cab5bff14f92ebdefadfefc7ffbd368b553b9e))

## [1.13.3](https://github.com/dinglebear-ai/labby/compare/v1.13.2...v1.13.3) (2026-08-18)


### Fixed

* **container:** refresh patched Debian base packages ([#439](https://github.com/dinglebear-ai/labby/issues/439)) ([478ec68](https://github.com/dinglebear-ai/labby/commit/478ec68cdfca2e1d3d1d3fe4747d1a79593b3de3))

## [1.13.2](https://github.com/dinglebear-ai/labby/compare/v1.13.1...v1.13.2) (2026-08-18)


### Fixed

* **auth:** tolerate concurrent refresh retries ([#427](https://github.com/dinglebear-ai/labby/issues/427)) ([56fee44](https://github.com/dinglebear-ai/labby/commit/56fee44250601533bd7eb166fde56e00e26b3c22))
* **gateway:** stop unsupported subscription retries ([#438](https://github.com/dinglebear-ai/labby/issues/438)) ([f0cc063](https://github.com/dinglebear-ai/labby/commit/f0cc063dbc88075ac64b20415cd5c11edad058a7))
* make remote gateway targets authoritative ([#429](https://github.com/dinglebear-ai/labby/issues/429)) ([fc75b29](https://github.com/dinglebear-ai/labby/commit/fc75b29249a23ce253ea30e07a4d74cf5d7bb364))
* **release:** restore container and Incus smoke builds ([#425](https://github.com/dinglebear-ai/labby/issues/425)) ([63bb92d](https://github.com/dinglebear-ai/labby/commit/63bb92d92ef1dffbff73acb045c694b682c1ebd0))
* **setup:** honor configured remote plugin target ([#428](https://github.com/dinglebear-ai/labby/issues/428)) ([196d811](https://github.com/dinglebear-ai/labby/commit/196d811e4ea70322b95647e573d92c6f6c148f7d))

## [1.13.1](https://github.com/dinglebear-ai/labby/compare/v1.13.0...v1.13.1) (2026-08-15)


### Fixed

* **mcp:** surface nested apps from Code Mode ([#423](https://github.com/dinglebear-ai/labby/issues/423)) ([fd04604](https://github.com/dinglebear-ai/labby/commit/fd04604f0d977b6b305c578260095801ac448f2d))

## [1.13.0](https://github.com/dinglebear-ai/labby/compare/v1.12.0...v1.13.0) (2026-08-15)


### Added

* **web:** align gateway console experience ([#409](https://github.com/dinglebear-ai/labby/issues/409)) ([2fc15c7](https://github.com/dinglebear-ai/labby/commit/2fc15c7afdb2ba0789d22bb47c8998752c3f6d96))
* **web:** show gateway capability status cluster ([#420](https://github.com/dinglebear-ai/labby/issues/420)) ([5c7696a](https://github.com/dinglebear-ai/labby/commit/5c7696a6ee719a9a211df0dee46749c7357e4dcc))


### Fixed

* **auth:** enforce OAuth egress policy ([#408](https://github.com/dinglebear-ai/labby/issues/408)) ([9eab82b](https://github.com/dinglebear-ai/labby/commit/9eab82b594d3179db3bac2210552ec32013b68c5))
* **auth:** harden OAuth metadata discovery ([#414](https://github.com/dinglebear-ai/labby/issues/414)) ([b11904b](https://github.com/dinglebear-ai/labby/commit/b11904b5357b12a8115b7bdc52d4a5a04c4e3c08))
* **skills:** harden native URI aggregation ([#410](https://github.com/dinglebear-ai/labby/issues/410)) ([80a61c5](https://github.com/dinglebear-ai/labby/commit/80a61c570cbaff5058707f9ce548774ede4fec1b))

## [1.12.0](https://github.com/dinglebear-ai/labby/compare/v1.11.0...v1.12.0) (2026-08-14)


### Added

* **mcp:** add the SEP-2640 skills vocabulary to labby-runtime ([#396](https://github.com/dinglebear-ai/labby/issues/396)) ([0a98c58](https://github.com/dinglebear-ai/labby/commit/0a98c58b32c970f7f1b01107163a36c09710d6c8))
* **mcp:** advertise tool safety annotations ([#402](https://github.com/dinglebear-ai/labby/issues/402)) ([20361b2](https://github.com/dinglebear-ai/labby/commit/20361b2fea06ac23f8282e98a90579cea25248ee))
* **mcp:** outputSchema for listed tools + lock Code Mode structured content ([#210](https://github.com/dinglebear-ai/labby/issues/210)) ([#399](https://github.com/dinglebear-ai/labby/issues/399)) ([313e589](https://github.com/dinglebear-ai/labby/commit/313e5896945f4e784238843f811a9d85f9d00600))
* **skills:** accept upstream skills under any URI scheme ([#403](https://github.com/dinglebear-ai/labby/issues/403)) ([ad80444](https://github.com/dinglebear-ai/labby/commit/ad804441cb32569c73dea38c86851aaee133982b))
* **web:** use persisted gateway metrics in dashboard ([#406](https://github.com/dinglebear-ai/labby/issues/406)) ([5f1ac8b](https://github.com/dinglebear-ai/labby/commit/5f1ac8ba311e9941e9d635c2649da5f5b46595b0))


### Fixed

* **gateway:** align destructive flags with the data-loss definition ([#395](https://github.com/dinglebear-ai/labby/issues/395)) ([10a6890](https://github.com/dinglebear-ai/labby/commit/10a6890d0fbee2a78321554b5895df8f792fc171))
* **gateway:** an upstream with no tools capability is tool-less, not broken ([#404](https://github.com/dinglebear-ai/labby/issues/404)) ([9513887](https://github.com/dinglebear-ai/labby/commit/9513887c6fbadef080045e4f508324d2239563e1))
* **gateway:** bound upstream catalog pagination ([#393](https://github.com/dinglebear-ai/labby/issues/393)) ([bac804d](https://github.com/dinglebear-ai/labby/commit/bac804da2ef363b83120799b7024a56e48e97924))
* harden MCP and OAuth lifecycle ([#400](https://github.com/dinglebear-ai/labby/issues/400)) ([3fe0668](https://github.com/dinglebear-ai/labby/commit/3fe06684fd5ee13a0414c6b5a8fec32ff9d20e6a))
* **mcp:** advertise resource subscriptions honestly to legacy clients ([#401](https://github.com/dinglebear-ai/labby/issues/401)) ([2de5184](https://github.com/dinglebear-ai/labby/commit/2de5184a85991916580db2957b977e8c8aed36b1))
* **mcp:** bound resource catalog refreshes ([#390](https://github.com/dinglebear-ai/labby/issues/390)) ([43d51ec](https://github.com/dinglebear-ai/labby/commit/43d51ec73c8b759f36cc7b2f44930f4e9f5ad893))

## [1.11.0](https://github.com/dinglebear-ai/labby/compare/v1.10.1...v1.11.0) (2026-08-10)


### Added

* **gateway:** allow Bun stdio upstreams ([#385](https://github.com/dinglebear-ai/labby/issues/385)) ([f111813](https://github.com/dinglebear-ai/labby/commit/f11181380b629ab27b0ec5ce439f9ddc93351bf4))

## [1.10.1](https://github.com/dinglebear-ai/labby/compare/v1.10.0...v1.10.1) (2026-08-09)


### Fixed

* **mcp:** stabilize ChatGPT Code Mode without freezing descriptions ([#386](https://github.com/dinglebear-ai/labby/issues/386)) ([0eb9280](https://github.com/dinglebear-ai/labby/commit/0eb9280123b6918491ccd005cc098ee129190eb1))

## [1.10.0](https://github.com/dinglebear-ai/labby/compare/v1.9.0...v1.10.0) (2026-08-08)


### Added

* add native per-upstream OAuth for stdio mode ([#370](https://github.com/dinglebear-ai/labby/issues/370)) ([b3ed59f](https://github.com/dinglebear-ai/labby/commit/b3ed59f3002fa0648664c4796d4898692cf57a2b))
* **auth:** add shared Google credential broker ([#356](https://github.com/dinglebear-ai/labby/issues/356)) ([cb6d404](https://github.com/dinglebear-ai/labby/commit/cb6d4044a0ac6839daf40b25d4ad9dc8fbb54f72))
* **incus:** move the supported gateway runtime to Ubuntu 26.04 ([#381](https://github.com/dinglebear-ai/labby/issues/381)) ([43ac1e0](https://github.com/dinglebear-ai/labby/commit/43ac1e0c3d58b82add5711bef257b9309afe9d4f))
* **unraid:** bump default incus image to 1.8.5 (verified labby 1.8.5) — 1.4.2 ([#372](https://github.com/dinglebear-ai/labby/issues/372)) ([07ff238](https://github.com/dinglebear-ai/labby/commit/07ff2389fe8c333a51de9fb7574d7e1a2d97efef))


### Fixed

* **auth:** accept CIMD private_key_jwt clients that publish jwks_uri ([#375](https://github.com/dinglebear-ai/labby/issues/375)) ([020f227](https://github.com/dinglebear-ai/labby/commit/020f22733e99920f417e2ac9c25107ce4ac8b2b9))
* **auth:** honour every auth method a CIMD client publishes, and log silent rejections ([#382](https://github.com/dinglebear-ai/labby/issues/382)) ([6e650ad](https://github.com/dinglebear-ai/labby/commit/6e650add84634941fe2e05473a91224006aaf971))
* **auth:** require explicit upstream OAuth callback base ([#374](https://github.com/dinglebear-ai/labby/issues/374)) ([8a06736](https://github.com/dinglebear-ai/labby/commit/8a06736cf0190cc7cfbd745ab606102397bc4069))
* **auth:** restore product route observability ([#384](https://github.com/dinglebear-ai/labby/issues/384)) ([2e19c83](https://github.com/dinglebear-ai/labby/commit/2e19c836318a9c9851886f3b13f3bd4a0b314e8c))
* **brand:** verify the fetched font and stop swallowing render failures ([#361](https://github.com/dinglebear-ai/labby/issues/361)) ([7c8ea81](https://github.com/dinglebear-ai/labby/commit/7c8ea81262efa70e32eb60d893c80de66d2e90c5))
* **build:** use portable install commands on macOS ([#376](https://github.com/dinglebear-ai/labby/issues/376)) ([b283935](https://github.com/dinglebear-ai/labby/commit/b283935ab34cb59c1f3c29b202cb5ce4e12425d0))
* **ci:** drop the literal internal endpoint fallback in a public repo ([#377](https://github.com/dinglebear-ai/labby/issues/377)) ([273f7b9](https://github.com/dinglebear-ai/labby/commit/273f7b93f322f951ec65d046d2788b07b0792aa3))
* **ci:** let the release reminder actually see draft releases ([#367](https://github.com/dinglebear-ai/labby/issues/367)) ([61fe1c4](https://github.com/dinglebear-ai/labby/commit/61fe1c4c71a91c7fcac4ff946a3bd9d3002f3973))
* **ci:** link draft releases to a page that actually exists ([#369](https://github.com/dinglebear-ai/labby/issues/369)) ([bd6f54a](https://github.com/dinglebear-ai/labby/commit/bd6f54ad57b3d02b585d01166a089e6b53b882d2))
* **ci:** never let a changed-path gate skip a required job silently ([#357](https://github.com/dinglebear-ai/labby/issues/357)) ([a6d55c1](https://github.com/dinglebear-ai/labby/commit/a6d55c10d510fefba83e168b70344bfd6bf9f40a))
* **ci:** realign the kache guard with the removed endpoint literal ([#379](https://github.com/dinglebear-ai/labby/issues/379)) ([160b3a0](https://github.com/dinglebear-ai/labby/commit/160b3a07798e3679a4d33af8b6128e18fee7676d))
* **gateway:** enforce expose_resources and expose_prompts (lab-r8cdd) ([#380](https://github.com/dinglebear-ai/labby/issues/380)) ([3ab1473](https://github.com/dinglebear-ai/labby/commit/3ab1473d4913889038e2c8816d069a7f4e4207f4))
* **gateway:** enforce expose_tools on OAuth subject-scoped upstreams ([#378](https://github.com/dinglebear-ai/labby/issues/378)) ([433d74b](https://github.com/dinglebear-ai/labby/commit/433d74ba11c7924b205358b6147aa2248acc5dbf))
* **gateway:** make discovery, macOS setup, and long runs reliable ([#373](https://github.com/dinglebear-ai/labby/issues/373)) ([3cdf011](https://github.com/dinglebear-ai/labby/commit/3cdf0115a98f0b6cf288f17fad7486bedcd75993))
* **gateway:** route task RPCs through the bulkhead and bound upstream error payloads ([#344](https://github.com/dinglebear-ai/labby/issues/344)) ([fe2810f](https://github.com/dinglebear-ai/labby/commit/fe2810f56364f5c7fb6531df1e0758033e3307ac))
* **mcp:** let the pool own upstream health accounting ([#348](https://github.com/dinglebear-ai/labby/issues/348)) ([6781264](https://github.com/dinglebear-ai/labby/commit/678126485ed5296318f0a331e33acffcbb9be32f))
* **npm:** resync the launcher README with the repo README ([#359](https://github.com/dinglebear-ai/labby/issues/359)) ([1324488](https://github.com/dinglebear-ai/labby/commit/132448802ebe5e1e0bbf85c39bf646acae52c912))
* **ui:** guard code mode inspector against trace re-delivery races ([#342](https://github.com/dinglebear-ai/labby/issues/342)) ([bf31f40](https://github.com/dinglebear-ai/labby/commit/bf31f4019b1bde15ed14a9e970f72b7adadecc86))


### Changed

* **errors:** consolidate redaction helpers and close contract gaps ([#351](https://github.com/dinglebear-ai/labby/issues/351)) ([8cde49d](https://github.com/dinglebear-ai/labby/commit/8cde49dbce90d7d4669b1890dc87a70cc2a2a156))

## [1.9.0](https://github.com/dinglebear-ai/labby/compare/v1.8.9...v1.9.0) (2026-08-05)


### Added

* **errors:** unify agent-facing recovery contract ([#331](https://github.com/dinglebear-ai/labby/issues/331)) ([3e5ab3d](https://github.com/dinglebear-ai/labby/commit/3e5ab3dfbb09f538a0d09e8df90f6cfb2e6dab03))
* **release:** publish canonical MCP Registry metadata ([#330](https://github.com/dinglebear-ai/labby/issues/330)) ([27c2bb8](https://github.com/dinglebear-ai/labby/commit/27c2bb89761f23018374c24eeac5c060aa9ca64a))
* **unraid:** prepare Labby for Community Applications ([#338](https://github.com/dinglebear-ai/labby/issues/338)) ([b4625af](https://github.com/dinglebear-ai/labby/commit/b4625af12ef8abb701f571bd102cb48ffc75aa5a))


### Fixed

* **deps:** clear npm Dependabot alerts ([#340](https://github.com/dinglebear-ai/labby/issues/340)) ([a47d214](https://github.com/dinglebear-ai/labby/commit/a47d214ff36400fb455e7cec3229810e537f1457))
* **gateway:** preserve MCP errors and schema fidelity ([#334](https://github.com/dinglebear-ai/labby/issues/334)) ([67a335a](https://github.com/dinglebear-ai/labby/commit/67a335ad49d42eefc18bf525247d2aede93ab177))
* **incus:** raise gateway service resource limits ([#339](https://github.com/dinglebear-ai/labby/issues/339)) ([6d624ae](https://github.com/dinglebear-ai/labby/commit/6d624ae601e7b8f21c59aa0e9a48cd9840f6b219))

## [1.8.9](https://github.com/dinglebear-ai/labby/compare/v1.8.8...v1.8.9) (2026-08-04)


### Fixed

* **auth:** recover revoked Google refresh credentials ([#335](https://github.com/dinglebear-ai/labby/issues/335)) ([3d352ce](https://github.com/dinglebear-ai/labby/commit/3d352cefeaeebc87fe7404309f52bbeda491f0c6))
* **ci:** use python3 for MCP drift workflow ([#327](https://github.com/dinglebear-ai/labby/issues/327)) ([2f69b99](https://github.com/dinglebear-ai/labby/commit/2f69b9935ed8ad31f335c37b21e8f32598b4d8bf))

## [1.8.8](https://github.com/dinglebear-ai/labby/compare/v1.8.7...v1.8.8) (2026-08-04)

### Fixed

* **container:** install Claude Code native binary under npm 12 ([#322](https://github.com/dinglebear-ai/labby/issues/322)) ([0322a37](https://github.com/dinglebear-ai/labby/commit/0322a37e6ddc3237f4a048e5882d92acffa0dd83))
* **container:** upgrade Claude Code to 2.1.163 to clear the release Trivy HIGH findings
* **container:** replace vulnerable npm-bundled `brace-expansion` and `ip-address` with checksum-pinned fixed patches
* **release:** compile the stdio process wrapper without Windows-only `unused_mut` failures
* **ci:** fall back to bare Cargo when a self-hosted runner exposes a locked Actions tool cache

## [1.8.7](https://github.com/dinglebear-ai/labby/compare/v1.8.6...v1.8.7) (2026-08-03)


### Fixed

* **container:** replace vulnerable bundled npm ([#320](https://github.com/dinglebear-ai/labby/issues/320)) ([2794c8d](https://github.com/dinglebear-ai/labby/commit/2794c8d2d331e2d1e8a13ff849e63ff3d55da79e))
* **release:** publish MCP metadata with dinglebear.ai ([9f13652](https://github.com/dinglebear-ai/labby/commit/9f136523e723e49209194c6ea9ada59c121818d6))

## [1.8.6](https://github.com/dinglebear-ai/labby/compare/v1.8.5...v1.8.6) (2026-08-01)


### Fixed

* enforce MCP Registry description limit ([#296](https://github.com/dinglebear-ai/labby/issues/296)) ([7754e9e](https://github.com/dinglebear-ai/labby/commit/7754e9ea0215bc12d5a626c77a07b53a8cbcabae))
* make release trigger contract cross-platform ([#302](https://github.com/dinglebear-ai/labby/issues/302)) ([eff39c7](https://github.com/dinglebear-ai/labby/commit/eff39c79d97c907f6c9956f4711ecab5cd8df62f))
* publish npm launcher as @dinglebear/labby ([3385d1a](https://github.com/dinglebear-ai/labby/commit/3385d1a07cca063a1669d851a71011d21f4971af))
* publish npm launcher under dinglebear scope ([#301](https://github.com/dinglebear-ai/labby/issues/301)) ([47daf44](https://github.com/dinglebear-ai/labby/commit/47daf442f02a8a7e70dc410b083472a96f9b715b))

## [1.8.5](https://github.com/dinglebear-ai/labby/compare/v1.8.4...v1.8.5) (2026-07-30)


### Fixed

* authenticate npm release publication ([#294](https://github.com/dinglebear-ai/labby/issues/294)) ([86de7d7](https://github.com/dinglebear-ai/labby/commit/86de7d781c5c8ee74e48a4b77c63574a6b87decc))
* isolate resource pagination fixture ([#292](https://github.com/dinglebear-ai/labby/issues/292)) ([0f5f45f](https://github.com/dinglebear-ai/labby/commit/0f5f45ff90020b3133fa8f6fd697234d6d237dab))

## [1.8.4](https://github.com/dinglebear-ai/labby/compare/v1.8.3...v1.8.4) (2026-07-30)


### Fixed

* validate draft release assets locally ([#290](https://github.com/dinglebear-ai/labby/issues/290)) ([4b6d80a](https://github.com/dinglebear-ai/labby/commit/4b6d80ab5f961d4ede1db07e6b0ad1823d31d03f))

## [1.8.3](https://github.com/dinglebear-ai/labby/compare/v1.8.2...v1.8.3) (2026-07-30)


### Fixed

* align release Incus builder ([#288](https://github.com/dinglebear-ai/labby/issues/288)) ([e4fbb6e](https://github.com/dinglebear-ai/labby/commit/e4fbb6e12c7c9c9243f405dc0141f647d1d7d8d7))

## [1.8.2](https://github.com/dinglebear-ai/labby/compare/v1.8.1...v1.8.2) (2026-07-30)


### Fixed

* reap one-shot release smoke runners ([#286](https://github.com/dinglebear-ai/labby/issues/286)) ([7e75085](https://github.com/dinglebear-ai/labby/commit/7e75085aa0a06ea0c37e29a1f2286d0f3314e9ff))

## [1.8.1](https://github.com/dinglebear-ai/labby/compare/v1.8.0...v1.8.1) (2026-07-30)


### Fixed

* **auth:** stabilize OAuth observability tests ([a1bb3c4](https://github.com/dinglebear-ai/labby/commit/a1bb3c4dcd332863b9432ba5ba12f19c5f889951))
* avoid top-level await in mock tests ([#285](https://github.com/dinglebear-ai/labby/issues/285)) ([977709c](https://github.com/dinglebear-ai/labby/commit/977709cf3f6bed17e13b55b4cb36e0c9cba29dac))
* **ci:** install Node in release preflight so releases can publish ([5150968](https://github.com/dinglebear-ai/labby/commit/5150968cf0bb092c93b94b5336c7b2b26e88c980))
* **ci:** install Playwright runtime libraries ([9324aa1](https://github.com/dinglebear-ai/labby/commit/9324aa1515812a63ccdf2247b21fab7982ae388d))
* **ci:** remove retired soldr cache preset ([581da2d](https://github.com/dinglebear-ai/labby/commit/581da2dba123e1cc1116a8f02d1deb4b0a1e996b))
* **ci:** use cached Playwright browser on Ubuntu 26.04 ([f3ba237](https://github.com/dinglebear-ai/labby/commit/f3ba2370979a83f2527a9601f9ff04ec1df23d77))
* **oauth:** make Codex issuer compatibility explicit ([1368218](https://github.com/dinglebear-ai/labby/commit/1368218aa066e5b7e822d8d5dbe3a4ab7a42e4cc))


### Changed


* remove retired media stack residue ([#284](https://github.com/dinglebear-ai/labby/issues/284)) ([83a6a21](https://github.com/dinglebear-ai/labby/commit/83a6a21674a1894983f0c53ff551ff4c3e4951f2))

## [1.8.0](https://github.com/dinglebear-ai/labby/compare/v1.7.0...v1.8.0) (2026-07-29)


### Added

* **cli:** add deployment-aware labby logs command ([3fec53c](https://github.com/dinglebear-ai/labby/commit/3fec53c49e84956c5c1d7448ae3ed3c3d997179a))
* **codemode:** warn before oversized result truncation ([923a432](https://github.com/dinglebear-ai/labby/commit/923a432190ea7d6f97bec577163983f5901b58cd))
* **gateway:** add Unix socket MCP transport ([522a57d](https://github.com/dinglebear-ai/labby/commit/522a57dc3a2af08cc912a31f9f7b0a233ad9832c))
* **incus:** bake operator diagnostics tools ([1840426](https://github.com/dinglebear-ai/labby/commit/1840426bdfcbb5d3e82ca1a57680d41e7ccc263a))
* **mcp:** add optional Code Mode app surface ([#277](https://github.com/dinglebear-ai/labby/issues/277)) ([85af8f1](https://github.com/dinglebear-ai/labby/commit/85af8f1da47685a9b4f22fa709682c31f5efb7e4))
* **mcp:** hash exact peer tool contracts ([d4d49b0](https://github.com/dinglebear-ai/labby/commit/d4d49b01c89c902dd9b7049004056650de47d654))


### Fixed

* **auth:** persist CIMD client references for token issuance ([48cf450](https://github.com/dinglebear-ai/labby/commit/48cf450ef69615de95b219168f736bb613abc867))
* **auth:** preserve Rust 1.90 consumer support ([53dbc47](https://github.com/dinglebear-ai/labby/commit/53dbc4798567f3cacc0f740b3b4de5839ca39133))
* **auth:** stabilize all-features CI contracts ([f64379c](https://github.com/dinglebear-ai/labby/commit/f64379c7fff3d7de3494afbb45da134993f11897))
* **auth:** use aws-lc JWT backend ([ea3d44a](https://github.com/dinglebear-ai/labby/commit/ea3d44a06021fe07211ed16153d6c3da005ab1dd))
* **ci:** build pinned distrobuilder release ([abf0ce2](https://github.com/dinglebear-ai/labby/commit/abf0ce23fcdee7851f3ce3cc5c8f09647bd01cdb))
* **ci:** drop dangling OpenWiki references after the retirement ([d64958d](https://github.com/dinglebear-ai/labby/commit/d64958df84d81233f36ce7756ff0185fced17aa5))
* **ci:** drop ripgrep dependency from the retired-feature guard ([c07d9a5](https://github.com/dinglebear-ai/labby/commit/c07d9a51c39d25c6864d4ae0da0e839e6fdda89b))
* **ci:** enable build caching for the GitHub-hosted Incus image job ([9e6f840](https://github.com/dinglebear-ai/labby/commit/9e6f8403838123038d3701f4f1896340d27e16d3))
* **ci:** install distrobuilder from apt ([a9a3e58](https://github.com/dinglebear-ai/labby/commit/a9a3e58d962416741310d3b14015d81a08189525))
* **ci:** isolate Incus image validation ([dcb39a2](https://github.com/dinglebear-ai/labby/commit/dcb39a21ceb1a9a9d3bc642709259de5f53090d9))
* **ci:** move the Incus image build back to a GitHub-hosted runner ([24cc2ec](https://github.com/dinglebear-ai/labby/commit/24cc2ec15df87c7165311a4c4803dd8b2da8070f))
* **ci:** pin Cargo Deny runner ([5d9bd25](https://github.com/dinglebear-ai/labby/commit/5d9bd255f766ed1f0d062afff49e38058beda94d))
* **ci:** satisfy shellcheck SC2015 in the Incus smoke cleanup trap ([dc5658e](https://github.com/dinglebear-ai/labby/commit/dc5658e5e3ccda2810d7e64d360dd89f8197a2b0))
* **ci:** start Incus daemon before image smoke ([f491aa9](https://github.com/dinglebear-ai/labby/commit/f491aa9ab452169a0fefa0e2f28514504da2dd52))
* **deps:** resolve all open Dependabot security alerts ([dcbfce2](https://github.com/dinglebear-ai/labby/commit/dcbfce2ed51ea0dc293e417dae5da86a90de60af))
* **docs:** resync npm launcher README and drop retired OpenWiki section ([0b3db9f](https://github.com/dinglebear-ai/labby/commit/0b3db9f68481934390b2ad8ee1baf10635b4be7f))
* **gateway:** fall back for misclassified discovery results ([6235d4c](https://github.com/dinglebear-ai/labby/commit/6235d4cc2dc05812c97a04b045fcd202b2c757de))
* **gateway:** harden Unix socket transport ([9cc7879](https://github.com/dinglebear-ai/labby/commit/9cc78790c9113974f347a472c7b0e5828e1fd3b4))
* **gateway:** initialize legacy upstreams deterministically ([c4e2352](https://github.com/dinglebear-ai/labby/commit/c4e2352c6d1e15343fec8188e8e8fe21569fc4fc))
* **gateway:** negotiate upstream lifecycle compatibly ([d95829b](https://github.com/dinglebear-ai/labby/commit/d95829b2d156a4c841a186b770286461f3872b1d))
* **hooks:** move workspace clippy from pre-commit to pre-push ([cba2fcd](https://github.com/dinglebear-ai/labby/commit/cba2fcd1ced916c338512317ef63284a9143fb47))
* **mcp:** address stateless review findings ([e47996e](https://github.com/dinglebear-ai/labby/commit/e47996e20c090127f398bed99f6f9d9bf33a368d))
* **mcp:** preserve legacy lifecycle compatibility ([9aa1f91](https://github.com/dinglebear-ai/labby/commit/9aa1f912bc516678f0cb24386c8abf5b77c81262))
* **test:** remove shared-counter race between destructive dispatch tests ([6ab4d95](https://github.com/dinglebear-ai/labby/commit/6ab4d95758d31c6d90730bd9a672f2d926c91a15))


### Changed

* delete retired Labby product surfaces ([b2b75cc](https://github.com/dinglebear-ai/labby/commit/b2b75cc1166cf007e5a7a4b5a09f0d67eb388097))

## [1.7.0](https://github.com/dinglebear-ai/labby/compare/v1.6.0...v1.7.0) (2026-07-27)


### Added

* **auth:** complete MCP 2026 authorization ([68b4079](https://github.com/dinglebear-ai/labby/commit/68b4079fd2ecf00f17fd56c3dbfe77f3ae2b39de))
* **auth:** make the canonical resource scope vocabulary configurable ([#268](https://github.com/dinglebear-ai/labby/issues/268)) ([bd11914](https://github.com/dinglebear-ai/labby/commit/bd1191416b72ad016b5630470d2868fe7bc993cb))
* **mcp:** align gateway with 2026-07-28 RC ([f076fc9](https://github.com/dinglebear-ai/labby/commit/f076fc9e7b513a0a84a9c30095a756dc8c59a907))
* **mcp:** coalesce catalog notifications and keep them out of open turns ([#267](https://github.com/dinglebear-ai/labby/issues/267)) ([7a76aa6](https://github.com/dinglebear-ai/labby/commit/7a76aa671fe8d7b06e200eeb00e79eb10fbab331)), closes [#261](https://github.com/dinglebear-ai/labby/issues/261)
* **mcp:** evaluate tools/list_changed per peer, not as a broadcast ([#264](https://github.com/dinglebear-ai/labby/issues/264)) ([e617a22](https://github.com/dinglebear-ai/labby/commit/e617a22c35d4ecbed270a8d8501245333dd30b61)), closes [#261](https://github.com/dinglebear-ai/labby/issues/261)
* **mcp:** land compact inspector and private app callbacks ([#272](https://github.com/dinglebear-ai/labby/issues/272)) ([c314f8f](https://github.com/dinglebear-ai/labby/commit/c314f8f88a2ba82834ec9c5b8a20ebe38f48335b))
* **mcp:** migrate to rmcp 3 stateless lifecycle ([41fbdae](https://github.com/dinglebear-ai/labby/commit/41fbdae5f7f38f1446a883f64e6708272921036e))
* **observability:** detect tools/list_changed notification churn ([#262](https://github.com/dinglebear-ai/labby/issues/262)) ([845d59c](https://github.com/dinglebear-ai/labby/commit/845d59c04391f7f4f155a7af28b28c5bff1bf734))
* **setup:** slim and harden Incus provisioning ([db77f71](https://github.com/dinglebear-ai/labby/commit/db77f71c5af2dce2818072e9996fd3be06645a1a))
* **unraid:** add live dashboard widget ([5240bc9](https://github.com/dinglebear-ai/labby/commit/5240bc9472c124445637b7a936fd25a161b59b91))
* **unraid:** polish mobile gateway management ([28fef10](https://github.com/dinglebear-ai/labby/commit/28fef10de84f9ca6e0cf2f95b2d5c65e22200e42))
* **web:** add operational Unraid settings page ([c31ca4d](https://github.com/dinglebear-ai/labby/commit/c31ca4d6002240a8108736905359014a1b9e5b1f))


### Fixed

* **auth:** harden MCP 2026 authorization ([#270](https://github.com/dinglebear-ai/labby/issues/270)) ([c667b3c](https://github.com/dinglebear-ai/labby/commit/c667b3c714c95eee7786ff58f63bad506a2cd88b))
* **auth:** separate browser callback from issuer ([#258](https://github.com/dinglebear-ai/labby/issues/258)) ([4dc5ce6](https://github.com/dinglebear-ai/labby/commit/4dc5ce628077ad71523ca9ece7b141222edba81e))
* **gateway:** bridge legacy upstream MCP lifecycle ([a4a2ada](https://github.com/dinglebear-ai/labby/commit/a4a2ada9750344ce07df9f70b84155bda3f55495))
* **gateway:** cascade upstream removals ([2d518f9](https://github.com/dinglebear-ai/labby/commit/2d518f9e8e160c24e04e2700ae8e1f562126ad92))
* **gateway:** cascade upstream renames into protected routes ([43a1412](https://github.com/dinglebear-ai/labby/commit/43a1412edb39382a117401d1cd0a01d04ea4dccb))
* **gateway:** stop tools/list_changed churn on Code Mode raw tool flapping ([6f81a7c](https://github.com/dinglebear-ai/labby/commit/6f81a7c17785884e508ba2b8aee564a009351a7c))
* **mcp:** prune closed peers so the registry stops growing without bound ([#269](https://github.com/dinglebear-ai/labby/issues/269)) ([23b2afb](https://github.com/dinglebear-ai/labby/commit/23b2afbb559c3a5b97bafc0b239d948e78349fe3))
* **unraid:** align settings shell with source mock ([ce825fe](https://github.com/dinglebear-ai/labby/commit/ce825fe0b636549cbbce5e03d4b9459c6e128a93))
* **unraid:** expose Incus CLI to dashboard status ([b89a6d7](https://github.com/dinglebear-ai/labby/commit/b89a6d7b705e4cddc5b8f302a828a95040cf81eb))
* **unraid:** honor Incus daemon state path ([ebdb087](https://github.com/dinglebear-ai/labby/commit/ebdb087e9aca6e50ba1fb8fa01fd862bcfe0715e))
* **unraid:** preserve xtables extensions in Incus mode ([c20daf4](https://github.com/dinglebear-ai/labby/commit/c20daf45e0d38aa958929e748115d25f28ca4e77))
* **unraid:** prevent mobile gateway row overlap ([05087a5](https://github.com/dinglebear-ai/labby/commit/05087a57cdcf6441ad5f4c4802a5ab3cfcda9ce0))
* **unraid:** run Incus UI commands as numeric user ([d084926](https://github.com/dinglebear-ai/labby/commit/d084926aa1993fe9f727936223aeb304206dcb22))
* **unraid:** ship the real Labby plugin control plane ([49d5707](https://github.com/dinglebear-ai/labby/commit/49d57074f0bf42411e22b75f9e7bb9739b727c56))

## [1.6.0](https://github.com/jmagar/labby/compare/v1.5.0...v1.6.0) (2026-07-17)


### Added

* **mcp:** add gateway upstream status app ([#252](https://github.com/jmagar/labby/issues/252)) ([01c2c4d](https://github.com/jmagar/labby/commit/01c2c4da665941eafdb7e3d06e6f5f25b2256b20))
* **mcp:** add responsive server onboarding app ([#250](https://github.com/jmagar/labby/issues/250)) ([ef49be1](https://github.com/jmagar/labby/commit/ef49be16727daa2b4a173ca642e43544ccd8b03c))


### Fixed

* **gateway:** survive request cancellation during gateway.reload ([4dbe7d8](https://github.com/jmagar/labby/commit/4dbe7d89fdfcebf77d4651d453e5864abf714800))
* **mcp:** advertise labby server identity instead of rmcp defaults ([#249](https://github.com/jmagar/labby/issues/249)) ([05eab05](https://github.com/jmagar/labby/commit/05eab05d68dc440599a57c7caafb4d7c9ca49dee))
* **mcp:** harden gateway apps after review ([#254](https://github.com/jmagar/labby/issues/254)) ([d969456](https://github.com/jmagar/labby/commit/d969456363f9199dc514b5e49dbb5872eb44d9d1))
* **mcp:** notify clients when app tools appear ([#253](https://github.com/jmagar/labby/issues/253)) ([a17fa9e](https://github.com/jmagar/labby/commit/a17fa9e3960c4e8e0c6afceeea9788722ec8a926))

## [1.5.0](https://github.com/jmagar/labby/compare/v1.4.1...v1.5.0) (2026-07-16)


### Added

* **unraid:** add Incus gateway converger ([1b213a1](https://github.com/jmagar/labby/commit/1b213a19b587108c349cb6013c8a0cdaf25a6edc))
* **unraid:** add Incus gateway runtime mode ([b8a9697](https://github.com/jmagar/labby/commit/b8a96976b47f68499ac3d67a598e78fd959c06c4))
* **unraid:** add Incus settings form fields ([0e345db](https://github.com/jmagar/labby/commit/0e345db458113f10f27b0d4941620f2dbf14ca6d))
* **unraid:** branch rc.labby on RUNTIME_MODE (native|incus) ([6678b4d](https://github.com/jmagar/labby/commit/6678b4dc87646849e9a46866f4efd941c3472dbc))
* **unraid:** vendor labby Incus profile and env sourcer ([11c732a](https://github.com/jmagar/labby/commit/11c732aee2981f78078e41ad8fe766f9495bad57))
* **unraid:** wire incus plugin assets ([7f8145e](https://github.com/jmagar/labby/commit/7f8145e5ed7af22c2dff69c7f89c0a5444ea1b0a))


### Fixed

* **auth:** allow legacy ChatGPT OAuth callback ([f6f5ae1](https://github.com/jmagar/labby/commit/f6f5ae15e857c41a7ec171361bef634d127d0727))
* **auth:** allow legacy ChatGPT openai callback ([42da0b2](https://github.com/jmagar/labby/commit/42da0b263818beff4508ceaf5d8baff20ae2e4ec))
* **mcp:** remove non-elicitation destructive gates ([bbac44f](https://github.com/jmagar/labby/commit/bbac44fb4e5b22651b26165bb78ee345e479abbc))
* remediate comprehensive project review ([#248](https://github.com/jmagar/labby/issues/248)) ([e9c6577](https://github.com/jmagar/labby/commit/e9c6577ac310fa65c9e391aca78d88c262cd8006))
* **unraid:** add native gateway management controls ([1332107](https://github.com/jmagar/labby/commit/1332107704d98ac5fb493e2f2f14975d9457bd20))
* **unraid:** bump native controls plugin version ([192a8fb](https://github.com/jmagar/labby/commit/192a8fbfef5ab0d485968f4c6eeb08f468c8bbc0))
* **unraid:** close Incus review gaps ([94a2f8a](https://github.com/jmagar/labby/commit/94a2f8aa383f14003ebe6bf6534ac6213b7b4aa5))
* **unraid:** close Incus runtime review gaps ([a7a5c81](https://github.com/jmagar/labby/commit/a7a5c81517cef2aa96788edc27b60bd300aa7df2))
* **unraid:** drop letter-suffixed plugin versioning, use plain patch bumps ([ffc2088](https://github.com/jmagar/labby/commit/ffc2088b4b769b06dc6376a023836938a4ab3ab2))
* **unraid:** embed gateway admin in plugin page ([875ff06](https://github.com/jmagar/labby/commit/875ff06c3766e0c79963566078f477dd2bf81281))
* **unraid:** fail closed on Incus init state checks ([7f4fa27](https://github.com/jmagar/labby/commit/7f4fa27a89175d9e8e5b663369572f9732d9f0f7))
* **unraid:** harden Incus mode handoffs ([c2547c5](https://github.com/jmagar/labby/commit/c2547c56197f3d700a4766199d749e9d16ae114f))
* **unraid:** harden native gateway controls ([9a66d4b](https://github.com/jmagar/labby/commit/9a66d4b91b4e4ae55128117296b28140c3fd8dea))
* **unraid:** preserve plugin version history ([a0c80e4](https://github.com/jmagar/labby/commit/a0c80e4976b5dc7804d6665ba94766436b7cc1c8))
* **unraid:** propagate Incus state failures during converge ([d3fc6f3](https://github.com/jmagar/labby/commit/d3fc6f30c2e00cfe22827a972f58b3730ee70f29))
* **unraid:** replace gateway iframe with native controls ([aade10b](https://github.com/jmagar/labby/commit/aade10be4b801862ba0c50dfd9ed02d569e8e0e4))
* **unraid:** safely redact one-shot Tailscale key ([1d1800c](https://github.com/jmagar/labby/commit/1d1800c4da8d8270576c8e196d3359add866ab52))
* **unraid:** trust rc.labby's exit code, backup cfg before overwrite, always stop on unmount ([c7c344f](https://github.com/jmagar/labby/commit/c7c344fa638d07be92cd6dd83748aa7550dcef8a))
* **unraid:** use current Incus config-set syntax ([e6adf66](https://github.com/jmagar/labby/commit/e6adf662e12a347648bbd26c8d999c5c289695b3))

## [1.4.1](https://github.com/jmagar/labby/compare/v1.4.0...v1.4.1) (2026-07-15)


### Fixed

* **auth:** add FK constraint on refresh_tokens.client_id, fix EXISTS naming ([#243](https://github.com/jmagar/labby/issues/243)) ([e6edbe5](https://github.com/jmagar/labby/commit/e6edbe58759a814d29ca3c4e2830b1070974ed28))
* **auth:** scope refresh-token existence check to the requesting client ([#242](https://github.com/jmagar/labby/issues/242)) ([84752e7](https://github.com/jmagar/labby/commit/84752e7ca39fe269f63fe62d0ef9e8bbf083a599))
* **codemode:** remove dangling __meta__.upstreams() and duplicate helpers ([#240](https://github.com/jmagar/labby/issues/240)) ([cb7a4a5](https://github.com/jmagar/labby/commit/cb7a4a567c8dc7023dc8e9865b5148e4363e9c03))

## [1.4.0](https://github.com/jmagar/labby/compare/v1.3.1...v1.4.0) (2026-07-14)


### Added

* integrate public OAuth callback relay ([#239](https://github.com/jmagar/labby/issues/239)) ([3a47567](https://github.com/jmagar/labby/commit/3a47567805ae0e11079597e11bea68a3eac2a0c0))


### Fixed

* **ci:** install OpenWiki with npm ([aa7b6c2](https://github.com/jmagar/labby/commit/aa7b6c2453617bc2aea62569e44a012b96d59c23))
* **ci:** repair main CI tool activation and Windows test cfg ([#237](https://github.com/jmagar/labby/issues/237)) ([038e262](https://github.com/jmagar/labby/commit/038e26296cdf104fea51f0a288eba40e1902fd36))
* **ci:** run OpenWiki through mise exec ([e671e17](https://github.com/jmagar/labby/commit/e671e17e169eea4f376a5a9751a4fb2e9d156d66))

## [1.3.1](https://github.com/jmagar/labby/compare/v1.3.0...v1.3.1) (2026-07-13)


### Fixed

* respect dynamic cargo job allocation ([9df9d6d](https://github.com/jmagar/labby/commit/9df9d6d796c31881211e2b86bf632468edc0c498))
* route rust builds through sccache wrapper ([2629696](https://github.com/jmagar/labby/commit/26296961e865214e56b831b7b1e72e3165fa44b4))

## [1.3.0](https://github.com/jmagar/labby/compare/v1.2.0...v1.3.0) (2026-07-12)


### Added

* add Labby operator apps ([efd06ff](https://github.com/jmagar/labby/commit/efd06ff8e20f29a904a638c364869fb14d4d1df9))
* publish labby via npm and MCP registry ([a0db54c](https://github.com/jmagar/labby/commit/a0db54c1ed71573de5c2a7a065b560cdf69a98cd))


### Fixed

* address operator app review findings ([ada96f2](https://github.com/jmagar/labby/commit/ada96f22441311611c89b304ae455b6bb73a4436))
* address pr toolkit review findings ([15c5541](https://github.com/jmagar/labby/commit/15c55412ceb491ed10b1c1077d88f049e75ff6bc))
* **lab-5vssx:** repair Windows npm archive extraction ([#235](https://github.com/jmagar/labby/issues/235)) ([69c1e90](https://github.com/jmagar/labby/commit/69c1e90a2337d520cef9a38b3bf51badf27db4ef))

## [1.2.0](https://github.com/jmagar/labby/compare/v1.1.0...v1.2.0) (2026-07-12)


### Added

* **auth:** support explicit HTTPS DCR callback opt-in ([28718bd](https://github.com/jmagar/labby/commit/28718bd422e05b9a62436df84b2a950a13dddeae))
* **code-mode:** cap inspector body at ~10 rows with internal scroll ([80176d7](https://github.com/jmagar/labby/commit/80176d7eabd7b029685695c944e1c29d018fcbea))
* **code-mode:** expandable tool rows and host-delivered Input row ([f77778c](https://github.com/jmagar/labby/commit/f77778c608d402b854d5a0ed075f5ba9baa90010))
* **code-mode:** inspector enrichment batch — failure traces, waterfall, artifacts, UX polish ([bf75ed4](https://github.com/jmagar/labby/commit/bf75ed4ac034b823b7a586da450ba54dc14820ca))
* **code-mode:** redesign inspector as compact inline widget ([b39faab](https://github.com/jmagar/labby/commit/b39faab4024232139194e0997de3dc0b927d6a4e))
* **code-mode:** render in-sandbox discovery results in the inspector ([916c1b1](https://github.com/jmagar/labby/commit/916c1b1cd9633a50c0a388d5221ba151b07c8dc3))
* **codemode:** notebook-as-log durable step journal (v1, lab-d6ke7) ([#230](https://github.com/jmagar/labby/issues/230)) ([7ff89d3](https://github.com/jmagar/labby/commit/7ff89d32c4e0c7b9feb55833f36bca92352c5bb0))
* enrich code mode inspector app ([58872d5](https://github.com/jmagar/labby/commit/58872d58e97d5e131eff7ee107c130e23877dd93))
* harden palette production flows ([b11e002](https://github.com/jmagar/labby/commit/b11e00289c4ef3a06eeccae747b327e7e7ff1bef))
* **incus:** provision rsync, mise, and chezmoi in the labby image ([d3130df](https://github.com/jmagar/labby/commit/d3130df44cef86a17ffcbd78f2ee9c229e4e484f))
* **incus:** provision rsync, mise, and chezmoi in the labby image ([b80e68f](https://github.com/jmagar/labby/commit/b80e68f516d1ab77d40a36df334ad2d251bd10e9))
* **palette,incus:** palette launcher service + incus setup CLI ([57550aa](https://github.com/jmagar/labby/commit/57550aa25041b49dae1dc98fb5606e0b4c3f9cca))


### Fixed

* address lavra review findings ([c9ed770](https://github.com/jmagar/labby/commit/c9ed770ab137e854be399fdf206c406a0627beb0))
* align dozzle skill drift env flag ([afa553c](https://github.com/jmagar/labby/commit/afa553c6dedcba0d709dae70d0e7e1b2e78de843))
* **code-mode:** drop redundant status indicators from the inspector ([018ce79](https://github.com/jmagar/labby/commit/018ce79f1d5a6bdde07444e462c332ee8a90d4ec))
* harden code mode review findings ([edb2e89](https://github.com/jmagar/labby/commit/edb2e89ff027b25df3d959a6fb07b3041a88d7d1))
* harden code mode review findings ([4a58cce](https://github.com/jmagar/labby/commit/4a58cce9cb7f7a819bb0bd1360cbd4dd285cf45f))
* **mcp:** clarify paginated tool list logging ([7d1b5bb](https://github.com/jmagar/labby/commit/7d1b5bbdeb8235b45ecc945f3bb5c8f67ad97cd1))
* **mcp:** finish pagination collector integration ([2d56246](https://github.com/jmagar/labby/commit/2d56246960da15935013f8d482099e4be9957c8b))
* **mcp:** preserve pagination cursor errors ([f47455d](https://github.com/jmagar/labby/commit/f47455d666471d40246d2e52dd12093e58f5a209))
* quiet palette refresh lint ([3309bb0](https://github.com/jmagar/labby/commit/3309bb0bd364a8b12aa0137157f05a94dcc6ab5d))
* restore incus web asset sync after merge ([afb4c2e](https://github.com/jmagar/labby/commit/afb4c2edf8616bbc8963c68df8107dec4d3b3813))
* satisfy generated docs and clippy gates ([b7ba0ea](https://github.com/jmagar/labby/commit/b7ba0ea2cd3ff3644bee128ead81116ff1269435))
* stabilize gateway admin frontend checks ([aacce4d](https://github.com/jmagar/labby/commit/aacce4db495b08d44c029df2a488f427daa8eb20))
* verify labby gateway daemon before dispatch ([444ab99](https://github.com/jmagar/labby/commit/444ab993851a98ce0ba4a71a66f59e1896409ac8))


### Changed

* **mcp:** add bounded pagination collector ([ee12533](https://github.com/jmagar/labby/commit/ee12533b0a6b2fbafe3798b94dcc17f655472f65))
* **mcp:** add shared catalog notification helper ([8b9031a](https://github.com/jmagar/labby/commit/8b9031a90cf54c94fe39cc1ed039e060444f7359))
* **mcp:** bound resources and prompts page collection ([55d470e](https://github.com/jmagar/labby/commit/55d470e49152e46858ef6246155a3dc114737005))
* **mcp:** bound tools list page collection ([14c9e44](https://github.com/jmagar/labby/commit/14c9e44f57f195ba9a651f7415adbc4892167e93))
* **mcp:** remove stale catalog change helper ([46ec8d1](https://github.com/jmagar/labby/commit/46ec8d158fa8fe57c5eb4fb1bde865a215789456))
* **mcp:** reuse catalog notification fanout for gateway peers ([e5c67e5](https://github.com/jmagar/labby/commit/e5c67e52df6ab946dbb6633cbf07755883a1d1cf))
* **mcp:** reuse catalog notification fanout in server ([6e0ab89](https://github.com/jmagar/labby/commit/6e0ab8933e1168928e6035d802af6d9cea483173))
* **mcp:** share catalog notification fanout ([82132d4](https://github.com/jmagar/labby/commit/82132d49cc30c87ba037d444beb2dfb5181699ba))

## [1.1.0](https://github.com/jmagar/labby/compare/v1.0.1...v1.1.0) (2026-07-09)


### Added

* add Labby desktop palette (Tauri), ported from Axon's palette shell ([44b8a4d](https://github.com/jmagar/labby/commit/44b8a4d5ac6c76f104f99bdc1d45fa4b24b3b98a))
* add unified palette launcher ([#205](https://github.com/jmagar/labby/issues/205)) ([28a7a97](https://github.com/jmagar/labby/commit/28a7a97f335f7fd2b96f265212cc8a040d334e1e))
* **auth:** add native callback/poll OAuth flow for desktop/native clients ([467d632](https://github.com/jmagar/labby/commit/467d6328ed180e70db531d0e04f084b3be788034))


### Fixed

* **auth:** review fixes for native-poll OAuth flow ([8b13f95](https://github.com/jmagar/labby/commit/8b13f959dcac9a392b5b188fbf48a9b6d2996155))
* **gateway-admin:** remove dead-route references missed by static analysis ([af49ecc](https://github.com/jmagar/labby/commit/af49ecc1e003a06730d86da4ab5639a813172857))
* review fixes for the desktop palette client ([5629ad6](https://github.com/jmagar/labby/commit/5629ad6796ffdd9c3f61c2a2363f96de9f5195f3))

## [1.0.1](https://github.com/jmagar/labby/compare/v1.0.0...v1.0.1) (2026-07-09)


### Fixed

* **ci:** drop invalid --generate-notes flag from gh release edit ([45c2079](https://github.com/jmagar/labby/commit/45c2079f5dd7ab60b9642964af7938ad9885c6d9))

## [1.0.0](https://github.com/jmagar/labby/compare/v0.29.0...v1.0.0) (2026-07-09)


### ⚠ BREAKING CHANGES

* rename all LAB_* env vars to LABBY_* (hard break, no aliases)

### Added

* **codemode:** add batch helper ([0cdc7af](https://github.com/jmagar/labby/commit/0cdc7afb3511a0906969c255fa1552c01846fd0a))
* **codemode:** semantic search blend for codemode.search() ([#172](https://github.com/jmagar/labby/issues/172)) ([0ba290d](https://github.com/jmagar/labby/commit/0ba290dc2fc0b59768d80db5f95a0979be7b98dd))
* **config:** add [code_mode] fields for 5 previously env-only artifact/call-budget prefs ([4066c21](https://github.com/jmagar/labby/commit/4066c217543bc893affb7e570e0d4fd2c18b72ad))
* **config:** add [gateway] fields for 4 previously env-only upstream/discovery prefs ([2a849f2](https://github.com/jmagar/labby/commit/2a849f27f624e49952be7ca88176c50e24ee065f))
* **config:** add config.toml fields for 7 previously env-only preferences ([496b65b](https://github.com/jmagar/labby/commit/496b65b82d8de1ce44d747f50628c9920dd3d341))
* **config:** wire [auth] rate-limit fields through resolve_auth() ([95aa488](https://github.com/jmagar/labby/commit/95aa48844a4f3d38174678c70700cb032a8bd5ca))
* gate base services (stash, acp, nodes) for gateway-only builds ([#171](https://github.com/jmagar/labby/issues/171)) ([30132cc](https://github.com/jmagar/labby/commit/30132cc8a2eff161ffc333e3c6fbe4c645aea4dd))
* **lab-jwbkn.5:** add real build-timing measurement to bench-labby-slimming ([39d574e](https://github.com/jmagar/labby/commit/39d574ef71258df919cec6a380df377d9ce104e1))
* **lab-jwbkn.6:** swap reqwest's TLS crypto backend from aws-lc-rs to ring ([0182a3b](https://github.com/jmagar/labby/commit/0182a3b7af2c4e8f8197a190d70212486c215ddc))
* **labby-auth:** auto-allow native-app OAuth redirect URI schemes ([8c6b9fa](https://github.com/jmagar/labby/commit/8c6b9fac48745649d63e6b3ad059f695078332f6))
* **openapi:** build registry + hardened client at startup and inject via required host accessors ([2478d8b](https://github.com/jmagar/labby/commit/2478d8b08a994ce35739cee7b82202788e56a67a))
* **openapi:** config types (secret-redacted) + loader with mandatory base_url and label validation ([1e766e4](https://github.com/jmagar/labby/commit/1e766e41951b0fbfa2a42887da220c422e356cbe))
* **openapi:** const globalThis.openapi.call shim emitted on the host path only ([f08a51c](https://github.com/jmagar/labby/commit/f08a51cba4a4ac48790b5e88ee27cb04d4891330))
* **openapi:** hardened own HTTP client (redirect-off, peer-IP recheck) + dispatch with credential injection + canary leak test ([b6c694d](https://github.com/jmagar/labby/commit/b6c694d7c0dd7fc76f7a510a8abb9de7281db82c))
* **openapi:** load-time base-URL SSRF validation via canonical labby_primitives::ssrf ([5f29d16](https://github.com/jmagar/labby/commit/5f29d1679155d0a6af8a999cd365531228abb273))
* **openapi:** LocalProviderName::Openapi + dotted-op ID parsing; wire dispatch via pre-lock branch + required host accessors ([0cd5ca4](https://github.com/jmagar/labby/commit/0cd5ca4074d74d0dd41984b9b5dcf66becfc3652))
* **openapi:** real ToolError mapping + parse-only spec-to-descriptor conversion with raw-id allowlist ([0ee2f57](https://github.com/jmagar/labby/commit/0ee2f571f98b062110872d3a2f368a50e83aac41))
* **openapi:** registry with concurrent timeout-bounded degraded-boot load + spec body-size cap + truncation warnings ([574d780](https://github.com/jmagar/labby/commit/574d78075eca2c248cf904174b9bbd6eb62e0466))
* **openapi:** scaffold isolated labby-openapi crate (parse-only rmcp-openapi + own HTTP) ([58e2bb5](https://github.com/jmagar/labby/commit/58e2bb59a6374a36ac34b0c57c2825d3be55f3d8))
* rename all LAB_* env vars to LABBY_* (hard break, no aliases) ([f3bb785](https://github.com/jmagar/labby/commit/f3bb7855e19f0a499e96ca1c9f601fae8246a7a1))
* **ui:** hide feature-gated services absent from this build (lab-x4mw2) ([#180](https://github.com/jmagar/labby/issues/180)) ([97244c0](https://github.com/jmagar/labby/commit/97244c06a1e8a7510114a838f962a72ab73bfaa1))


### Fixed

* canonical example env var name + accurate labby-auth error prefixes ([ae45de3](https://github.com/jmagar/labby/commit/ae45de36de9301a3d39d8e25d5c3d973b4432c5f))
* CI failures and review findings from PR [#170](https://github.com/jmagar/labby/issues/170) ([35a42eb](https://github.com/jmagar/labby/commit/35a42eb38ece52d531bfdddcb3026864ffa080b6))
* CI matrix drift, fs-slice compile error, and normalize prologue bug ([f42004b](https://github.com/jmagar/labby/commit/f42004b041bbb19fa8983c88de69c89852ed5189))
* **ci:** repair release-please manifest and switch OpenWiki to local proxy ([9bef3f0](https://github.com/jmagar/labby/commit/9bef3f0f628a747837f581fb96fcbf2610cf6d37))
* **deps:** address Dependabot advisories ([9a05409](https://github.com/jmagar/labby/commit/9a0540984c17db17b0263391c405ed06049558f1))
* **fmt:** reorder use statement in config.rs ([1471646](https://github.com/jmagar/labby/commit/1471646882987cc7ff642a04995343a2ae569e19))
* **incus:** su - lab → su - labby in bootstrap hint and smoke test ([#179](https://github.com/jmagar/labby/issues/179)) ([def7235](https://github.com/jmagar/labby/commit/def723541ac39ee91a9adc093d276f517046c705))
* **lab-jwbkn.2:** stop find -empty from deleting the just-created SCCACHE_DIR ([57f58fe](https://github.com/jmagar/labby/commit/57f58fe65e633e917235969e32e2e62e153f14f5))
* **labby-auth:** circuit-break upstream OAuth refresh after a failure ([8209188](https://github.com/jmagar/labby/commit/8209188ccf92d05f3eb48090017723498ea6a0e5))
* **labby-auth:** log refresh outcome on the live upstream OAuth pool path ([cf40fea](https://github.com/jmagar/labby/commit/cf40fea271a84ccdbc3bbd72755121891929aba8))
* **labby-auth:** skip forced re-consent once a refresh token exists ([ee00416](https://github.com/jmagar/labby/commit/ee004161ab08397656e0c8fd9f0dc38a6b3271ed))
* **labby:** forward everything the bridge's Peer&lt;RoleClient&gt; can forward ([6996d4a](https://github.com/jmagar/labby/commit/6996d4a50c8b8992577abbcf8affd4a6cbd6768c))
* **labby:** forward ping, task management, and custom requests through the MCP bridge ([7611351](https://github.com/jmagar/labby/commit/7611351f6dfc15a51aebceec0581da74eeefcb06))
* **labby:** lazy CLI manager construction + full stdio MCP bridge to the live daemon ([a30ed49](https://github.com/jmagar/labby/commit/a30ed49343fd51a330e057392bcf8dfc9aee111a))
* **labby:** mirror the daemon's real capabilities and relay elicitation/sampling/roots ([af8c20f](https://github.com/jmagar/labby/commit/af8c20f97a05f0b55699fb761f1d89dbf0944263))
* **labby:** remove Code Mode pause gate and HTTP confirm gate ([50f4dae](https://github.com/jmagar/labby/commit/50f4dae282fcbd5dc72208bde69c5fd6e1bbf09f))
* **labby:** remove Code Mode pause gate and HTTP confirm gate ([e357519](https://github.com/jmagar/labby/commit/e357519385e131c2651b3c46f8390506525ca769))
* **labby:** route gateway CLI commands through the live daemon when one is running ([502418d](https://github.com/jmagar/labby/commit/502418d03739acbf99d7f185f583e089aafab4db))
* **labby:** widen live-daemon detection to the gateway's own public URLs ([413a104](https://github.com/jmagar/labby/commit/413a1046612e0825dad595c5b4246f0b1a1a0765))
* **openapi:** address CodeRabbit review (label charset + spec-url SSRF) ([1f983ca](https://github.com/jmagar/labby/commit/1f983cae1b1ee28287e6405a7a7804d6e403155a))
* **openapi:** address final lavra-review findings (P3 robustness) ([f96ddc2](https://github.com/jmagar/labby/commit/f96ddc2ac5c9ccf511e30b3129e8447eb4c30652))
* **openapi:** address lavra-review findings ([334d657](https://github.com/jmagar/labby/commit/334d65721e15b8d51f88d4121cf63384f3612bfa))
* **openapi:** CI green + second-review-pass cleanups ([1399aa1](https://github.com/jmagar/labby/commit/1399aa15fa01384fb6a4b06a05a7982cf52b33ba))
* **release:** switch release-type to simple; add Cargo.toml/lock sync job ([8a3f580](https://github.com/jmagar/labby/commit/8a3f580903211c575bfe9be48bee13bfc4104287))
* restore missing lab_admin_enabled import in registry tests ([d65183e](https://github.com/jmagar/labby/commit/d65183e93bdbfd414bd1dca08e8c55628d5408d4))
* **setup:** pin uv provision CWD to labby home + UV_NO_CONFIG ([#177](https://github.com/jmagar/labby/issues/177)) ([52a2e89](https://github.com/jmagar/labby/commit/52a2e891905f8dda182190ff827a866621e8c736))
* sync Cargo.lock labby-primitives to 0.30.0 (unblock --locked CI after [#175](https://github.com/jmagar/labby/issues/175)) ([#176](https://github.com/jmagar/labby/issues/176)) ([c633a5e](https://github.com/jmagar/labby/commit/c633a5e5e013776b32e0c5f8973e93c8ef563c02))
* **xtask:** send Code Mode runner timeout ([201e368](https://github.com/jmagar/labby/commit/201e3687ec65294548f15b82031f6c922204ebf6))


### Changed

* **lab-jwbkn.3:** migrate config_store.rs off legacy .env write path ([b5329ba](https://github.com/jmagar/labby/commit/b5329baee29b481c465bbc86dc3a6314da9f9a28))
* **labby:** drop unused ExecCtx.execution_id, fix stale comment ([9a15c6c](https://github.com/jmagar/labby/commit/9a15c6cf41271ff4474300231105585ecf262dad))

## [Unreleased]

---

## [0.30.0] - 2026-07-02

### Added

- **Incus primary deployment hardening** — added top-level `labby incus setup`, `labby incus sync`, and `labby update` flows, CI image build support, baked gateway runtime dependencies, and a supported AppArmor signal rule so systemd service stops no longer require routine force-restart fallback.

### Changed

- **Labby runtime clean break** — the supported runtime now uses `~/.labby`, `~/.config/labby`, the `labby` container user, and `/home/labby` only; legacy `~/.lab`, `/home/lab`, and old registry namespace compatibility are intentionally removed.

### Fixed

- **Code Mode and dashboard telemetry** — Code Mode usage surfaces now report named child tool calls instead of counting internal `gateway`, `logs`, or `code_mode` dispatch wrappers as top tools.
- **MCP UI widget clipping** — the embedded Code Mode MCP UI no longer clamps its content height or hides overflow inside Claude Desktop.

---

## [0.29.0] - 2026-07-01

### Added

- **Code Mode local state and git providers** — added V1 `state.*` workspace
  APIs and local-only `git.*` commands inside the Code Mode sandbox, backed by a
  jailed `$LABBY_HOME/code-mode-workspaces/` workspace with path, quota, output,
  and git process guards.
- **Code Mode state/git V2** — expanded local Code Mode workspace APIs with
  safe filesystem mutation helpers, JSON/hash/detect/archive helpers, guarded
  git branch/remote commands, and explicit unauthenticated remote git operations.
- **Binary-owned Incus bootstrap** — bare `labby setup` now materializes the
  supported Incus profile, snapshot policy, and installer artifacts from the
  binary and converges the gateway container without requiring a source checkout
  or manual release-version lookup. The old web setup flow remains available as
  `labby setup wizard`, while `labby setup incus` keeps advanced bootstrap flags.

### Fixed

- **Incus Tailscale hostname stability** — Incus bootstrap now sets the
  container hostname and passes an explicit Tailscale hostname during join so
  images built on CI runners do not register random runner names in the tailnet.

---

## [0.28.0] - 2026-06-27

### Added

- **Incus gateway deployment artifact** — added a standalone Incus profile YAML
  and a host bootstrap script that consumes it, validates the Incus substrate,
  pushes the selected Labby binary, provisions the container with
  `labby setup --provision`, and optionally joins Tailscale inside the container.
- **Incus runtime docs and reference capture** — documented the supported Ubuntu
  24.04 system-container shape, system service model, provisioning boundaries,
  rollback steps, and saved the reviewed Incus jail article as a local reference.

### Changed

- **Self-hosted gateway default** — the documented gateway runtime now favors an
  Incus system container with the hardened `/etc/systemd/system/labby.service`
  system unit. Docker Compose remains available for explicit dev-container and
  image-smoke paths.
- **Installer fallback contract** — source builds are now opt-in with
  `LAB_ALLOW_SOURCE_FALLBACK=1` / `--allow-source-fallback`; unsupported release
  platforms fail clearly instead of implying a hidden fallback.

### Fixed

- **Incus bootstrap hardening** — TUN validation now accepts valid `/dev/net/tun`
  passthrough devices without reading an invalid `type` config key, and
  Tailscale auth-key cleanup can no longer mask the `tailscale up` exit status.
- **Existing Incus container convergence** — bootstrapping an existing container
  whose root disk uses a different Incus storage pool now derives and attaches a
  rootless runtime profile instead of trying to replace the immutable root disk.
- **Provisioning installer hardening** — the `uv` installer is downloaded to a
  checked temporary file before execution instead of piping directly into `sh`.
- **Gateway admin protected routes** — the UI no longer falls back to
  `mcp.example.com`; protected-route saves now fail closed until
  `NEXT_PUBLIC_PROTECTED_MCP_HOST` is configured.
- **Labby runtime path hard break** — runtime state now uses `~/.labby`,
  `~/.config/labby`, and the `labby` Incus user/home only; old Lab path and
  metadata namespace compatibility is intentionally removed.

---

## [0.27.0] - 2026-06-25

### Added

- **Code Mode upstream namespace context** — the synthetic `codemode` MCP tool
  description now includes the current enabled, route-visible upstream namespace
  names, plus top-level `upstreams` and `tools` schema inputs so agents can
  scope runs without inventing sandbox-only knobs.
- **Configurable upstream relay timeout** — new `upstream_relay_timeout_ms` gateway setting (default 5 minutes) bounds relayed upstream tool calls on the opt-in `LAB_UPSTREAM_RELAY_ELICITATION` elicitation-relay path. Exposed in the settings registry and `config get`, and folded into the pool-rebuild fingerprint so a reload applies it.

### Removed

- **Bundled plugin marketplace** — Lab no longer ships its own plugin marketplace or the `labby marketplace generate` command that produced it. The marketplace moved to the dedicated [dendrite](https://github.com/jmagar/dendrite) repo, decoupling it from this Rust workspace. Removed the 23 migrated plugin directories (keeping `plugins/labby` and `plugins/scripts`), both the Claude (`.claude-plugin/marketplace.json`) and Codex (`.agents/plugins/marketplace.json`) catalogs, and the `marketplace` Justfile recipe. The marketplace browse/install dispatch service (`sources.*`, `plugins.*`, `plugin.install`, MCP Registry, ACP agents) is unchanged.

### Fixed

- **Relayed elicitations are no longer aborted mid-dialog** — relayed upstream calls now use the dedicated relay timeout (default 5 minutes) instead of the 30s `upstream_request_timeout_ms`, so an upstream elicitation forwarded to the downstream agent is not killed while a human is answering it.
- **Upstream relay connections are isolated per OAuth subject** — the relay connection cache is now keyed by `(upstream, session_id, subject)` instead of `(upstream, session_id)`, so a dedicated connection authenticated as one identity can never be reused for a call made as another within the same session.
- **Relayed calls now feed the circuit breaker and emit request logs** — `call_tool_relayed` records success/failure into the upstream circuit breaker and emits `request.start`/`finish`/`error` events like the pooled path, so a wedged relayed upstream (especially on the subject-scoped branch, which previously recorded nothing) is excluded and observable. Connect failures are no longer double-counted by the raw proxy branch.
- **Relay capability diagnostics** — `RelayClientHandler::get_info` now warns (was debug, below the default log level) when the downstream peer info is unavailable rather than silently advertising no server→client capabilities, and the relay's `call_tool`-only scope is documented.

---

## [0.26.1] - 2026-06-17

### Fixed

- **Dependabot npm alerts** — patched vulnerable transitive dependencies in the root npm lockfile and gateway-admin pnpm lockfile by overriding `hono` to `4.12.25`, `js-yaml` to `4.2.0`, `@babel/core` to `7.29.6`, `dompurify` to `3.4.9`, `ws` to `8.21.0`, and `brace-expansion` to `5.0.6`.

---

## [0.26.0] - 2026-06-17

### Added

- **Fast Labby container sync** — added a `release-fast` profile plus `just sync-container` / `just container-sync` to rebuild only stale local binaries, update the host/container-bound `labby`, rebuild the dev image only when runtime inputs changed, and restart the Labby container.
- **Code Mode returned-value inspection** — execute traces now carry returned values through structured content and the gateway-admin inspector renders returned values directly, while search traces keep the full structured result available for agents.

### Changed

- **Gateway CLI internals** — split the gateway CLI implementation into focused `args`, `code`, `dispatch`, `list`, and `oauth` modules while preserving the public command surface.

---

## [0.25.0] - 2026-06-13

### Added

- **Snippets as a first-class workflow** — added schema-backed snippet dispatch, CLI/API/MCP surfaces, generated docs, and a gateway-admin sidebar entry so built-in snippets can be listed, inspected, tested, and run from shared dispatch rather than one-off shell glue.

### Changed

- **Built-in snippet docs** — expanded the four built-in snippets into more practical operator examples with clearer inputs, tool selections, and validation expectations.
- **Setup draft handling** — stale setup drafts now expose entry counts and mtimes, can be discarded from both UI and `labby setup draft discard`, and no longer present the old conflict warning after the draft is removed.

### Fixed

- **Code Mode MCP App callback — destructive bypass hardening** — the widget callback gate now fails closed when a requested tool name matches more than one allowed upstream (returns `ambiguous_tool` instead of proxying an arbitrary, hash-order-dependent upstream). This closes a hole where a destructive sibling tool exposed by multiple UI-bearing upstreams could be invoked unconfirmed because the multi-candidate case skipped the destructive gate. The three callback routes (legacy bypass, direct UI tool, hidden sibling) are now modeled as a single `CallbackDecision` so the destructive check always runs on the exact resolved tool.

---

## [0.24.1] - 2026-06-12

### Fixed

- **Code Mode MCP App callbacks** — MCP Apps rendered from an exposed upstream UI tool can now call exposed sibling tools from the same upstream through `callServerTool` while ordinary raw tools remain hidden from `list_tools`. Destructive sibling tools still require the `execute` confirmation path.

### Changed

- **Vibin repo-status skill** — moved the repo readiness audit skill into the Vibin plugin and refreshed Claude/Codex marketplace metadata so the skill ships with the plugin bundle.

---

## [0.24.1] - 2026-06-13

### Fixed

- Hardened schema-backed settings writes after PR review: env-shadowed config fields now use the target `.env` file and process env consistently, generated OpenAPI documents settings update arrays and destructive `confirm`, and invalid numeric UI input no longer turns into an accidental unset.

---

## [0.24.0] - 2026-06-11

### Added

- **First-run `labby serve` self-bootstrap** — when no MCP token is configured
  (`LAB_MCP_HTTP_TOKEN` unset and `LAB_AUTH_MODE` != `oauth`), `labby serve`
  generates a 64-char hex bearer token and writes a minimal `~/.labby/.env`
  (token + loopback MCP defaults via the atomic `env_merge` path), then prints
  the token and the `http://<host>:<port>/setup` URL once, so a fresh headless
  install is reachable without hand-editing config. A new non-destructive
  `setup.bootstrap` dispatch action backs it.

---

## [0.23.1] - 2026-06-10

### Fixed

- **Windows: Job Object reaping for stdio upstream children and Code Mode runner** ([lab-jouhb]).
  Two spawn sites previously left grandchildren (`cmd → node`, `npx → python`, etc.) orphaned on
  Windows when an upstream was killed or reloaded, because only the direct child PID was killed:

  - `connect_stdio.rs` — a new `#[cfg(windows)]` `JobObjectGuard` is armed immediately after
    spawn (mirroring the Unix `ProcessGroupGuard`), disarmed on successful `UpstreamConnection`
    construction, and the raw Job Object `HANDLE` is stored in `UpstreamRuntimeMetadata.job_handle`.
    `UpstreamConnection::Drop` and `shutdown()` close the handle, causing the OS to terminate the
    entire descendant tree via `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.

  - `runner_drive.rs` — a `_runner_job_guard` (also `JobObjectGuard`) is held for the lifetime of
    `run_in_runner_with_config`, covering all exit paths (completion, timeout, protocol error). The
    existing `terminate_code_mode_runner` direct-child kill is retained as belt-and-suspenders.

  The Unix path (`ProcessGroup::leader()` / `ProcessGroupGuard` / `killpg`) is byte-for-byte
  unchanged. All new code is additive under `#[cfg(windows)]`. A `#[ignore]` integration test
  (`tests/windows_job_object_reaping.rs`) is included for verification on the `windows-lab`
  self-hosted CI runner.

  The raw `windows-sys` Job Object FFI lives in a **new `lab-winjob` crate** — the sanctioned
  unsafe boundary — which exposes a safe API (`create_job_for_pid`, `close_job`). This mirrors
  how the Unix path routes its unsafe through the external `nix` crate: `lab` and `lab-apis`
  keep the workspace-wide `unsafe_code = "forbid"` lint and contain zero `unsafe`. The job
  handle is carried as `isize` (not `windows-sys 0.59`'s `!Send + !Sync` `HANDLE`) so storing
  it in `AppState` does not break the axum router's `Send`/`Sync` bounds.

---

## [0.24.0] - 2026-06-11

### Highlights

- **Gateway usage dashboard** (gateway-admin) — the operator landing page is now a usage dashboard: live fleet + windowed usage in one compact 8-up stat row (1h/24h/7d), tool-call volume + top-tools charts, most-active (agent/device/IP facets), Code Mode fan-out, least-used, and an analytics band (latency p50/p95/p99, failures-by-kind, calls-by-surface, tokens-by-tool, top-upstreams, throughput, activity-by-hour heat). Drill-downs: tool + agent detail drawers and a filterable `/usage` call explorer. Dismissable warning banner, squared/uniform Aurora radii (retuned `--radius` tokens to 6/8/10), light-mode-safe toggles, loading + error/retry states.
- **Usage-metrics backend** — the gateway aggregates real usage from its own dispatch logs:
  - Estimated `input_tokens` / `output_tokens` logged on every MCP + API dispatch-completion event (estimators moved to the shared `dispatch::helpers` leaf).
  - `logs.metrics` — rolling-window aggregation: tool calls, tokens, latency percentiles, failures by kind, surfaces, tokens-by-tool, upstreams, throughput, hourly, timeseries.
  - `logs.tool_detail` / `logs.agent_detail` / `logs.calls` — drill-down metrics + a filterable, paginated call log for the drawers and explorer.
  - All reachable via the existing `/logs` route; the frontend consumes them once `NEXT_PUBLIC_MOCK_DATA` is off.

### Notes

- The dashboard ships against mock data by default; the backend endpoints are implemented and unit-tested but **not yet verified end-to-end against a live gateway**.
- Tier-2 telemetry still pending (best-effort/empty for now): source IP per call, agent-vs-device classification, Code-Mode truncation/artifact counts, new-vs-returning agents.

| Commit | Change |
|--------|--------|
| *(this)* | chore(release): v0.24.0 — usage dashboard + metrics endpoints, session log |
| `2f2bf442` | feat(lab): logs drill-down endpoints — tool_detail, agent_detail, calls |
| `d705adf1` | feat(lab): logs.metrics — usage aggregation endpoint for the dashboard |
| `05fe1399` | feat(lab): log estimated tokens on MCP + API dispatch events |
| `727fbcd3` | feat(gateway-admin): usage dashboard for the gateway overview (mock) |
| `dc3c84de` | style: fix rustfmt 1.94.1 drift in 3 dispatch files |
| `9d387a92` | docs: save session log |
| `9a55ed40` | docs(plan): setup-wizard consolidation plan + eng-review fixes |
| `c64099ef` | ci: run Windows test on same-repo PRs (still skips forks) |
| `57a15d5a` | docs: document [gateway] spawn-guard config knobs |

---

## [0.23.0] - 2026-06-10

### Highlights

- **Gateway comprehensive review landed** (PR #106) — all Critical/High findings plus ~50 Medium/Low: stdio spawn hardening (`env_clear` + allowlist + eval-flag denials), 0o600 secret perms with self-heal, catalog-driven admin scope, OAuth connection caching (kills the N+1), diff-based pool reload, `GatewayManager` split into 13 modules, fuel-vocabulary purge, gitleaks CI, real `/ready`.
- **Upstream stderr forwarding hardened and configurable** (PR #102) — DEBUG default with `LAB_GW_UPSTREAM_STDERR` level control, line/rate caps, redaction, UTF-8-safe truncation.
- **The labby plugin no longer ships a binary** (PR #107) — plugin is skills + MCP config only; hooks are advisory PATH-based shims. Install explicitly via `scripts/install.sh` (release download, sha256-verified, cargo fallback), then `labby setup`. Marketplace generator de-bundled (`--binary` flag removed; generated plugins invoke `labby` from PATH).
- **CI**: Windows release smoke skipped on PRs (runs on main/schedule/dispatch); container smoke job polls `/health`; ACP adapter versions pinned exactly; `RUSTC_WRAPPER` disabled in CI (fixes Windows builds).
- **OAuth**: Google split token-endpoint origin (`accounts.google.com` → `oauth2.googleapis.com`) allowed under strict issuer binding.

### Breaking

- `labby marketplace generate` no longer accepts `--binary`; generated service plugins invoke `labby` from `PATH` instead of a bundled `lab-core/bin/labby`.
- Stdio upstream children run with `env_clear` + allowlist: upstreams needing env vars outside `STDIO_ENV_ALLOWLIST` must declare them in the upstream `env` map.

---

## [0.22.2] - 2026-06-06

### Highlights

- **Production Synapse2 workspace mount** — bind-mount `/workspace/synapse2`
  into the production Labby container so gateway config entries that reference
  the local Synapse2 MCP workspace resolve inside the runtime.

| Commit | Change |
|--------|--------|
| *(this)* | fix(compose): mount Synapse2 workspace in production Labby container |

---

## [0.22.1] - 2026-06-04

### Highlights

- **Skill quality sweep** — systematic review and update of ~70 SKILL.md files across all lab plugins and 10 rmcp workspace repos; fixes include stale script paths, async-trait contradiction in acp/rust, wrong rmcp version in using-rmcp, coercive block removal from paperless-ngx, full rewrites of rarcane and exampleclient skills, and description rewrites to third-person trigger form for all 9 cortex sub-skills.
- **Gateway-admin dialog consolidation** — removed stale `delete-gateway-dialog.tsx`, `disable-gateway-dialog.tsx`, and `confirm-dialog.tsx`; refactored `gateway-detail-content`, `gateway-list-content`, `allowed-users-panel`, `app-command-palette`, `session-sidebar`, and `plugin-detail-content` components.
- **Marketplace silent-error surfacing** — fixed `installPath` handling; surface silent errors in `acp_dispatch`, `claude`, `mcp_dispatch` backends.

| Commit | Change |
|--------|--------|
| *(this)* | docs(skills): skill quality sweep — fix paths, descriptions, content gaps |
| `40e2087e` | docs: save session log |
| `c7db07a6` | fix(marketplace): surface silent errors and fix installPath handling |

---

## [0.22.0] - 2026-06-04

### Highlights

- **Stash export/service expanded** — new export capabilities and service layer additions (+224/+114 lines) covering additional export formats and service operations.
- **Path safety hardened** — `path_safety.rs` significantly expanded (+263 lines) with additional canonicalization and rejection helpers for system path validation.
- **Config loading simplified** — `config.rs` stripped down (-269 lines), removing dead branches and streamlining the load path.
- **Gateway manager refactored** — `manager.rs` reworked (+181/-? lines) for cleaner state transitions and lifecycle handling.
- **Gateway-admin code-mode toggle** — new `code-mode-toggle.tsx` component added to gateway-admin frontend; stale `tool-search-toggle.tsx` removed.
- **ACP security hardening** — constant-time HMAC verification in `persistence.rs`; 5 IDOR principal-isolation tests added covering `session.get`, `session.events`, `session.prompt`, `session.cancel`, `session.subscribe_ticket`.
- **Marketplace plugins updated** — nvidia-skills, qdrant-skills, redis-development, mcp-apps source corrections across Claude and Codex plugin manifests.

| Commit | Change |
|--------|--------|
| `091b1c3d` | docs: save session log |
| `ca834476` | test(acp): add principal-isolation (IDOR) tests for session actions |
| `bb675cb8` | fix(plugins): clean up agent-workstation README and notebooklm SKILL.md |
| `2c139784` | fix(acp): constant-time HMAC verify in persistence + rustfmt cleanup |
| `5b2b3150` | Fix nvidia-skills, qdrant-skills, redis-development, mcp-apps with correct sources |
| `b43d21fe` | Remove nvidia-skills, qdrant-skills, redis-development, mcp-apps — paths not found |
| `5b778acc` | Add nvidia-skills, qdrant-skills, redis-development, mcp-apps, mcp-tunnels to marketplaces |
| `4ae079d5` | docs: final skill quality fixes |
| `21f7e1c0` | docs: update examplerequests skill references |
| `cad9cb67` | docs: normalize skill triggers |
| `f6bf24d9` | docs: refresh plugin skills |
| `88de5a1b` | Add beads, lavra, superpowers to Codex marketplace |
| `c77017a4` | Add Codex plugin manifests |
| `96cbda1c` | Allow no-auth HTTP gateway upstreams |
| *(this)* | feat: stash export expansion, path_safety hardening, gateway-admin code-mode toggle, config simplification |

---

## [0.21.3] - 2026-06-01

### Highlights

- **Stopped `-32601` capability-absence noise from poisoning health state** —
  upstreams that don't implement `prompts/list` or `resources/list` return
  JSON-RPC `-32601 Method not found`. The gateway was logging that at WARN once
  per upstream on every catalog refresh *and* recording a circuit-breaker
  failure each time. Now `is_capability_unsupported` matches the structured
  `ErrorCode::METHOD_NOT_FOUND` (string fallback retained), logs at DEBUG, and
  records success — so a server merely lacking a capability no longer accrues
  phantom failures. Other errors still WARN + count as failures.
- **Namespaced upstream prompts to end silent collisions** — two upstreams both
  exposing a `quick_start` prompt previously dropped one. Upstream prompts now
  carry an `{upstream}/{name}` prefix (mirroring the resource-URI convention),
  with a symmetric strip on `prompts/get` (standard + OAuth subject-scoped
  paths) so upstreams still receive bare names. The collision now surfaces as
  e.g. `exampleclient/quick_start` and `exampleseries/quick_start`.

| Commit | Change |
|--------|--------|
| *(this)* | fix(gateway): demote -32601 capability-absence to debug + fix breaker accounting; namespace upstream prompts |

---

## [0.21.2] - 2026-06-01

### Highlights

- **Code Mode serves the full upstream catalog (no truncation)** — removed the
  256 KB soft-cap drop loop and 512 KB hard-cap error that silently dropped
  tools from the Code Mode `search` inline catalog. Healthy, callable upstreams
  (e.g. `cortex`) were being dropped from discovery purely because the serialized
  catalog exceeded the cap. The catalog is injected into the Boa sandbox and
  never enters model context, so it is now served complete and uncapped —
  matching Cloudflare's Code Mode design.
- **Removed dead `scout` vocabulary** — deleted the `gateway.scout.get/set`
  action aliases (exact duplicates of `gateway.code_mode.*`), the stale
  `[scout]` config-key allowlist entry, the dead truncation hint strings, and
  `scout` mentions in live docs (`GATEWAY.md`, `CONFIG.md`, the using-lab-cli
  catalog). No behavior change — the CLI now emits `gateway.code_mode.*`.

| Commit | Change |
|--------|--------|
| *(this)* | fix(gateway): serve full Code Mode catalog; drop truncation + dead scout vocabulary |
| 72c420b6 | fix(plugin): deliver the labby binary in plugins/labby + add 'setup install' |
| cb636bad | docs: save session log |

---

## [0.21.1] - 2026-06-01

### Highlights

- **Trimmed gateway `search` tool description** — dropped the "No embedding
  model, no vector DB" marketing filler from the Code Mode description in
  `mcp/server.rs` and the matching `code_mode.rs` doc comment.
- **Code Mode admin callers share one gateway OAuth subject** — admin Code Mode
  callers collapse to a single shared gateway OAuth subject.
- **Plugin/marketplace housekeeping** — vibin skills reorganized into top-level
  plugins, redundant manifest `hooks` field dropped, broadcastr binary tracked
  via Git LFS.

| Commit | Change |
|--------|--------|
| *(this)* | chore: trim gateway search tool description |
| 9da2c310 | chore(plugin): track broadcastr binary via Git LFS |
| 3b567e06 | chore(plugin): drop redundant hooks field from manifest |
| 14b1fadf | chore(plugins): reorganize vibin skills into top-level plugins; update marketplace |
| 9a6fbf59 | fix(gateway): collapse admin Code Mode callers to shared gateway OAuth subject |
| 8e4333de | docs: save session log |

### Version bumps

- Rust workspace: `0.21.0 -> 0.21.1`
- Gateway admin package: `0.21.0 -> 0.21.1`

---

## [0.21.0] - 2026-05-31

### Highlights

- **Code Mode normalization now follows Cloudflare's AST behavior** — snippets are
  parsed structurally so trailing expressions, named functions, export defaults,
  arrows, classes, and parse fallback cases are wrapped consistently before
  sandbox execution.
- **Typed Code Mode parity tightened** — generated TypeScript now handles
  boolean schemas, tuple arrays, exact empty objects, format JSDoc, stable
  property order, safer identifier sanitization, and sanitized method collision
  rejection.
- **Runtime parity gaps closed** — MCP tool results now unwrap text and mixed
  content like Cloudflare, recursive JSON Schema validation runs before dispatch,
  and binary values are preserved across sandbox results and tool calls.

| Commit | Change |
|--------|--------|
| *(this)* | feat: close Code Mode Cloudflare parity gaps |

### Version bumps

- Rust workspace: `0.20.0 -> 0.21.0`
- Gateway admin package: `0.20.0 -> 0.21.0`

---

## [0.20.0] - 2026-05-27

### Highlights

- **Code Mode input shape locked to Cloudflare parity** — the `code` MCP tool's
  input schema is now EXACTLY `{ "code": "string" }` with `code` required.
  Removed the `action` discriminator (`search`/`preamble`/`execute`) plus
  `max_tool_calls` and `confirm` from the input. Three new tests
  (`code_tool_input_schema_is_locked_to_cloudflare_parity`,
  `code_tool_input_schema_rejects_action_discriminator`,
  `code_tool_description_template_uses_types_placeholder`) lock the shape so
  pre-commit (`cargo clippy -D warnings` + `cargo nextest run`) fails if anyone
  re-introduces a discriminator. Matches `cloudflare/agents/packages/codemode/src/tool.ts::codeSchema`.
- **Typed catalog moved into the tool description via `{{types}}`** — at
  `list_tools` time the server substitutes the generated
  `declare namespace codemode { ... }` block into the `code` tool's
  `description` field, matching Cloudflare's `createCodeTool` exactly. No more
  separate `code(action="preamble")` round-trip.
- **Removed `code_search` JS-against-JSON-catalog pattern** — the spec marked it
  dead; `CodeModeBroker::search`, `evaluate_code_search`, and the
  `gateway code search` CLI subcommand are all gone.
- **Docs synced to implementation** — `code-mode-agent-contract-legacy.md`
  and `code-mode-spec-legacy.md` now use snake_case examples and
  `timeout_ms = 30000`, matching the code.

### Breaking changes

- The `code` MCP tool no longer accepts `action`, `max_tool_calls`, or `confirm`
  in its input. Callers using `code({action:"execute", code:"..."})` must drop
  the wrapper and call `code({code:"..."})`. Callers using
  `code({action:"search", ...})` or `code({action:"preamble"})` have no
  replacement — read the typed namespace from the tool's description instead.
- The `lab gateway code search` CLI subcommand is removed. Use
  `lab gateway code exec` (no behavior change beyond the rename).

| Commit | Change |
|--------|--------|
| *(this)* | feat: lock `code` tool input to Cloudflare parity `{ code }` + `{{types}}` description injection |

### Version bumps

- Rust workspace: `0.19.0 -> 0.20.0`
- Gateway admin package: `0.19.0 -> 0.20.0`

---

## [0.19.0] - 2026-05-27

### Highlights

- **Cloudflare Code Mode parity**: tool names now normalize to snake_case (`movie.search` → `movie_search`) so models trained on Cloudflare examples call the right helpers.
- **Removed legacy tool aliases**: old pre-Code-Mode aliases are no longer accepted — only `code`, `search`, `execute` remain (breaking for legacy clients).
- **Typed return types in preamble**: `generate_preamble` now passes upstream `output_schema` through `schema_to_ts`, replacing `Promise<unknown>` with derived types when available.
- **Bounded preamble cache**: `PreambleCache` is now a 64-entry LRU (was unbounded `DashMap`), preventing memory growth under upstream catalog churn.
- **Canonical error kinds only**: removed non-contract `code_mode_disabled` and `code_execution_failed`; mapped to `internal_error` / `server_error` so agents switching on `err.kind` don't hit the default branch.
- **Higher default Code Mode timeout**: 5000 ms → 30000 ms (Cloudflare parity); still TOML-configurable via `[code_mode].timeout_ms`.
- **Pure computation valid in Code Mode**: removed the "must call callTool at least once" rejection so filter/sort/reduce snippets work.
- **Fixed embedded web assets test**: `serves_embedded_web_assets_without_configured_directory` now skips with a build hint when `apps/gateway-admin/out/` is empty rather than failing.

| Commit | Change |
|--------|--------|
| *(this)* | feat: Cloudflare Code Mode parity — snake_case names, bounded cache, typed returns, canonical error kinds |
| f02f8341 | fix: address all PR review issues in Code Mode gateway |

### Version bumps

- Rust workspace: `0.18.1 -> 0.19.0`
- Gateway admin package: `0.18.1 -> 0.19.0`

---

## [0.18.1] - 2026-05-27

### Highlights

- **Local runtime ignore cleanup**: ignored Broadcastr local state so repo status stays focused on source changes.

| Commit | Change |
|--------|--------|
| *(this)* | chore: ignore broadcastr local state |

### Version bumps

- Rust workspace: `0.18.0 -> 0.18.1`
- Gateway admin package: `0.18.0 -> 0.18.1`

---

## [0.18.0] - 2026-05-26

### Highlights

- **Gateway Code Mode reset**: restored the primary MCP tools to the Code Mode `search` and `execute` surface while keeping legacy aliases for that release.
- **Gateway admin controls**: added Code Mode configuration support and a `/gateways` toggle.
- **Gateway API parity**: exposed `gateway.code_mode.*` actions with validation and docs coverage.

| Commit | Change |
|--------|--------|
| *(this)* | feat: restore gateway tool names and code mode toggle |

### Version bumps

- Rust workspace: `0.17.7 -> 0.18.0`
- Gateway admin package: `0.17.7 -> 0.18.0`

---

## [0.17.7] - 2026-05-26

### Highlights

- **Retired extract service removal**: removed the dead `extract` API client, CLI command, dispatch layer, HTTP route, generated docs, and service docs.
- **Crate extraction docs cleanup**: aligned the extraction planning docs with retired `extract`, target route examples, OAuth ownership, standalone binary verification, and generated OpenAPI verification.
- **Verification stability**: fixed the host sccache wrapper toolchain resolution issue and tightened a websocket disconnect test that was racing server cleanup.

| Commit | Change |
|--------|--------|
| *(this)* | chore: remove retired extract service |

### Version bumps

- Rust workspace: `0.17.6 -> 0.17.7`
- Gateway admin package: `0.17.6 -> 0.17.7`

---

## [0.17.6] - 2026-05-26

### Highlights

- **Crate extraction ADRs**: added accepted architecture decision records for the reusable Rust crate, TypeScript package, runtime composition, surface, client-generation, distribution, execution-lane, and verification decisions.
- **Decision record index**: added a dedicated ADR index and linked it from the crate-extract and main documentation entrypoints.
- **Extraction execution ownership**: captured OAuth lane ownership and merge ordering updates in the extraction execution strategy.

| Commit | Change |
|--------|--------|
| *(this)* | docs: add crate extraction ADR records |

### Version bumps

- Rust workspace: `0.17.5 -> 0.17.6`
- Gateway admin package: `0.17.5 -> 0.17.6`

---

## [0.17.5] - 2026-05-26

### Highlights

- **Crate extraction spec**: added the crate/package extraction spec, contract, dependency map, inventory, package manifest, API surface, roadmap, testing strategy, and open-question docs.
- **Execution strategy**: documented isolated worktree lanes, parallel extraction waves, choke-point ownership, and integration responsibilities.
- **Gateway extraction planning**: tightened the standalone Gateway extraction plan around MCP resources, rmcp transports, scout/invoke behavior, and fresh-clone prune guidance.

| Commit | Change |
|--------|--------|
| *(this)* | docs: add crate extraction architecture docs |

### Version bumps

- Rust workspace: `0.17.4 -> 0.17.5`
- Gateway admin package: `0.17.4 -> 0.17.5`

---

## [0.17.4] - 2026-05-24

### Highlights

- **Stdio MCP gateway parity**: normal `labby mcp` startup now wires the same gateway manager, upstream discovery, upstream OAuth runtime, and auto-import path used by HTTP MCP.
- **Recursive stdio guard**: `LAB_SPAWN_DEPTH` still suppresses upstream spawning for recursive Lab child processes without weakening the normal stdio tool/resource surface.
- **Startup coverage**: focused tests pin the recursion-guard behavior and tolerate malformed spawn-depth environment values.

| Commit | Change |
|--------|--------|
| *(this)* | fix: align stdio mcp gateway startup with http |

### Version bumps

- Rust workspace: `0.17.3 → 0.17.4`
- Gateway admin package: `0.17.3 → 0.17.4`

---

## [0.17.3] - 2026-05-24

### Highlights

- **Gateway invoke disambiguation**: `invoke` now accepts fully-qualified `upstream::tool` names or a separate `upstream` selector when multiple upstream MCP servers expose the same tool name.
- **Agent retry guidance**: ambiguous gateway tool errors now include a structured retry hint alongside valid qualified tool names.
- **MCP boundary coverage**: focused tests cover the agent-visible ambiguity envelope and resolver behavior.

| Commit | Change |
|--------|--------|
| *(this)* | fix: disambiguate gateway invoke upstream tools |
| `3bc9faac` | docs: capture gateway oauth quick-push session |
| `4e0570c5` | fix: route gateway oauth tool access |

### Version bumps

- Rust workspace: `0.17.2 → 0.17.3`
- Gateway admin package: `0.17.2 → 0.17.3`

---

## [0.17.2] - 2026-05-24

### Highlights

- **Gateway OAuth tool routing**: admin and trusted MCP callers now route upstream OAuth tools, prompts, and resource discovery through the shared gateway credential subject while preserving non-admin subject isolation.
- **Gateway CLI/docs polish**: gateway catalog/help output and the Lab CLI skill references were refreshed around upstream proxy and schema discovery behavior.
- **Gateway admin OAuth coverage**: gateway admin adapter tests now cover OAuth config patch behavior.

| Commit | Change |
|--------|--------|
| *(this)* | fix: route gateway oauth tool access |
| `82a85762` | merge scout security fixes |

### Version bumps

- Rust workspace: `0.17.1 → 0.17.2`
- Gateway admin package: `0.17.1 → 0.17.2`

---

## [0.17.1] - 2026-05-23

### Highlights

- **Scout authorization hardening**: `scout` now requires read-capable scopes, suppresses schemas for read-only callers, and keeps `invoke` behind execution scopes.
- **Gateway priority enforcement**: priority-zero upstreams are filtered from semantic RRF results, direct invoke resolution, and semantic index rebuilds.
- **Semantic pipeline reliability**: Qdrant/TEI clients now support auth headers, semantic URL resolution no longer relies on panic-prone `expect()` gates, and wiremock coverage exercises the semantic client request path.
- **Worktree hygiene**: `.cache` symlinks are ignored alongside cache directories.
- **Stale service cleanup**: removed remaining Paperless plugin, docs, UI, env, and health-check references after the active service implementation was removed.

| Commit | Change |
|--------|--------|
| `b7f4bab7` | docs: capture scout cleanup quick-push session |
| `466586d0` | fix: harden scout access and remove paperless remnants |
| `3a01a1f4` | test(lab-mqd6f.3): cover semantic pipeline clients |
| `47b47703` | fix(lab-9ycyb): add semantic client auth |
| `fa4b4d0f` | fix(lab-mqd6f.5): remove search semantic expects |
| `8f5722d6` | fix(lab-mqd6f.1): scope scout gateway search |
| `9e17b029` | fix(lab-mqd6f.2): enforce priority-zero gateway suppression |

### Version bumps

- Rust workspace: `0.17.0 → 0.17.1`
- Gateway admin package: `0.17.0 → 0.17.1`

---

## [0.17.0] - 2026-05-21

### Highlights

- **Gateway pending-approval queue**: auto-import of upstream MCP servers is now disabled by default; discovered servers land in an approval queue instead of being added blindly.
- **Semantic search improvements**: hybrid (dense + sparse) tool search wiring extended in `dispatch/gateway/semantic.rs` with new index plumbing.
- **TEI + Qdrant client surface**: expanded TEI client capabilities; Qdrant client gains additional collection-management methods.
- **Docs**: new `docs/contracts/` directory established with the first wire-shape contract (`gateway-schema-resources.md`).

| Commit | Change |
|--------|--------|
| `bd1dcee9` | feat(gateway): disable auto-import by default; add pending approval queue |
| *(this)* | feat: tei/qdrant client surface + gateway semantic search wiring |

### Version bumps

- Rust workspace: `0.16.0 → 0.17.0`
- Gateway admin package: `0.16.0 → 0.17.0`

---

## [0.16.0] - 2026-05-16

### Added
- `GET /v1/catalog` endpoint: aggregated service+action catalog filtered to enabled services, with ETag and Cache-Control headers
- ⌘K command palette web CLI: live catalog browse, cmdk page-stack navigation, schema-driven param forms, destructive confirmation, `X-Lab-Source: palette` tracing

---

## [0.15.2] — 2026-05-11

### Highlights

- **gateway-admin server terminology**: the admin UI now labels managed MCP upstreams as Servers instead of Gateways across navigation, dashboards, lists, detail pages, dialogs, docs, settings, protected routes, and tests.
- **OAuth refresh resource reuse**: refresh-token grants now preserve the stored resource when the request omits `resource`, while still rejecting explicit mismatched resources.

| Commit | Change |
|--------|--------|
| *(this)* | fix(gateway-admin): rename gateways to servers in UI |

### Version bumps

- Rust workspace: `0.15.1 → 0.15.2`
- Gateway admin package: `0.15.1 → 0.15.2`

---


## [0.15.1] — 2026-05-11

### Highlights

- **gateway protected-route editing**: editing an existing gateway now hydrates the protected-route path and auth mode from persisted protected route state, including late-arriving route data and stdio gateways with OAuth-protected endpoints.

| Commit | Change |
|--------|--------|
| *(this)* | fix(gateway): restore protected route edit state |

### Version bumps

- Rust workspace: `0.15.0 → 0.15.1`
- Gateway admin package: `0.15.0 → 0.15.1`

---

## [0.15.0] — 2026-05-05

### Highlights

- **gateway-admin mobile chat**: chat message bubbles now preserve long prose, markdown, code blocks, and action traces inside the mobile viewport; the copy affordance remains reachable on touch devices.
- **agent running state**: active runs now show as an inline assistant working bubble instead of a top-of-conversation status banner, with tests covering the streaming and waiting-for-permission conditions.
- **chat state mockups**: adds the assistant working-bubble mockup used to compare running-state placement options.

| Commit | Change |
|--------|--------|
| *(this)* | feat: optimize mobile chat running state |

### Version bumps

- Rust workspace: `0.14.0 → 0.15.0`
- Gateway admin package: `0.14.0 → 0.15.0`

---

## [0.14.0] — 2026-05-04

### Highlights

- **ACP sessions**: prompt dispatch now replaces the default "New session" title with a bounded title derived from the user's prompt, and unfinished provider exits now preserve provider-error details instead of always emitting the generic no-stop-reason event.
- **gateway-admin chat UI**: reasoning summaries and agent actions now render as separate panels; action traces keep grouped read/search/edit/command summaries, and a render test guards against folding actions back into reasoning.
- **Vibin GitHub workflow consolidation**: GitHub review and CI skills move under the Vibin plugin, with marketplace and plugin metadata updated to describe the expanded workflow surface.

| Commit | Change |
|--------|--------|
| *(this)* | feat: improve ACP session titles and separate chat reasoning from actions |

### Version bumps

- Rust workspace: `0.13.1 → 0.14.0`
- Gateway admin package: `0.13.1 → 0.14.0`

---

## [0.13.1] — 2026-05-04

### Highlights

- **gateway-admin chat UI**: agent tool calls are now compact by default — summary text, file preview snippets, and category/status labels moved behind the expand chevron; file paths shown inline under the label instead of as chips; skill labels now show the skill name rather than full description text

| Commit | Change |
|--------|--------|
| `d62b33bf` | fix: validate acp smoke stream output |
| `5743e804` | fix(gateway-admin): compact agent action tool calls — collapse summary/preview by default, inline paths, extract skill name from label |

---

## [0.13.0] — 2026-05-04

| Commit | Change |
|--------|--------|
| `60939ce2` | fix(nodes): close only on rejected initialize, not on pre-init method errors |
| `f619f025` | fix(lab-p760): wrap all sync stash dispatch arms in spawn_blocking |
| `2270470f` | fix(lab-qytb): provider.pull writes revision meta inside component lock |
| `5f409c05` | fix(lab-gxhk): target.add marked destructive + path validated at registration |
| `6ca17048` | fix(lab-n4fb): canonicalize fail-closed for stash deploy path denylist |
| `35036109` | fix(lab-686q): typed 404 downcast in node_connected, remove redundant log event, add retry assertion |
| `e5c3361e` | fix(lab-686q): allow dead_code on build_release compat wrapper |
| `7e9db919` | test(lab-686q.2): replace symbol-check with real behavior tests for node_connected |
| `e8bd9793` | fix(lab-686q.1): run_impl builds per-role artifacts — no more panic on Node-role hosts |
| `d9c4a050` | test(lab-686q.3): add tests for wait_for_node_connected retry and timeout logic |
| `df4bc31f` | test(lab-686q.4): add tests for --role node and config role=node without controller host |
| `e44249b2` | fix(lab-686q): fix clippy lint warnings — remove unused Duration/jitter_window, allow dead_code on reserved fields |
| `e7ae7d59` | docs(lab-686q): Task 14 — normalize controller/node naming, document artifact split |
| `aad75295` | feat(lab-686q): Task 13 — per-role artifact map in deploy runner, DeployArtifactSummary in plan/summary |
| `c93172a3` | chore(lab-686q): fix extract feature ordering in lab-apis features list |
| `a4af24a4` | feat(lab-686q): Task 12 — gate lab-apis/extract deps behind extract feature |
| `8a6766d7` | feat(lab-686q): Task 11 — make clap_complete optional, gate completions behind controller feature |
| `85ae9017` | feat(lab-686q): Task 10 — feature groups (controller, services-all, node-runtime), gate gateway/marketplace/upstream |
| `29867ca6` | feat(lab-686q): Tasks 8+9 — readiness contract docs, backup path in recovery result |
| `9411d92a` | fix(lab-686q): thread config port through verify_local_health (no hardcoded 8765) |
| `8137a3b2` | feat(lab-686q): Task 7 — role-based nodes update, wait_for_node_connected, multi-artifact build |
| `df7e13c9` | feat(lab-686q): Task 6 — MasterClient::node_connected for rollout verification |
| `1f01558e` | feat(lab-686q): Tasks 4+5 — deploy profiles, ArtifactProfile, build_artifact with timeout |
| `33650f64` | feat(lab-686q): Task 3 — move backoff helpers to net/backoff, add node-runtime feature |
| `44847e42` | feat(lab-686q): Task 2 — node-mode early return in serve, start_background_tasks, loopback health server |
| `e7f9ad68` | fix(lab-686q): add resolution source to role.resolved tracing event |
| `3889b496` | feat(lab-686q): Task 1 — NodeRuntimeRole config, ServeRole CLI, resolve_runtime_role_from_config |
| `073e1456` | fix(acp): add turn-drain timeout to handle stale messages after idle-completed turns |

### Highlights

- **Node/controller runtime split** — adds explicit node runtime role handling, node-mode serving behavior, controller/node naming docs, and deployment artifacts split by role.
- **Deploy and readiness hardening** — adds deploy profiles, artifact summaries, local-health port threading, wait-for-node-connected retry behavior, and recovery backup path reporting.
- **Feature grouping** — gates controller-only and service-heavy code behind feature groups, makes completions optional, and gates extract dependencies behind the extract feature.
- **ACP and dispatch fixes** — protects ACP multi-turn flows from stale messages and wraps sync stash dispatch paths in `spawn_blocking`.

### Version bumps

- Rust workspace: `0.12.2 → 0.13.0`
- Gateway admin package: `0.6.0 → 0.13.0`

---

## [0.12.2] — 2026-05-03

| Commit | Change |
|--------|--------|
| `50824844` | chore(lab-in5q.4): fix internal cross-references in moved doc files |
| `498b1ffa` | chore(lab-in5q.3): update Rust source doc path comments |
| `7ce5812e` | chore(lab-in5q.2): update CLAUDE.md references for moved doc files |
| `79824d98` | chore(lab-in5q.1): reorganize docs/ root — move 34 files into surfaces/ runtime/ services/ dev/ |
| `cf9373e7` | chore: dev tooling, ACP multi-turn fix, and docs reorg prep |
| `386e6d7b` | chore: save CI debugging session state |
| `9689190c` | fix: avoid Windows CI cache save failure |
| `6f8b8189` | fix: Windows release warnings |
| `68a35a37` | chore: trigger CI after history rewrite |
| `11199215` | fix: CI failures |
| `60568674` | chore: set up CI release smoke and generated docs |
| `da3a8d10` | docs: say copy config.example.toml to ~/.labby/config.toml |
| `0db193bf` | fix: config.toml is gitignored; update docs |
| `3a226869` | feat: fleet scan wizard step, config consolidation, and TS fixes |
| `8b1b9967` | chore: document cargo-deny advisory exceptions |
| `a0c5f734` | chore: integrate service wave and CI updates |
| `d31767c9` | fix(lab-8l5s): preserve ServiceForm RHF state across tab switch |
| `ef2cae3a` | docs(lab-qz0z): document R5 RHF state-loss tradeoff inline |
| `233595ca` | fix(lab-qz0z): post-review cleanup (HTTPS_SCHEME_RE, %00 blocking) |
| `f911d607` | fix(lab-qz0z): mirror sessionStorage write outside React state updater |
| `8510d39c` | fix(lab-qz0z): lifecycle cleanups (P3-3, P3-4, P3-7) |
| `de0952f7` | fix(lab-qz0z): RHF perf — Controller for bool, memoized callbacks |
| `00210371` | fix(lab-qz0z): TypeScript and code-quality nits batch |
| `19f16e8c` | fix(lab-qz0z): secret-handling hardening (P2-6, P3-8) |
| `65c4bcc7` | fix(lab-qz0z): harden schemaBuilder validation |
| `619c8445` | feat(lab,lab-apis,lab-auth): backend in-flight work |
| `13e29ede` | fix(lab-qbbt): distinguish transport errors from blocking findings |
| `7be1b484` | fix(lab-emkz): lazy-mount only the active ServiceForm tab |
| `6c16f591` | fix(lab-1ai7): re-check pathname after setup.state await |
| `f987aae6` | fix(lab-kltp): surface draft-stale check failures instead of silencing |
| `fbd2af79` | fix(lab-ijf3): synchronous lock + AbortController for core-config save |
| `7bf605a3` | fix(lab-fcz0): thread AbortSignal through doctor.service.probe |
| `9a641bc6` | fix(lab-4cn9): persist wizard selectedServices to sessionStorage |
| `7dd6570f` | fix(lab-68ja): centralize '***' secret sentinel as STORED_SECRET_MARKER |
| `44a3728a` | fix(lab-zmj1): extract CORE_FIELDS to shared module |
| `77efe9b5` | fix(lab-7bat): remove dead draftValues/setDraftValue from WizardContext |
| `927a1a6a` | feat(lab-apis,lab): onboard 3 services + extend adguard/glances/uptime-kuma |
| `9604c93f` | feat(lab-apis,lab): onboard 6 services (dozzle, freshrss, immich, loki, exampleindexer, exampleusenet) |
| `331a38e1` | feat(lab-bg3e.4,bg3e.5): /setup wizard + /settings rail web UI |
| `9d24b17e` | test(lab-bg3e.3.11): mechanical guard for orchestrator one-way dependencies |
| `b28d5a28` | refactor(lab-bg3e.3.7): env_merge polish |
| `2ef9b43c` | fix(lab-bg3e.3.9): defense-in-depth hardening |
| `7a612a65` | refactor(lab-bg3e.3.8): tighten setup dispatch hygiene |
| `07041287` | refactor(lab-bg3e.3.10): drop dead is_headless() |
| `8195e86b` | feat(lab-bg3e.3.4): add write_service_creds shim over env_merge::merge |
| `b705d37b` | perf(lab-bg3e.3.1): memoize ToolRegistry + env-var/secret-key indexes |
| `4e717482` | fix(lab-bg3e.3.2): wrap doctor.audit.full in 30s timeout |
| `de859e41` | fix(lab-bg3e.3.3): apply host_validation Layer to all v1 unauthenticated routes |
| `758ec61f` | perf(lab-bg3e.3.6): single-pass audit_summary count |
| `bb1d071a` | fix(lab-bg3e.3.5): fsync parent dir after env_merge::persist |

### Highlights

- **ACP multi-turn drain timeout** — `acp_turn_drain_timeout()` + `DEFAULT_TURN_DRAIN_TIMEOUT` (5 min, overridable via `LAB_ACP_TURN_DRAIN_TIMEOUT_MS`) drains stale messages left by idle-completed turns before starting the next prompt. Prevents a late `PromptResponse`/`StopReason` from poisoning the new inner read loop during long agentic tool calls.
- **Docs reorganization (lab-in5q)** — 34 docs moved from `docs/` root into `docs/surfaces/`, `docs/runtime/`, `docs/services/`, and `docs/dev/`; CLAUDE.md, README references, and Rust source path comments all updated.
- **Service onboarding wave** — 9 new services onboarded: dozzle, freshrss, immich, loki, exampleindexer, exampleusenet (wave 1) + 3 more + adguard/glances/uptime-kuma extensions.
- **Setup wizard + settings rail UI (lab-bg3e)** — full `/setup` wizard flow (fleet scan, service creds, config write) and `/settings` side-rail; `write_service_creds` shim, `env_merge` polish, `ToolRegistry` memoization, `doctor.audit.full` timeout, host-validation middleware, fsync-after-persist hardening.
- **Frontend hardening (lab-qz0z, lab-8l5s, and others)** — RHF state preservation across tab switches, secret-sentinel centralization, schemaBuilder validation hardening, sessionStorage persistence for wizard selections, AbortController for config saves, lazy-mount for inactive ServiceForm tabs.
- **CI improvements** — release smoke tests, generated-docs pipeline, Windows cache-save fix, cargo-deny advisory exceptions.

### Version bumps

- Rust workspace: `0.12.1 → 0.12.2`

---

## [0.12.1] — 2026-04-30

| Commit | Change |
|--------|--------|
| `5a00e40c` | chore(release): v0.12.1 — binary build fix |
| `bcc59e4f` | fix: declare observability module in main.rs and add stash to router parity test |

### Highlights

- **Binary build fix** — `main.rs` was missing `mod observability;`, so `crate::observability::activity::ActorKey{,Deriver}` references in `api/state.rs` and `api/router.rs` failed to resolve when compiling the binary (lib.rs already declared the module, so library-only callers were unaffected). Five E0433 errors gone. Also adds `stash` to the `registry_and_router_service_sets_are_identical` parity test, which had been silently asserting an outdated set since `lab-qz6a.8` landed stash in the HTTP router.

### Version bumps

- Rust workspace: `0.12.0 → 0.12.1`

---

## [0.12.0] — 2026-04-30

| Commit | Change |
|--------|--------|
| `3244fb7c` | chore(release): v0.12.0 — ACP review remediation epic close-out |
| `e2ade2b9` | docs(BD-lab-j04j.16): refresh ACP docs against landed first-class state |
| `f8e88fda` | feat(BD-lab-j04j.11): structured AcpProviderEntry args/cwd/env |
| `90b16a48` | feat(BD-lab-j04j.10): bound ACP event channel to 1024 with await-on-send |
| `0838775d` | docs(BD-lab-j04j.19): document provider prompt idle timeout |
| `e2d8b6c0` | feat(BD-lab-j04j.18): replace page-context allowlist with predicate sanitizer |
| `20c0a2b7` | feat(BD-lab-j04j.15): cap ACP SSE backfill at SQL layer |
| `cf2c7e5b` | feat: gate stdio gateway specs behind allow_stdio admin ack |
| `0221b23f` | docs: expand product and marketplace surface |
| `4a8a2d53` | docs: expand product feature overview |
| `3215a9ba` | docs: describe product feature surface |
| `18a5684b` | chore: update marketplace docs and monitors |
| `fe09366c` | fix(dev): address code review findings |
| `4ae40caf` | feat(lab-zxx5.8): add MCP server install modal with gateway selection |

### Highlights

- **ACP review remediation (lab-j04j) — epic closed** — 18 of 19 child beads landed; runtime/security hardening across SSE backfill, event channel bounding, provider config, page-context sanitizer, idle-timeout docs. Bridge\* compat removal (.12) deferred pending coordinated frontend wire-format change.
- **ACP SSE backfill SQL cap (.15)** — `load_events_since_capped` on `AcpPersistence` trait + SQLite subquery (`ORDER BY seq DESC LIMIT N`, re-sorted ASC) preserves "last N events" backfill contract without materialising the full event range. Previous in-Rust truncation was a memory waste at high event rates.
- **ACP event channel bounded (.10)** — per-session `UnboundedSender<AcpEvent>` from runtime → registry hub becomes `mpsc::Sender<AcpEvent>` at capacity 1024 with await-on-send. Back-pressures the provider's stdio reader on persistence stalls instead of growing memory unboundedly. Five sync `emit_*`/`push_session_update` helpers become async; `std::Mutex` guard scopes restructured to avoid spanning `.await`.
- **Structured AcpProviderEntry (.11)** — `command + args + cwd + env` schema with serde defaults; legacy entries fall back to whitespace-split `command` for one-time read fidelity. Re-installing a provider migrates the on-disk entry. Marketplace install paths (binary/npx/uvx) build args as `Vec<String>` rather than concatenating into a single string.
- **Page-context sanitizer (.18)** — predicate-based `is_safe_page_context_char` replaces the 62-element char allowlist; deny-list bypass detection adds a separator-stripped joined-form check; 23 tests covering control chars, unicode rejection, separator-bypass attempts, and length boundaries.
- **Stdio gateway admin ack** — `gateway.test`/`add`/`update` require explicit `allow_stdio: true` when the upstream spec uses stdio. Stdio specs spawn local subprocesses, so admin operations against them are gated through `ensure_stdio_admin_ack` to prevent silent process launches via remote dispatch. CLI mirrors with `--allow-stdio` flags; catalog publishes `allow_stdio` as a documented param.
- **Provider prompt idle timeout (.19)** — operator-facing section in `docs/acp/README.md` documenting the 5 s default, `LAB_ACP_PROMPT_IDLE_TIMEOUT_MS` override, and the observable firing behavior (`session_state` Completed + `provider_info` `idle_completion`).
- **ACP docs match landed first-class state (.16)** — README inventories the landed pieces (lab-apis::acp module, dispatch/acp/, registry registration, HTTP routes), enumerates landed protections, and lists remaining gaps (Bridge\* compat, typed CLI shim, provider workspace jail) without claiming deferred work.
- **Pre-existing unreleased work** — earlier commits (`0221b23f` … `4ae40caf`) accumulated in the previous Unreleased section before the epic close-out and ride along with this release: Code Mode config + settings UI for gateway-admin, MCP server install modal with gateway selection, marketplace and product docs expansion, dev review-finding fixes.

### Version bumps

- Rust workspace: `0.11.1 → 0.12.0`
- gateway-admin: `0.5.1 → 0.6.0` (bumped during the Unreleased window prior to this release)

---

## [0.11.1] — 2026-04-25

| Commit | Change |
|--------|--------|
| `82478a0b` | chore(release): v0.11.1 — marketplace P1 security follow-up + workspace fs hardening |
| `2f6d76c6` | docs: setup+settings feature design spec + component-development doc update |
| `07ccb54c` | fix(dev): ensure dev_mockup routes survive router.rs refactors |
| `d10b05ec` | fix(dev/systeminfo): read env from process (dotenvy already loaded .env at startup) |
| `991fcd1b` | feat(dev): extend systeminfo to return .env values with secrets masked |
| `aea3bb59` | fix(dev): restore dev_mockup handlers and page routes |
| `b1385289` | fix(dev): restore /dev mockup routes + add /dev/api/systeminfo |
| `265a701e` | feat(dev): add mockup file server at /dev and /dev/:name |
| `3e8db769` | fix(pr29): address review threads — security, fleet, ACP, marketplace, docs |
| `f168964b` | fix(lab-zxx5.32): R2 P3 roll-up — redact_home in errors, log tiering, sync_all, dead Sized |
| `39266dce` | refactor(lab-f1t2): address simplify + review findings on the f1t2 wave |
| `b7f488af` | fix(lab-zxx5.30,lab-zxx5.31): partial-extraction detection + fail-closed walk |
| `7b051062` | fix(lab-zxx5.29): validate node install result shape |
| `12eb0ea0` | fix(lab-zxx5.28): typed error markers restore install taxonomy |
| `ae302ef6` | docs(lab-f1t2.32): document MCP transport auth requirement for fs |
| `86e943eb` | fix(lab-f1t2.26): redact path from deny-list oracle log events |
| `c9be4573` | fix(lab-f1t2.30): reset AttachmentChip thumbUrl at effect start |
| `33db1293` | fix(lab-f1t2.29): reset loading/truncated when picker closes mid-fetch |
| `0e7a569f` | fix(lab-f1t2.24): handle help/schema before workspace_root resolution |
| `6101fdbe` | fix(lab-zxx5.27): P3 roll-up — SSRF edges, per-node cap, redact_home, naming cleanup |
| `3c135072` | docs(lab-f1t2.31): document fs registry uses MCP-filtered slice intentionally |
| `b6386ad9` | fix(lab-f1t2.28): move setSending(true) inside sendingRef try |
| `76962fc3` | fix(lab-f1t2.27): align workspace-picker error kinds with backend |
| `c892efce` | test(lab-f1t2.25): bidirectional parity test for MCP fs catalog |
| `85f019e4` | fix(lab-f1t2.23): case-insensitive credential deny-list |
| `9aaa8c7a` | fix(lab-f1t2.22): reject intra-workspace symlinks in openat2 fallback |
| `40ac16a1` | fix(lab-zxx5): resolve multi-agent review P1+P2 findings |
| `e7ea8528` | refactor(lab-f1t2.20): inline log_dispatch/log_dispatch_preview wrappers |
| `01de323a` | chore: untrack crates/lab/target/ build artifacts |

### Highlights

- **Marketplace P1 security follow-up (lab-zxx5)** — multi-agent review P1+P2 fixes, install_component/agent.install hardening, SSRF blocklist edges, per-node caps, `redact_home` helper applied to errors and log tiering, partial-extraction detection with fail-closed walk, typed install error markers
- **Workspace fs hardening (lab-f1t2)** — security headers via subrouter middleware, intra-workspace symlink rejection in openat2 fallback, case-insensitive credential deny-list with path redaction, MCP transport auth requirement documented, MCP↔canonical fs ActionSpec parity locked, AttachmentChip + chat-input + workspace-picker race elimination, UX polish
- **Dev mockup routes** — mockup file server at `/dev` and `/dev/:name`, `/dev/api/systeminfo` returning `.env` values with secrets masked, route survival across router.rs refactors
- **Docs** — setup+settings feature design spec, component-development doc update

### Version bumps

- Rust workspace: `0.11.0 → 0.11.1`
- gateway-admin: `0.5.0 → 0.5.1`

---

## [0.11.0] — 2026-04-24

| Commit | Change |
|--------|--------|
| `9d83267b` | chore: bump workspace to 0.11.0 + misc uncommitted work |
| `979bae1a` | feat(lab-zxx5.18): install_component/agent.install security hardening |
| `bbebe993` | refactor(lab-f1t2.18): removeAttachment keys on (kind, path) compound |
| `b41a7315` | ux(lab-f1t2.19): workspace picker polish — truncated reset + kind messages + aria |
| `328664b4` | perf(lab-f1t2.15): dedupe concurrent workspace preview fetches |
| `1c8b9731` | fix(lab-f1t2.16): eliminate chat input + workspace picker + preview races |
| `d077428b` | test(lab-f1t2.11): lock MCP/canonical fs ActionSpec parity |
| `f66823aa` | perf(lab-f1t2.14): eliminate redundant lstat + ASCII fast-path for deny-list |
| `c844d053` | feat(lab-zxx5.16): cherry-pick SSE progress endpoint |
| `b14cbe75` | refactor(lab-f1t2.17): consolidate fs dispatch into single match body |
| `a718f15a` | fix(lab-f1t2.12): apply fs security headers via subrouter middleware |
| `cfeb698a` | feat(lab-f1t2.13): register fs unconditionally when feature-enabled |
| `12666cef` | fix(lab-zxx5.2): route mcp.* actions to mcp_dispatch in marketplace dispatch |
| `8d0b2572` | chore(lab-f1t2): snapshot pre-review-fixes state |
| `7610accd` | feat(lab-zxx5.6): wire real NodeRpcPort + master pending infra + rename device→node |
| `4c7567a1` | feat(lab-zxx5.19): bounded inbound-RPC dispatch + UUIDv4 request ids |
| `7f0f55e4` | fix(lab-zxx5.15): normalize marketplace client path helpers to Result |
| `910037d3` | feat(lab-zxx5.14): Default derives, redact_home helper, plugins.list invariant test |
| `d18eb12b` | feat(lab-ccc9): Phase 3 WS fleet method handlers + MCP demux |
| `1351cad2` | feat(lab-e2tu): SQLite-backed node log persistence with 30-day TTL retention |
| `9300b884` | fix(lab-zxx5.13): map ambiguous_tool kind to 409 Conflict + document |
| `daeb1ef6` | fix: restore compile — add AmbiguousTool variant, fix codex backend Option/Result, update Marketplace/Plugin literals |
| `d77fbeab` | feat(lab-f1t2.1): workspace root resolver + AppState field |
| `462e63f6` | feat(lab-yn60): complete device→node module rename |
| `0564a9e2` | wip(acp): chat-shell + session events + ACP runtime refactor |
| `916ac283` | feat(lab-zxx5.8): add MCP server install modal with gateway selection |
| `20cc45a9` | feat(lab-zxx5.3): stream SHA-256 during binary archive download |
| `453162aa` | fix: commit node module files and resolve device→node rename breakage |
| `ec476ba3` | feat(lab-zxx5.3): implement remote fleet WS install and binary agent download |
| `81901791` | perf(lab-kvhi.16): run config.read + current_pool concurrently in gateway list/get |
| `f16f43a9` | fix(lab-kvhi.14): accumulate reasoning duration across SSE reconnects |
| `a4851368` | feat(lab-zxx5.6): add plugin.cherry_pick dispatch action |
| `21e5f4b5` | feat(lab-zxx5.11): unified marketplace API client + PluginComponent types |
| `e93da3ae` | feat(lab-zxx5.4): delete mcpregistry dispatch surface, migrate to marketplace |
| `094eeba4` | feat(lab-zxx5.3): add ACP agent dispatch actions (agent.list/get/install/uninstall) |
| `9bbfd50c` | feat(lab-zxx5.10): add cherry-pick component selector dialog |
| `ae827055` | fix(lab-zxx5): resolve Wave 1 compile errors and test failures |
| `0c7f4cbc` | feat(lab-bg3e.2): promote doctor to full Bootstrap dispatch service |
| `f504e26a` | fix(gateway-admin): misc correctness + accessibility batch |
| `d2bbdd05` | fix(gateway-admin): prop-spread ordering to prevent consumer clobbering |
| `043920c7` | fix(gateway-admin): file-tree accessibility + dead code + handler ordering |
| `282e18b5` | fix(gateway-admin): prompt-input five correctness fixes |
| `41b1f167` | feat(lab-zxx5.8): add MCP server install modal with gateway selection |
| `e7760dd9` | fix(gateway-admin): shared useCopyTimeout hook to prevent leaked setState-after-unmount |
| `a3de2667` | feat(lab-zxx5.9): add ACP agent install modal with device and scope selection |
| `7a76de00` | fix(gateway-admin): runtime crash + stuck timer + unreachable Cancel |
| `eca9f7d9` | fix(gateway-admin): resolve broken ~/ import aliases in AI components |
| `d8490870` | feat(lab-jwbg.8): ACP service registration — PluginMeta, registry, serve wiring |
| `c2f8bd65` | feat(lab-zxx5.1): add lab-apis/src/acp_registry SDK client |
| `1945e5b3` | fix(lab-zxx5): resolve Wave 0 compile errors and test failures |
| `8a166f14` | feat(lab-jwbg.7): migrate API/ACP surface to dispatch/acp layer |
| `dbf49212` | feat(lab-jwbg.6): dispatch/acp layer — catalog, client, params, dispatch |
| `3ff6b209` | feat(lab-jwbg.5): rewrite AcpSessionRegistry — Arc<Session>, per-subscriber mpsc, ownership |
| `78a8f7f7` | feat(lab-bg3e.1): UiSchema/FieldKind types + PluginMeta.supports_multi_instance for all 23 services |
| `dd707162` | feat(lab-jwbg.3): SQLite persistence layer — AcpPersistence trait + SqliteAcpPersistence |
| `c3e0f350` | feat(lab-zxx5.5): add marketplace.install_component + agent.install RPC methods |
| `791d1196` | feat(bd-security/marketplace-p1): ACP types, fleet WS registry, marketplace UI, Category::Marketplace |
| `f8de5bde` | feat(lab-jwbg.2): migrate ACP types — Bridge* → Acp* in lab-apis |
| `bba30eb2` | feat(lab-zxx5.7): unified marketplace type filter + MCP/ACP item cards |
| `3124a871` | feat(lab-zxx5.5): add fleet WS master→device sender registry |
| `43ad105b` | fix(pr29): catalog filter chips can return to 'all' view |
| `b8ad6306` | feat(lab-zxx5.12): add Category::Marketplace, recategorize marketplace + mcpregistry |
| `35752048` | fix(pr29): address remaining review threads on AI components + docs |
| `9e0383ba` | fix(marketplace): address PR #29 review threads — installPath validation |
| `299eb724` | fix(lab-jwbg.9): eliminate try_write().expect() panic in AcpSessionRegistry |
| `526bf3e1` | feat(lab-jwbg.1): create lab-apis::acp module scaffold |

### Highlights

- **Workspace bumped two minors in one commit** — `9d83267b` jumped `0.9.0 → 0.11.0` directly with no `0.10.x` published; this section accumulates everything done between the `0.9.0` bump and that commit
- **WS fleet runtime + remote install (lab-zxx5.3/.6, lab-ccc9, lab-e2tu)** — real `NodeRpcPort` master pending infra, device→node module rename, remote fleet WS install + binary agent download (streamed SHA-256), plugin.cherry_pick dispatch + cherry-pick component selector dialog, Phase 3 WS fleet method handlers + MCP demux, SQLite-backed node log persistence with 30-day TTL retention, SSE progress endpoint
- **ACP service consolidation (lab-jwbg)** — `acp_registry` SDK client + `lab-apis::acp` scaffold, `dispatch/acp` layer (catalog, client, params, dispatch), API/ACP surface migrated to dispatch layer, `AcpSessionRegistry` rewrite with `Arc<Session>` + per-subscriber mpsc + ownership semantics, SQLite persistence (`AcpPersistence` trait), ACP agent dispatch actions (`agent.list/get/install/uninstall`), MCP server + ACP agent install modals with gateway/device/scope selection, `try_write().expect()` panic eliminated
- **Marketplace consolidation (lab-zxx5.x)** — unified marketplace API client + `PluginComponent` types, `mcpregistry` dispatch surface deleted and migrated to marketplace, `Category::Marketplace` introduced, install_component/agent.install RPC methods, fleet WS master→device sender registry, multi-agent review P1+P2 fixes
- **Workspace fs (lab-f1t2 entry)** — workspace root resolver + AppState field, fs registered unconditionally when feature-enabled, dispatch consolidated into single match body, MCP/canonical fs ActionSpec parity test, deny-list ASCII fast-path
- **Doctor + bootstrap (lab-bg3e)** — doctor promoted to full Bootstrap dispatch service, `UiSchema`/`FieldKind` types + `PluginMeta.supports_multi_instance` for all 23 services
- **Gateway admin AI component pass** — prompt-input five-fix correctness pass, file-tree accessibility, prop-spread ordering, runtime-crash + stuck-timer + unreachable-Cancel fixes, shared `useCopyTimeout` hook, AI components import-alias repair
- **Gateway perf** — `config.read` + `current_pool` run concurrently in gateway list/get, reasoning duration accumulated across SSE reconnects

### Version bumps

- Rust workspace: `0.9.0 → 0.11.0` _(skipped `0.10.x`)_

---

## [0.9.0] — 2026-04-23

| Commit | Change |
|--------|--------|
| `2013dbdd` | feat: AI component library, ACP docs, gateway/marketplace UI refinements — v0.9.0 |
| `7c4fb9f` | fix(lab-kvji.10.1): validate path components in parse_plugin_id |
| `ca66a3b` | fix(lab-kvji.10.3): validate installPath from installed_plugins.json |
| `cd8bfa9` | fix(lab-kvji.10.2): add symlink guards to all filesystem walkers |
| `a9dcd54` | Finalize gateway admin, registry, and auth follow-ups |
| `0a6c846` | feat: add registry metadata curation and admin filters |
| `479bae4` | fix: address latest PR comment |
| `5a75aba` | fix: address follow-up PR comments |
| `227b4ed` | fix: address PR review feedback |
| `fd8aafc` | docs: update fleet websocket runtime docs |
| `8ecda7b` | feat: add websocket fleet runtime |
| `facca22` | docs: add fleet ws runtime design |
| `0cad306` | Finalize remaining gateway admin and registry work |
| `47171c0` | fix: address remaining marketplace and upstream review comments |
| `4392a42` | fix: address gateway plan and docs review comments |
| `867dda3` | fix: address gateway admin design-system review comments |
| `ccafbdb` | fix: address gateway admin registry review comments |
| `91188af` | fix: address gateway admin chat and logs review comments |
| `410acdb` | Finalize remaining chat, marketplace, and deploy updates |
| `38fd124` | fix: address PR comments for gateway policy and browser session auth |
| `997110e` | fix: address PR comments for marketplace client and dialog flows |
| `6ae4bd9` | fix: address PR comments for registry and marketplace dispatch |
| `a51056f` | fix: address PR comments for gateway and registry docs |
| `e5dec3d` | Add gateway ACP, marketplace, and CLI UI updates |
| `9a0f23b` | Address PR review feedback |

### Highlights

- **Marketplace security hardening P1 (lab-kvji.10)** — path traversal via plugin ID blocked at parse time; symlink following eliminated from all four filesystem walkers; `installPath` from `installed_plugins.json` validated against `plugins_root` before use
- **AI component library** — 26 new TSX components under `components/ai/` covering agents, artifacts, attachments, code blocks, reasoning, tool calls, and more
- **Fleet websocket runtime** — initial `feat: add websocket fleet runtime`; ACP provider, session registry, SSE transport, design docs
- **Registry metadata curation** — Lab-owned `_meta["dev.labby/registry"]` contract, validation, audit fields, server-side metadata filters, typed CLI metadata commands, gateway-admin structured metadata editing
- **Marketplace and upstream hardening** — marketplace client/dispatch cleanup, upstream pool adjustments, browser session auth fixes, large batch of PR-review-driven repairs across gateway, registry, marketplace, chat, and deploy

### Version bumps

- Rust workspace: `0.7.3 → 0.9.0` _(skipped `0.8.x`)_

---

## [0.7.3] — 2026-04-22

| Commit | Change |
|--------|--------|
| `681986c` | feat(gateway-chat-registry-log-ui): marketplace UI, gateway/chat/registry/log component polish, mcpregistry fixes — v0.7.3 |
| `802d67e` | feat(marketplace): route + sidebar nav entry — Marketplace page complete |
| `3674c5b` | feat(marketplace): all UI components — cards, panels, dialogs, modal |
| `120bf6a` | feat(marketplace): types, API client (mock data), and SWR hooks |
| `861e4e8` | feat(gateway-admin): wire listServers to GET /v0.1/servers REST endpoint |
| `de8d173` | fix(registry_v01): normalize error kinds; add owner filter; use ToolError uniformly |
| `ff6185a` | fix(mcpregistry): extract shared sync guards to dispatch layer |
| `4dfd248` | fix(mcpregistry/params): add Tailscale CGNAT range to SSRF blocklist |
| `9892d33` | fix(mcpregistry/store): ON CONFLICT DO UPDATE, jiff, WAL, UTF-8 truncation |
| `c67b839` | fix(lab): remove chrono dep, feature-gate rusqlite/r2d2 under mcpregistry |
| `281dfbd` | fix(log_fmt): replace chrono with jiff for timestamp formatting |
| `af7d12a` | fix(mcpregistry): surface upstream errors properly; add Upstream variant |
| `9ff7ded` | feat(mcpregistry): add sync observability — start/page/finish log events |
| `8e17b84` | fix(registry_v01): use axum 0.8 {param} route syntax instead of :param |
| `388c22e` | fix: squash serve/dispatch warnings (unnecessary qualifications, dead code) |

### Highlights

- **Marketplace UI** — full Marketplace page: types, mock API client, SWR hooks, card/panel/dialog/modal components, route + sidebar nav entry
- **Gateway admin REST wiring** — `listServers` now calls `GET /v0.1/servers`; gateway/registry/log/chat UI components updated (filters, table, detail panel, session sidebar, log console)
- **mcpregistry fixes** — sync guard extraction, SSRF blocklist (Tailscale CGNAT), `ON CONFLICT` upsert, WAL mode, jiff timestamp, upstream error surfacing, sync observability log events
- **Chrono → jiff migration** — removed `chrono` dep from workspace; log formatter uses `jiff`
- **Registry v0.1 API fixes** — axum 0.8 `{param}` route syntax, owner filter, `ToolError` normalization

### Version bumps

- Rust workspace: `0.7.2 → 0.7.3`

---

## [0.7.2] — 2026-04-22

| Commit | Change |
|--------|--------|
| `2caf21b` | feat(lab-h5pm.4): dispatch sync action with RAII AtomicBool rate-limit guard |
| `8233ac5` | feat(registry): use GitHub owner avatar as server image |
| `0d1acba` | feat(gateway-admin): aurora token sweep + eslint enforcement |
| `04a0dbd` | feat(lab-h5pm.2): implement RegistryStore query methods, upsert, and full sync |
| `96ddf66` | feat(lab-h5pm.1): create RegistryStore module skeleton in dispatch layer |

### Highlights

- **RegistryStore (lab-h5pm)** — SQLite-backed MCP server registry with skeleton, query/upsert/full-sync, and dispatch sync action protected by a RAII AtomicBool rate-limit guard
- **GitHub owner avatar** — registry list rows and detail header now pull `https://github.com/<owner>.png` from `server.repository.url`, falling back to `icons[0]` then a `Package` lucide icon
- **Aurora token sweep (product code)** — replaced shadcn-generic tokens (`text-muted-foreground`, `bg-card`, `bg-muted`, `bg-background`, `border-border`, `text-foreground`, `rounded-xl`) with Aurora equivalents across 19 files in `components/` and `app/`
- **ESLint enforcement** — new `no-restricted-syntax` rule bans the same tokens in `className` literals and template elements, scoped to `app/**` and `components/**` with `components/ui/**` exempted as the sanctioned escape hatch
- **Design-system contract** — added Authentication Surfaces section, banned-shadcn-token mapping table, eyebrow drift guidance, typography-ramp override rule, and Display Slot Assignments table
- **Brand icon polish** — gateway form brand chip now renders white-backed with colored border and SVG fill recoloring for stronger contrast
- **Test-compile repairs** — added `proxy_prompts` to `UpstreamConfig` literals across 4 files + `search` to `StoreListParams` literal; all-features tests compile clean

### Version bumps

- Rust workspace: `0.7.1 → 0.7.2`
- gateway-admin: `0.2.1 → 0.2.2`

---

## [0.7.1] — 2026-04-21

| Commit | Change |
|--------|--------|
| `52ef7d4` | refactor(ui): complete Aurora token sweep across all shadcn primitives — v0.7.1 |

### Highlights

- **Aurora token sweep** — complete theming of all `components/ui/` shadcn primitives: toggle, navigation-menu, skeleton, dialog, item, calendar, scroll-area, resizable, badge, checkbox, switch, radio-group, slider, dropdown-menu, select, alert, separator, accordion, progress, tabs, sonner, command, context-menu, menubar
- **Focus ring normalization** — all Radix primitives now use `aurora-accent-primary` rings instead of shadcn `ring-ring/50` defaults
- **Hover state normalization** — all `bg-accent`/`focus:bg-accent`/`hover:bg-accent` replaced with `aurora-hover-bg` across all menu and interactive components
- **Light mode fix** — `--aurora-hover-bg: #dcedf2` added to `.light` class (was dark-only)
- **`text-aurora-text-secondary` purge** — removed all 10 usages of the no-op token (not in `@theme inline`); replaced with `text-aurora-text-muted`
- **`aurora-scrollbar` utility** — added to `globals.css` for Firefox + WebKit scrollbar theming
- **`alert` success variant** — new `success` variant added to `alert.tsx`
- **JsonHighlight** — syntax-colored JSON renderer in `server-detail-panel.tsx`

### Version bumps

- Rust workspace: `0.7.0 → 0.7.1`
- gateway-admin: `0.2.0 → 0.2.1`

---

## [0.7.0] — 2026-04-21

| Commit | Change |
|--------|--------|
| `8cc9a59` | feat(gateway-admin): chat UI, registry enhancements, log toolbar refactor — v0.7.0 |
| `3eaa81c` | docs(observability): document ANSI sanitization, resource_uri redaction, and shell wrapper boundary |
| `762be6e` | feat(observability): add missing identifying fields to MCP/upstream warn events |
| `b09db3f` | feat(observability): normalize startup lifecycle events in lab serve |
| `0203829` | feat(formatter): extract PremiumEventFormatter into log_fmt/ with Axon-style semantic coloring |
| `234f7c4` | fix(security): sanitize log field values + redact upstream credentials |

### Highlights

- Chat UI (`components/chat/`, `app/(admin)/chat/`) and branding lib added to gateway-admin
- Registry: server detail panel expansion, filter sidebar, richer list content
- Log toolbar refactored; `log-filters.tsx` and `log-stream-status.tsx` consolidated
- Observability improvements: startup lifecycle events, MCP/upstream warn fields, ANSI sanitization
- `PremiumEventFormatter` extracted into `log_fmt/` with Axon-style semantic coloring
- Security: log field value sanitization + upstream credential redaction

---

## [0.6.1] — 2026-04-21

| Commit | Change |
|--------|--------|
| `9d1d355` | refactor(cli): wire CLI shims to shared dispatch + add --yes/--dry-run |
| `29e6166` | fix: restore plugins/ to repo |
| `a1058de` | chore: remove stale root plugin files and gh-webhook tool |

### Highlights

- All CLI service shims now delegate to the shared `dispatch/` layer
- `--yes` / `--dry-run` flags wired for destructive actions across all services
- Plugin asset hygiene pass

---

## [0.6.0] — 2026-04-21

| Commit | Change |
|--------|--------|
| `b13fb8a` | feat(auth): browser session + upstream pool + MCP peers |
| `4ddac44` | chore(plugin): restructure plugin assets under plugins/ |

### Highlights
- Browser session cookie management for services requiring login flows
- `dispatch/upstream/pool.rs` — upstream MCP proxy pool with circuit breaker
- MCP peer registry for multi-instance upstream routing

---

## [0.5.1] — 2026-04-21

| Commit | Change |
|--------|--------|
| `beb3de0` | chore(cli): action enum validation + plugin.json simplification |
| `86ed3c5` | feat(lab-aiit.1): stdio install dispatch + security hardening for mcpregistry |

### Highlights
- CLI action enum validated at parse time (unknown actions rejected early)
- mcpregistry stdio install path + SSRF/path-traversal hardening

---

## [0.5.0] — 2026-04-21

| Commit | Change |
|--------|--------|
| `d1a3ea6` | chore: v0.5.0 — gateway-admin redesign, deploy monitor, docs |
| `740ff96` | refactor(lab-5x4t): finish aurora palette sweep |
| `513bd48` | feat(lab-5x4t.5): add --aurora-preview-* tokens |
| `6d7731d` | feat(lab-5x4t.3): migrate components/gateway to aurora tokens |
| `0f2abb7` | feat(lab-5x4t.4): migrate components/logs to aurora tokens |
| `6938158` | feat(lab-5x4t.2): migrate auth login-screen to aurora tokens |
| `3dd6734` | feat(lab-5x4t.1): add --aurora-hover-bg token |
| `0cc38fd` | refactor(lab-x2nj): move aurora tokens to components/aurora/ |
| `b37e766` | fix(lab-abch): activate shadow-aurora-* utilities |

### Highlights
- Full Aurora design token sweep across gateway-admin UI
- Aurora token module extracted to `components/aurora/tokens.ts`
- Deploy monitor scaffolding added

---

## [0.4.1] — 2026-04-21

| Commit | Change |
|--------|--------|
| `aec694f` | chore: bump version to 0.4.1 |
| `55c6c36` | feat(lab-17th.12): register CLI implementation and skill docs |
| `de0505e` | feat(lab-17th.12): register binary, systemd unit, monitor |
| `4ec80d9` | feat(lab-17th.11): axum router handlers and graceful shutdown |
| `2ececa7` | feat(lab-17th.10): flush pipeline with atomic writes and watermark |
| `58e43d7` | feat(lab-17th.9): JSONL notification line enum with atomic append |
| `bd932e4` | feat(lab-17th.8): per-PR debouncer with generation counter |
| `4744429` | feat(lab-17th.7): digest rendering with dynamic fences |
| `64fb70e` | feat(lab-17th.6): GitHub REST client with pagination + SSRF guard |
| `1d2af2a` | feat(lab-17th.5): bounded FIFO delivery-id dedup cache |
| `591b583` | feat(lab-17th.4): typed event parsing with issue_comment PR filter |
| `35372f8` | feat(lab-17th.3): constant-time HMAC-SHA256 signature verification |
| `b7f5aad` | feat(lab-17th.2): config loader with redacted Debug and empty-secret rejection |
| `6c28391` | feat(lab-17th.1): scaffold gh-webhook crate |

### Highlights
- **gh-webhook crate** — full GitHub webhook ingestion pipeline: HMAC verification, event parsing, per-PR debouncer, digest renderer, atomic JSONL append, axum HTTP server
- Bounded FIFO dedup cache for delivery-id replay protection
- GitHub REST client with SSRF guard and 429 retry

---

## [0.4.0] — 2026-04-20

| Commit | Change |
|--------|--------|
| `48ee2db` | feat(lab-eixf.8): sandbox sections + token drift docs |
| `d4f16c9` | feat(lab-eixf.7): migrate Docs page to Aurora |
| `4cf7c99` | feat(lab-eixf.6): migrate Settings page to Aurora |
| `35a4426` | feat(lab-eixf.5): migrate Activity page to Aurora |
| `ffd67c4` | feat(lab-eixf.4): migrate Overview page to Aurora |
| `d6d1c76` | feat(lab-eixf.3): Aurora primitive variants (Card/Badge/Alert) |
| `0e5c410` | simplify: abort checks, deriveGatewayName extraction |
| `ebfbab9` | fix(lab-iwtf.13,19): gateway name validation and option handling |
| `7ac4bc6` | fix(lab-iwtf.7,10,13,15): installServer return type, polling fixes |
| `9c67663` | fix(lab-iwtf.3,4,14,17,18,29): SSRF probe, restart hazard, auth edge cases |
| `d8b71eb` | fix(lab-iwtf.6,12,16): HTTP 422 for SSRF kinds, replay-window fixes |
| `10fc672` | fix(lab-iwtf.2,8): popup user-activation and external-close fixes |
| `ea21977` | fix(lab-iwtf.1,5,9,11): OAuth patch drop, proxy_prompts dedup |
| `f39f119` | feat(cli): richer palette — violet categories, teal action names |
| `806f7f9` | feat(cli): premium palette + catalog/doctor renderers |

### Highlights
- Full Aurora migration for all gateway-admin pages (Overview, Activity, Settings, Docs)
- Aurora primitive component variants (Card, Badge, Alert)
- **mcpregistry security** — SSRF probe, replay-window guard, HTTP 422 error mapping
- OAuth upstream flow fixes (popup activation, external close, proxy_prompts dedup)
- Premium CLI output palette (violet categories, teal actions, semantic colors)
