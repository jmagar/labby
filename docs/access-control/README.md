---
title: "Access Control, Workspaces, and Artifact Distribution"
created: "2026-08-22"
updated: "2026-08-22"
status: "design"
---

# Access Control, Workspaces, and Artifact Distribution

This folder is the canonical design packet for first-class multi-user authorization in Labby. It defines how organizations, departments/teams, projects, users, service accounts, Loadouts, Artifacts, and runtime MCP capabilities compose into an effective workspace.

The goal is not merely to add roles. Labby must be able to answer, consistently across CLI, API, MCP, Code Mode, and web UI:

- who is the caller;
- which organization, groups, and project are active;
- which assets and runtime capabilities the caller may discover and use;
- which Loadouts and mandatory baselines apply;
- which personal additions may overlay the project workspace;
- which Artifacts may be mirrored, followed, forked, exported, or reshared to another Labby;
- which credentials/runtime bindings may be used for the active project; and
- why access was allowed or refused without leaking hidden catalog contents.

## Design documents

- [SPEC.md](./SPEC.md) defines product behavior, user stories, invariants, and acceptance criteria.
- [CONTRACT.md](./CONTRACT.md) defines the normative domain contract and cross-surface behavior.
- [ARCHITECTURE.md](./ARCHITECTURE.md) defines component boundaries and how the resolver integrates with current Labby runtime filtering.
- [DATA_MODEL.md](./DATA_MODEL.md) defines the proposed persisted entities, identifiers, relationships, versioning, and migration rules.
- [PERMISSIONS.md](./PERMISSIONS.md) defines permission vocabulary, roles, inheritance, masking, and authorization evaluation.
- [PROJECT_CONTEXT.md](./PROJECT_CONTEXT.md) defines request/route/session-scoped Project binding across HTTP, CLI, web, MCP, Code Mode, background jobs, caches, and runtime credentials.
- [ARTIFACT_DISTRIBUTION.md](./ARTIFACT_DISTRIBUTION.md) defines Add to My Labby, managed mirrors, pin/follow/fork/export/reshare semantics, and revocation.
- [THREAT_MODEL.md](./THREAT_MODEL.md) defines security boundaries, abuse cases, and mandatory adversarial tests.
- [ENGINEERING_REVIEW.md](./ENGINEERING_REVIEW.md) records the pre-implementation architecture, simplicity, security, performance, failure-mode, and deferral review.
- [IMPLEMENTATION_PLAN.md](./IMPLEMENTATION_PLAN.md) defines the TDD implementation sequence and required verification gates.
- [PROGRESS.md](./PROGRESS.md) tracks decisions, implementation status, tests, migrations, and documentation follow-through.
- [MULTI_USER_AUTHORITY.md](./MULTI_USER_AUTHORITY.md) freezes the v1 multi-user/Team authority, ownership, projection, delegation, migration, recovery, and audit contract shared with Depot.
- [authority-matrix-v1.json](./authority-matrix-v1.json) is the checked role/capability/resource/service classification registry.
- [MIGRATION.md](./MIGRATION.md) defines the production-shaped ownership migration rehearsal, activation boundary, backup/restore set, quarantine, and rollback proof.

## Existing contracts this design preserves

### ArtifactInterchange v1 remains frozen

The canonical Artifact contract remains [../artifacts/contract.md](../artifacts/contract.md). This design MUST NOT add access-control fields to the frozen dinglebear.artifact-interchange/v1 envelope. Artifact identity, immutable revisions, content digests, provenance, license state, publication state, lineage, import/export safety, and provider acquisition remain owned by the Artifact subsystem.

Access policy references Artifact IDs and immutable revisions. Federation or sync authorization travels in a separate Labby access/transfer contract.

### OAuth scopes remain coarse transport authorization

labby-auth currently authenticates requests into an AuthContext containing issuer, subject, effective OAuth scopes, session state, and optional email/actor metadata. OAuth scopes such as lab:read, lab, and lab:admin remain coarse transport guards.

Domain authorization defined here runs after authentication. An OAuth scope is never equivalent to organization, department, project, or Artifact permission. A request must satisfy both layers where both apply.

### Gateway Loadouts remain runtime projections

GatewayLoadoutConfig already selects upstreams/services and gates tools, resources, prompts, skills, and Code Mode. The current contract intentionally only narrows capability exposure.

The access-control system MUST NOT grow department/user ACL fields directly into GatewayLoadoutConfig. Instead, the shared resolver produces an EffectiveWorkspace that compiles to and narrows existing gateway catalog and Loadout projections.

## Locked design principles

1. **Default deny.** No membership or grant means no domain access.
2. **One shared authorization layer.** CLI, HTTP/API, MCP, Code Mode, and web UI consume the same resolver result and do not reimplement policy.
3. **Authorization-aware discovery.** Unauthorized tools, resources, prompts, skills, Loadouts, and Artifacts are omitted from catalog/list/search surfaces whenever possible. Direct invocation is still re-authorized.
4. **Shared immutable assets, scoped assignments.** Projects and groups do not need private copies of every Artifact. They hold assignments/policy over shared immutable Artifact revisions or runtime capability references.
5. **Groups model organizational structure.** Department, team, squad, business unit, and similar labels are group kinds, not separate authorization engines.
6. **Projects remain first-class.** Projects carry active workspace, Loadout, runtime binding, secret, session, and audit significance and are not merely another group label.
7. **Personal overlay without privilege expansion.** A user's personal Labby/assets may overlay a project workspace only where project policy allows. Personal additions can never grant runtime authority that the project/user does not already possess.
8. **Use, copy, fork, and reshare are distinct rights.** Visibility or use permission never implies permission to move Artifact bytes elsewhere.
9. **License and publisher policy cap distribution.** An access grant cannot override Artifact redistribution/license restrictions.
10. **Revocation is enforced at runtime.** Catalog caches, sessions, managed mirrors, and runtime bindings must respond to membership/policy revocation.
11. **Explainability is first-class.** Policy evaluation records bounded reason/evidence chains for administrators and audit without exposing names of hidden assets to unauthorized callers.
12. **No generic explicit deny algebra in v1.** V1 uses default deny, positive grants, inheritance boundaries, and narrowly scoped inherited-assignment masking. It does not reproduce an AWS-style allow/deny policy language.

## Primary domain vocabulary

- **Principal:** authenticated user or service account.
- **Subject:** principal or group receiving a membership/grant.
- **Organization:** top-level administrative boundary.
- **Group:** nestable organizational unit such as department or team.
- **Project:** first-class work/runtime scope within an organization.
- **Scope:** personal, organization, group, or project policy boundary.
- **Asset:** Artifact-backed content or a stable reference to a runtime capability/Loadout.
- **Assignment:** makes an asset available in a scope and defines inheritance/override behavior.
- **Grant:** gives a subject one or more permissions in a scope.
- **Role:** named permission bundle used by memberships/grants.
- **EffectiveWorkspace:** deterministic, authorization-filtered projection for one principal in one active project/context.
- **Managed mirror:** locally materialized Artifact whose source remains authoritative.
- **Personal fork:** independent Artifact identity created from an explicitly forkable revision while retaining lineage.

## Current status

This packet is a design and implementation contract. No access-control persistence, resolver, federation protocol, or UI behavior described here should be documented elsewhere as already implemented until the corresponding progress item and tests are complete.

Engineering review narrowed the first implementation milestone to canonical identity, explicit owner bootstrap, direct Project membership, one existing named Gateway Loadout per Project, server-owned MCP Project binding, authorization-aware discovery, and direct-call reauthorization. Groups, generalized Grants and Assignment composition, personal overlays, Artifact distribution, destination federation, project credentials, persistent explanations, and caching remain roadmap capabilities, not requirements for the first enforcement milestone.
