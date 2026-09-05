# CLIProxyAPI plugin deployment

This guide installs CPA Whale without distributing the CLIProxyAPI Management Key to Windows clients. It supports native Linux deployments and containerized CPA deployments.

Verified development baseline: CLIProxyAPI v7.2.145 with dynamic C-ABI plugin support. Confirm the target process reports plugin support before activation.

## Release files

A v0.3.3 release bundle contains:

```text
cpa-whale-plugin-v0.3.3-linux-amd64.so
cpa-whale-admin-v0.3.3-linux-amd64
cpa-whale-v0.3.3-windows-x64.exe
plugin-config.example.yaml
pricing-gpt-5.6.example.yaml
pricing-gpt-6-astra.example.yaml
docker-compose.fragment.yaml
release-manifest.json
SHA256SUMS
```

Verify `SHA256SUMS` before installation. Do not use an unverified `curl | sh` pipeline for a plugin loaded into the CPA process.

## 1. Preflight

```bash
./cpa-whale-admin-v0.3.3-linux-amd64 check \
  --config /etc/cliproxyapi/config.yaml
```

The tool reports:

- host architecture
- native/container context
- discovered CPA configuration
- configured plugin directory
- installed CPA version when available
- required plugin-support verification

It does not modify the host.

## 2. Generate a client token

```bash
./cpa-whale-admin-v0.3.3-linux-amd64 token generate \
  --endpoint https://your-cpa.example
```

Output includes:

- `WHALE_READ_TOKEN`: raw read-only token, given only to the intended client
- `WHALE_READ_TOKEN_SHA256`: digest stored in plugin config
- `WHALE_CONNECTION_CODE`: optional single-field Windows connection code

The connection code contains the raw read token. It is compact, not encrypted, and must be protected like the token itself.

Config v2 supports multiple tokens:

```yaml
api:
  read-tokens:
    - id: desktop
      sha256: <digest>
    - id: laptop
      sha256: <different digest>
```

Removing one entry revokes only that client after plugin reconfiguration/reload.

## 3. Render a config fragment

```bash
./cpa-whale-admin-v0.3.3-linux-amd64 config render \
  --token-id desktop \
  --token-sha256 "$WHALE_READ_TOKEN_SHA256" \
  --database /var/lib/cliproxyapi/whale/metrics.db \
  --timezone UTC
```

The command prints config v2 YAML. Merge it into the existing CPA config. The tool intentionally does not rewrite an unknown YAML file or discard its comments.

A complete template is [`plugin-config.example.yaml`](plugin-config.example.yaml).

### Deployment-specific choices

Review these fields before activation:

- reporting timezone
- raw event and daily rollup retention
- named read tokens
- instance display name
- quota adapters and account visibility
- pricing catalog
- external signal sources

Config v2 defaults third-party signals to disabled. Optional pricing fragments are available for [GPT-5.6](pricing-gpt-5.6.example.yaml) and [GPT-6 Astra](pricing-gpt-6-astra.example.yaml); use them only if the deployment exposes those exact IDs and accepts those equivalent rates.

### Add GPT-6 Astra to an existing deployment

`gpt-6-astra` uses the existing dynamic model discovery and pricing path; plugin v0.3.0 and client v0.3.1 already support its usage and pricing. Use client v0.3.3 or later for configured display names in the model details panel as well as the bubble and data settings.

1. Append the rate from [`pricing-gpt-6-astra.example.yaml`](pricing-gpt-6-astra.example.yaml) to `plugins.configs.cpa-whale.pricing.rates`. Preserve existing Sol/Terra/Luna or other rates; do not replace the entire catalog or add a second `pricing` key.
2. Update `pricing.version` to identify the combined catalog. The supplied rates are **OpenAI Flex equivalents provided by the deployment owner on 2026-09-05**: input $5.00/M, output $25.00/M, cache read $0.50/M. Reasoning uses the output rate without double charging; cache writes use the input rate as an explicit estimate because no separate rate was supplied. This does not detect or enforce the actual request service tier.
3. The fragment matches the exact model ID across providers. Add `provider: codex` (or the provider recorded by your CPA) if the estimate should apply only to that provider.
4. Apply the config through the existing plugin reconfiguration/reload workflow. Restart/reconnect the widget if its model selector still shows cached capabilities, then select **Astra** in data settings. Existing model preferences are preserved.
5. If an enabled intelligence source has an `include-models` allowlist, append `gpt-6-astra` without removing existing entries. An empty allowlist already accepts all models. Astra intelligence appears only if the source actually publishes a matching model/effort; old-model values are never substituted.

Pricing applies to newly observed requests after reconfiguration. Previously unpriced history is not automatically recalculated.

## 4. Install the plugin file

```bash
sudo ./cpa-whale-admin-v0.3.3-linux-amd64 install \
  --plugin ./cpa-whale-plugin-v0.3.3-linux-amd64.so \
  --config /etc/cliproxyapi/config.yaml \
  --database /var/lib/cliproxyapi/whale/metrics.db
```

The tool:

1. validates the ELF architecture;
2. detects the configured plugin directory;
3. creates required directories;
4. copies the plugin atomically under a versioned filename;
5. backs up an existing same-version file;
6. backs up the config and Whale database when present;
7. writes `/var/lib/cliproxyapi/whale/install-manifest.json`;
8. prints the installed SHA-256.

It does **not**:

- rewrite CPA config automatically;
- read or distribute the Management Key;
- activate the plugin through Management API;
- restart CLIProxyAPI.

Enable/reload the plugin through the deployment's existing Management UI/API workflow. A service restart should not be the normal installation path.

Do not delete an older `.so` until CPA confirms it is no longer active or mapped by the process.

## 5. Native deployment layout

Typical paths:

```text
/etc/cliproxyapi/config.yaml
/var/lib/cliproxyapi/plugins/linux/amd64/
/var/lib/cliproxyapi/whale/metrics.db
/var/lib/cliproxyapi/whale/install-manifest.json
```

The CPA process user needs:

- read/execute access to the plugin `.so`;
- read/write access to the Whale data directory;
- permission to create SQLite WAL/SHM files.

## 6. Docker / Compose deployment

Merge [`docker-compose.fragment.yaml`](docker-compose.fragment.yaml) into the existing CPA service. Preserve the upstream image, command, ports and environment.

Required persistent mounts:

```yaml
volumes:
  - ./cpa-plugins:/var/lib/cliproxyapi/plugins
  - ./cpa-whale-data:/var/lib/cliproxyapi/whale
```

The config inside the container must use the matching container paths. Ensure the host directories are writable by the container UID/GID.

For a containerized CPA, the admin tool may be run:

- on the host with explicit `--plugin-dir` and `--state-dir`; or
- in a one-shot utility container sharing the same volumes.

Do not bake the raw Whale token into an image layer.

## 7. Resource API

Authenticated routes:

```text
GET /v0/resource/plugins/cpa-whale/v1/capabilities
GET /v0/resource/plugins/cpa-whale/v1/snapshot
GET /v0/resource/plugins/cpa-whale/v1/models
GET /v0/resource/plugins/cpa-whale/v1/accounts
GET /v0/resource/plugins/cpa-whale/v1/signals
GET /v0/resource/plugins/cpa-whale/v1/health
```

Every request requires:

```http
Authorization: Bearer <Whale read token>
```

Management-only diagnostics:

```text
/v0/management/plugins/cpa-whale/diagnostics
```

The Management Key is not accepted as a substitute by the Windows client and must not be copied into its configuration.

## 8. Verify

```bash
WHALE_READ_TOKEN='<raw token>' \
./cpa-whale-admin-v0.3.3-linux-amd64 doctor \
  --endpoint https://your-cpa.example
```

Doctor checks:

- unauthenticated health returns 401
- capabilities schema is present
- authenticated health and snapshot decode correctly
- database/writer health are good
- dropped events remain zero

Also verify organic traffic increments `sequence` and token totals.

Privacy review should confirm no prompt, response, API key, access token, IP, User-Agent, email, auth path or raw response headers appear in JSON/SQLite.

## 9. Windows client

Launch the EXE normally. On first launch, enter either:

- CPA root URL and raw Whale Token;
- full Whale API root and token; or
- the `CPAW1-...` connection code.

The connection panel validates capabilities/snapshot before saving. Windows protects the token with DPAPI.

## Upgrade

1. Verify the new release checksums.
2. Run `check` and `install` with the new versioned `.so`.
3. Keep the previous version file and backups.
4. Reconfigure/hot-load through the existing Management workflow.
5. Run `doctor`.
6. Remove older files only after CPA has released them.

SQLite uses explicit `PRAGMA user_version` migrations. A failed migration rejects plugin initialization rather than silently running a partial schema.

## Rollback

First disable or switch the active plugin through the existing Management workflow. Then:

```bash
sudo ./cpa-whale-admin-v0.3.3-linux-amd64 rollback \
  --manifest /var/lib/cliproxyapi/whale/install-manifest.json
```

Rollback restores the replaced plugin/config backups recorded by the manifest or removes a newly installed unreferenced file. It does not restart CPA.

If the target CPA version cannot unload/reload plugins safely, schedule and approve a service restart separately.
