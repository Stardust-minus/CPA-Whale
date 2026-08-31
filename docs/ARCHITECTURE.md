# Architecture

## Components

```text
CLIProxyAPI process
├── cpa-whale plugin (Linux cdylib)
│   ├── usage.handle consumer
│   ├── in-memory aggregate
│   ├── bounded SQLite writer
│   ├── quota adapter registry
│   ├── optional signal adapter registry
│   ├── capability discovery
│   └── authenticated resource API
│
└── existing provider executors / auth files

HTTPS + named Whale read token
              │
              ▼
CPA Whale Windows client
├── capability discovery / legacy fallback
├── WinHTTP snapshot polling
├── local startup baseline
├── DPAPI token storage
├── dynamic card/model settings
├── DirectComposition renderer
└── tray / menu / setup / details windows

Linux operator
└── cpa-whale-admin
    ├── check / token / config render
    ├── atomic install + backup manifest
    ├── doctor
    └── rollback
```

## Version boundaries

CPA Whale v0.3.1 uses separate compatibility numbers:

- application bundle version: `0.3.1`
- CPA plugin C ABI: `1`
- CPA JSON lifecycle schema: `4`
- snapshot API schema: `1`
- capabilities schema: `1`
- plugin config schema: `2`
- SQLite `PRAGMA user_version`: `2`

The snapshot DTO remains backward compatible with the v0.2.6 client. A v0.3.x client first requests capabilities; if an older plugin has no route, it synthesizes legacy capabilities from snapshot models, accounts and signals.

## Plugin lifecycle

Exported entry point:

```c
int cliproxy_plugin_init(
    const cliproxy_host_api* host,
    cliproxy_plugin_api* plugin
);
```

Handled CPA methods:

- `plugin.register`
- `plugin.reconfigure`
- `plugin.quiesce`
- `plugin.shutdown`
- `usage.handle`
- `management.register`
- `management.handle`

Capabilities:

- `usage_plugin`
- `management_api`

`usage.handle` is independent of CPA's built-in usage-statistics queue. The callback decodes PascalCase usage records, drops sensitive fields, computes pricing/quota through configured adapters, updates in-memory aggregates and enqueues a bounded SQLite write.

## Configuration

Config v2 separates deployment concerns:

```text
instance   display name, scope label, focus model, cards, poll interval
api        named read-token SHA-256 digests
storage    database, timezone, queue, raw/daily retention
quota      implemented adapters and account visibility policy
pricing    exact provider/model/alias/effort rates
signals    explicitly enabled source adapters
```

Unversioned v1 configuration remains accepted. Legacy fields are normalized into the runtime config; no secret or raw token is added during migration.

Config v2 safe defaults:

- timezone: UTC
- external signals: disabled
- quota visibility: compatible available quota accounts
- pricing: empty/unknown
- focus model: automatic

## Pricing

Prices are integer USD micros. A rate can match:

- optional provider
- one or more exact model IDs/aliases
- optional reasoning effort
- explicit priority

A model with no matching rate remains unknown. Unknown traffic never becomes zero-cost traffic. Daily rollups retain token categories and pricing version so a future supported repricing command can operate without relying only on raw events.

## Quota adapters

The adapter registry currently implements:

```text
codex-response-headers
```

It normalizes safe `X-Codex-*` fields into provider-neutral `QuotaSnapshot` and `QuotaWindow` DTOs. Provider, plan and active-limit filters belong to deployment config, not Windows client code.

Future providers require a new tested adapter; arbitrary user-defined header expressions are intentionally unsupported.

For the account-level remaining quota shown by the Windows client, an exact credential-level `primary` window is preferred. Namespaced windows such as `bengalfox primary` are model/additional limits and remain available in the API, but are not mixed into the account's headline remaining percentage. The UI labels the value as remaining rather than mirroring a management page that may label the complementary used percentage.

## Signal adapters

Config v2 supports these explicit adapters:

- `statuspage-v2`
- `codex-radar-intelligence`
- `divin-reset-events`
- `historical-risk-window`

Each source has a stable ID, display name, interval and adapter-specific filters. The worker tracks last attempt, last success and last error per source. Reconfiguration removes disabled source data.

Third-party sources are opt-in. Signals carry source/fetch/expiry timestamps, confidence and stale status. Community information is not represented as an official prediction.

## Capability discovery

Authenticated route:

```text
GET /v0/resource/plugins/cpa-whale/v1/capabilities
```

The response includes:

- instance name, scope and timezone
- attribution support
- pricing/quota/signal feature flags
- observed/configured model descriptors and effort options
- quota provider support
- default focus model/effort
- recommended poll interval and cards

Model descriptors merge current model traffic, pricing entries and intelligence signals. Display names come from config rather than GPT-specific string rewriting.

## Resource API

Routes below `/v0/resource/plugins/cpa-whale/v1/`:

- `capabilities`
- `snapshot`
- `models`
- `accounts`
- `signals`
- `health`

All validate a dedicated Whale Token. Config v2 supports multiple named token digests. The request token is hashed and compared against every configured digest in constant time. Diagnostics expose token IDs/count only, never digests.

Operator diagnostics remain separate under the Management-authenticated plugin route.

## Storage

SQLite uses:

- WAL
- `synchronous=NORMAL`
- `busy_timeout=5000`
- foreign keys
- explicit `PRAGMA user_version` migrations

Tables:

- `lifetime_totals`
- `usage_events`
- `quota_snapshots`
- `daily_model_usage`

Each usage transaction updates the raw event, lifetime total, daily provider/model/effort rollup and optional normalized quota snapshot.

Retention is tiered:

- `raw-events-retention-days`: removes detailed sanitized usage events
- `daily-retention-days`: removes daily model rollups
- lifetime totals remain cumulative

When an existing schema is upgraded, retained raw events backfill daily rollups without rewriting lifetime totals. A newer unsupported schema or failed migration rejects initialization.

Persisted data excludes prompts, responses, failure bodies, API keys, access tokens, IP, User-Agent, email, auth paths and raw headers.

## Windows client

The client is Rust native Win32 and uses:

- D3D11 hardware device with WARP fallback
- Direct2D geometry/bitmap rendering
- DirectWrite text
- DirectComposition premultiplied-alpha surfaces
- DWM refresh timing and `DwmFlush`
- WinHTTP
- DPAPI
- system tray and Per-Monitor DPI V2

First connection accepts a CPA root URL, full plugin endpoint or `CPAW1-...` code. It probes capabilities and snapshot before saving. Public plain HTTP requires a second confirmation.

Capability-driven behavior:

- automatic or GUI-selected focus model/effort
- server-provided model display names
- feature-aware card deck
- all server-visible quota providers
- generic service-status source names
- server-recommended polling interval clamped to 15–3600 seconds

The top-level DirectComposition surface remains square, but Win32 `SetWindowRgn` is rebuilt from cached whale-alpha scanline runs plus bubble/menu geometry. Pixels outside that visible region never participate in cross-process hit testing, so the transparent canvas does not block applications underneath.

A full-window click-through toggle is intentionally not exposed: `HTTRANSPARENT` only forwards within one UI thread, and disabling the window blocks clicks and produces a system warning sound. Region shaping solves the transparent-canvas problem without making the visible whale non-interactive.

The existing whale proportions, bubble paths, animation timings, alpha hit testing and event-driven idle behavior otherwise remain unchanged.

## Administration tool

`cpa-whale-admin` deliberately does not automate unknown Management API workflows.

- `check`: read-only deployment discovery
- `token generate`: OS-random 256-bit token/digest/connection code
- `config render`: config v2 YAML fragment
- `install`: ELF architecture validation, atomic versioned copy and backup manifest
- `doctor`: unauthenticated/authenticated API health validation
- `rollback`: restore manifest-recorded files without restarting CPA

The raw token is printed only by the explicit token command. Release assembly uses an explicit allowlist and never copies local `build-output` secrets.
