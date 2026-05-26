# Architecture Decision Records

This directory contains accepted architecture decisions for Lab.

ADRs record decisions that should stay stable across implementation plans,
roadmaps, and temporary migration notes. They complement the topic docs under
`docs/` and should link back to the source material that motivated them.

## Records

- [0001: Extract Lab as Reusable Rust and TypeScript Packages](./0001-extract-lab-as-reusable-packages.md)
- [0002: Split Shared Platform Crates from Product Runtime Crates](./0002-shared-platform-and-product-runtime-crates.md)
- [0003: Compose Products Through Runtime Builders](./0003-product-runtime-builders.md)
- [0004: Separate REST Admin APIs from MCP Action Dispatch](./0004-rest-admin-and-mcp-action-surfaces.md)
- [0005: Generate TypeScript Clients from REST OpenAPI](./0005-typescript-client-generation-from-openapi.md)
- [0006: Package Reusable Admin UI as Lab Web](./0006-lab-web-frontend-package-boundary.md)
- [0007: Use Semver with Workspace-First Extraction and Git Tags](./0007-versioning-and-distribution.md)
- [0008: Execute Extraction with Isolated Lanes and Integration Ownership](./0008-extraction-execution-lanes.md)
- [0009: Require Boundary and Generated-Client Verification](./0009-extraction-verification-gates.md)
