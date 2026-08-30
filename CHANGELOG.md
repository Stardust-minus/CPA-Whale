# Changelog

## v0.3.0 — 2026-08-30

First generalized open-source-ready bundle. Client, plugin and administration tool now share one release version.

### Added

- authenticated `/v1/capabilities` discovery route
- config schema v2 with instance/storage/API/quota/pricing/signal sections
- multiple named read-token digests
- provider/model/alias/effort pricing matches with priorities
- quota adapter registry and deployment-controlled account visibility
- opt-in signal adapter list with per-source diagnostics
- SQLite `PRAGMA user_version` migrations
- daily provider/model/effort rollups and working daily retention
- capability-driven client model/effort selection and data-card settings
- CPA root URL normalization and `CPAW1-...` connection code import
- connection probe before DPAPI save and public HTTP confirmation
- alpha-derived Win32 window region so transparent square pixels do not capture clicks
- Linux `cpa-whale-admin` check/token/config/install/doctor/rollback commands
- native and Docker deployment examples
- Cargo-metadata release builder, checksums and public-tree scan
- GitHub Actions test/build/ABI workflow

### Changed

- removed the maintainer production endpoint from client defaults
- removed GPT-5.6-specific model-name shortening
- removed the client-side Codex Pro Premium quota filter
- external third-party signals default to disabled in config v2
- new config defaults to UTC; unversioned config preserves legacy behavior
- build exports use stable generic filenames; release assembly applies versions
- User-Agent and metadata versions come from Cargo package metadata
- plugin metadata no longer points to the CLIProxyAPI repository as its own source
- all new release components use v0.3.0
- service-status cards compact common source suffixes so provider names fit the bubble

### Removed

- the unreliable full-window click-through option; visible-region shaping now handles transparent canvas pixels without disabling the window

### Compatibility

- `/v1/snapshot` remains schema v1 for old v0.2.6 clients
- v0.3.0 clients synthesize capabilities when connected to plugin v0.1.2
- old `read-token-sha256`, flat storage fields and `external-signals` remain accepted as legacy config
- no production deployment or database migration is performed by building this release

## Windows client v0.2.6 — 2026-08-30

Finalized initial private-deployment client.

### Added

- native D3D11 / Direct2D / DirectWrite / DirectComposition renderer
- DWM-refresh-aware animation scheduling and WARP fallback
- self-drawn connection, menu and details panels
- DPAPI-protected Whale read-only token
- original whale bubble geometry, Rua GIF, sounds and entertainment deck
- continuous scaling, snapping, mirroring, multi-monitor DPI and tray recovery
- embedded multi-size application icon

### Changed

- refresh opens the today bubble and displays loading feedback
- long text uses adaptive wrapping and line-aware vertical layout
- known-model USD subtotal is shown when strict aggregate pricing is incomplete
- UI terminology uses `CLIProxyAPI 今日`, `挂件启动后` and `CLIProxyAPI 统计`
- quota display was limited to the deployment's Codex Pro 20x accounts

### Removed

- previous-click interval metric
- double-click details action on the whale
- implementation labels such as D3D11/WARP from end-user panels
- CPU tiny-skia/fontdue production renderer

## CPA plugin v0.1.2 — 2026-08-30

Initial production plugin release.

### Added

- CLIProxyAPI C ABI v1 / JSON schema v4 integration
- `usage.handle` aggregation independent of built-in usage statistics
- privacy-minimized SQLite WAL storage
- lifetime/today/model/account aggregates
- exact Sol/Terra/Luna pricing profile used by the initial deployment
- Codex response-header quota normalization
- read-token-protected resource routes
- official/community external signal polling

### Changed

- restored aggregate state after historical pricing backfill
- exposed known priced model subtotals to the Windows client
