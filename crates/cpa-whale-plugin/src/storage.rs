use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use chrono::{DateTime, Utc};
use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use whale_core::reporting_day;
use whale_protocol::{QuotaSnapshot, TokenUsage, UsageTotals};

use crate::config::PluginConfig;
use crate::usage::SanitizedUsage;

const DATABASE_SCHEMA_VERSION: i64 = 2;

pub type RestoredAccount = (String, UsageTotals, QuotaSnapshot, Option<DateTime<Utc>>);
pub type RestoredAccounts = BTreeMap<String, RestoredAccount>;

pub struct RestoredState {
    pub sequence: u64,
    pub reporting_day: String,
    pub all_time: UsageTotals,
    pub today: UsageTotals,
    pub models: BTreeMap<(String, String, String), UsageTotals>,
    pub accounts: RestoredAccounts,
    pub last_event_at: Option<DateTime<Utc>>,
}

#[derive(Clone)]
pub struct PersistedUsage {
    pub sequence: u64,
    pub reporting_day: String,
    pub usage: SanitizedUsage,
}

pub struct WriterHealth {
    pub database_ok: AtomicBool,
    pub writer_ok: AtomicBool,
    pub dropped_events: AtomicU64,
    pub last_error: Mutex<Option<String>>,
}

impl Default for WriterHealth {
    fn default() -> Self {
        Self {
            database_ok: AtomicBool::new(false),
            writer_ok: AtomicBool::new(false),
            dropped_events: AtomicU64::new(0),
            last_error: Mutex::new(None),
        }
    }
}

enum WriteMessage {
    Usage(Box<PersistedUsage>),
    Shutdown,
}

pub struct StorageHandle {
    sender: Sender<WriteMessage>,
    join: Mutex<Option<JoinHandle<()>>>,
    health: Arc<WriterHealth>,
}

impl StorageHandle {
    pub fn open(config: &PluginConfig) -> Result<(Self, RestoredState), String> {
        ensure_parent(&config.database)?;
        let mut connection = Connection::open(&config.database)
            .map_err(|error| format!("open database {}: {error}", config.database))?;
        migrate(&mut connection)?;
        let restored = load_state(&connection, &config.timezone)?;
        cleanup(
            &connection,
            config.raw_events_retention_days,
            config.daily_retention_days,
        )?;

        let (sender, receiver) = bounded(config.queue_capacity);
        let health = Arc::new(WriterHealth::default());
        health.database_ok.store(true, Ordering::Relaxed);
        health.writer_ok.store(true, Ordering::Relaxed);
        let writer_health = Arc::clone(&health);
        let raw_retention_days = config.raw_events_retention_days;
        let daily_retention_days = config.daily_retention_days;
        let join = thread::Builder::new()
            .name("cpa-whale-sqlite".into())
            .spawn(move || {
                writer_loop(
                    &mut connection,
                    receiver,
                    raw_retention_days,
                    daily_retention_days,
                    writer_health,
                )
            })
            .map_err(|error| format!("start database writer: {error}"))?;
        Ok((
            Self {
                sender,
                join: Mutex::new(Some(join)),
                health,
            },
            restored,
        ))
    }

    pub fn try_enqueue(&self, usage: PersistedUsage) -> bool {
        match self.sender.try_send(WriteMessage::Usage(Box::new(usage))) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.health.dropped_events.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    pub fn queue_depth(&self) -> usize {
        self.sender.len()
    }

    pub fn health(&self) -> Arc<WriterHealth> {
        Arc::clone(&self.health)
    }

    pub fn shutdown(&self) {
        let _ = self.sender.send(WriteMessage::Shutdown);
        if let Some(join) = self.join.lock().take() {
            let _ = join.join();
        }
    }
}

fn writer_loop(
    connection: &mut Connection,
    receiver: Receiver<WriteMessage>,
    raw_retention_days: i64,
    daily_retention_days: i64,
    health: Arc<WriterHealth>,
) {
    let mut processed_since_cleanup = 0_u64;
    loop {
        let first = match receiver.recv_timeout(Duration::from_millis(500)) {
            Ok(WriteMessage::Usage(usage)) => *usage,
            Ok(WriteMessage::Shutdown) => break,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };
        let mut batch = vec![first];
        let mut shutdown = false;
        while batch.len() < 100 {
            match receiver.try_recv() {
                Ok(WriteMessage::Usage(usage)) => batch.push(*usage),
                Ok(WriteMessage::Shutdown) => {
                    shutdown = true;
                    break;
                }
                Err(_) => break,
            }
        }
        match write_batch(connection, &batch) {
            Ok(()) => {
                health.database_ok.store(true, Ordering::Relaxed);
                health.writer_ok.store(true, Ordering::Relaxed);
                *health.last_error.lock() = None;
            }
            Err(error) => {
                health.database_ok.store(false, Ordering::Relaxed);
                health.writer_ok.store(false, Ordering::Relaxed);
                *health.last_error.lock() = Some(error);
            }
        }
        processed_since_cleanup = processed_since_cleanup.saturating_add(batch.len() as u64);
        if processed_since_cleanup >= 1000 {
            if let Err(error) = cleanup(connection, raw_retention_days, daily_retention_days) {
                *health.last_error.lock() = Some(error);
            }
            processed_since_cleanup = 0;
        }
        if shutdown {
            break;
        }
    }
    health.writer_ok.store(false, Ordering::Relaxed);
}

fn write_batch(connection: &mut Connection, batch: &[PersistedUsage]) -> Result<(), String> {
    let transaction = connection
        .transaction()
        .map_err(|error| format!("begin database transaction: {error}"))?;
    for item in batch {
        insert_usage(&transaction, item)?;
    }
    transaction
        .commit()
        .map_err(|error| format!("commit database transaction: {error}"))
}

fn insert_usage(transaction: &Transaction<'_>, item: &PersistedUsage) -> Result<(), String> {
    let usage = &item.usage;
    transaction
        .execute(
            "INSERT INTO usage_events (
                sequence, requested_at, reporting_day, provider, executor_type, model, alias,
                reasoning_effort, service_tier, auth_index, auth_type, failed, status_code,
                latency_ms, ttft_ms, input_tokens, output_tokens, reasoning_tokens,
                cached_tokens, cache_read_tokens, cache_write_tokens, total_tokens,
                pricing_version, estimated_usd_micros
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                       ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)",
            params![
                item.sequence as i64,
                usage.requested_at.to_rfc3339(),
                item.reporting_day,
                usage.provider,
                usage.executor_type,
                usage.model,
                usage.alias,
                usage.reasoning_effort,
                usage.service_tier,
                usage.auth_index,
                usage.auth_type,
                i64::from(usage.failed),
                usage.status_code,
                usage.latency_ms,
                usage.ttft_ms,
                usage.tokens.input_tokens,
                usage.tokens.output_tokens,
                usage.tokens.reasoning_tokens,
                usage.tokens.cached_tokens,
                usage.tokens.cache_read_tokens,
                usage.tokens.cache_write_tokens,
                usage.tokens.total_tokens,
                usage.pricing_version,
                usage.estimated_usd_micros,
            ],
        )
        .map_err(|error| format!("insert usage event: {error}"))?;

    let priced = i64::from(usage.estimated_usd_micros.is_some());
    transaction
        .execute(
            "UPDATE lifetime_totals SET
                sequence = ?1,
                requests = requests + 1,
                successful_requests = successful_requests + ?2,
                failed_requests = failed_requests + ?3,
                input_tokens = input_tokens + ?4,
                output_tokens = output_tokens + ?5,
                reasoning_tokens = reasoning_tokens + ?6,
                cached_tokens = cached_tokens + ?7,
                cache_read_tokens = cache_read_tokens + ?8,
                cache_write_tokens = cache_write_tokens + ?9,
                total_tokens = total_tokens + ?10,
                estimated_usd_micros = CASE
                  WHEN usd_complete = 1 AND ?11 = 1
                  THEN COALESCE(estimated_usd_micros, 0) + ?12
                  ELSE NULL END,
                usd_complete = CASE WHEN usd_complete = 1 AND ?11 = 1 THEN 1 ELSE 0 END
             WHERE id = 1",
            params![
                item.sequence as i64,
                i64::from(!usage.failed),
                i64::from(usage.failed),
                usage.tokens.input_tokens,
                usage.tokens.output_tokens,
                usage.tokens.reasoning_tokens,
                usage.tokens.cached_tokens,
                usage.tokens.cache_read_tokens,
                usage.tokens.cache_write_tokens,
                usage.tokens.total_tokens,
                priced,
                usage.estimated_usd_micros.unwrap_or(0),
            ],
        )
        .map_err(|error| format!("update lifetime totals: {error}"))?;

    transaction
        .execute(
            "INSERT INTO daily_model_usage (
                reporting_day, provider, model, reasoning_effort, requests,
                successful_requests, failed_requests, input_tokens, output_tokens,
                reasoning_tokens, cached_tokens, cache_read_tokens, cache_write_tokens,
                total_tokens, pricing_version, estimated_usd_micros, usd_complete
             ) VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                       ?13, ?14, ?15, ?16)
             ON CONFLICT(reporting_day, provider, model, reasoning_effort) DO UPDATE SET
                requests = requests + 1,
                successful_requests = successful_requests + excluded.successful_requests,
                failed_requests = failed_requests + excluded.failed_requests,
                input_tokens = input_tokens + excluded.input_tokens,
                output_tokens = output_tokens + excluded.output_tokens,
                reasoning_tokens = reasoning_tokens + excluded.reasoning_tokens,
                cached_tokens = cached_tokens + excluded.cached_tokens,
                cache_read_tokens = cache_read_tokens + excluded.cache_read_tokens,
                cache_write_tokens = cache_write_tokens + excluded.cache_write_tokens,
                total_tokens = total_tokens + excluded.total_tokens,
                estimated_usd_micros = CASE
                  WHEN usd_complete = 1 AND excluded.usd_complete = 1
                  THEN COALESCE(estimated_usd_micros, 0) + excluded.estimated_usd_micros
                  ELSE NULL END,
                usd_complete = CASE
                  WHEN usd_complete = 1 AND excluded.usd_complete = 1 THEN 1 ELSE 0 END,
                pricing_version = CASE
                  WHEN pricing_version IS NULL THEN excluded.pricing_version
                  WHEN excluded.pricing_version IS NULL THEN pricing_version
                  WHEN pricing_version = excluded.pricing_version THEN pricing_version
                  ELSE 'mixed' END",
            params![
                item.reporting_day,
                usage.provider,
                usage.model,
                usage.reasoning_effort,
                i64::from(!usage.failed),
                i64::from(usage.failed),
                usage.tokens.input_tokens,
                usage.tokens.output_tokens,
                usage.tokens.reasoning_tokens,
                usage.tokens.cached_tokens,
                usage.tokens.cache_read_tokens,
                usage.tokens.cache_write_tokens,
                usage.tokens.total_tokens,
                usage.pricing_version,
                usage.estimated_usd_micros,
                priced,
            ],
        )
        .map_err(|error| format!("update daily model rollup: {error}"))?;

    if let Some(quota) = usage
        .quota
        .as_ref()
        .filter(|_| !usage.auth_index.is_empty())
    {
        let quota_json = serde_json::to_string(quota)
            .map_err(|error| format!("encode quota snapshot: {error}"))?;
        transaction
            .execute(
                "INSERT INTO quota_snapshots (auth_index, provider, observed_at, quota_json)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(auth_index) DO UPDATE SET
                   provider=excluded.provider,
                   observed_at=excluded.observed_at,
                   quota_json=excluded.quota_json",
                params![
                    usage.auth_index,
                    usage.provider,
                    usage.requested_at.to_rfc3339(),
                    quota_json,
                ],
            )
            .map_err(|error| format!("upsert quota snapshot: {error}"))?;
    }
    Ok(())
}

fn migrate(connection: &mut Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA busy_timeout=5000;
             PRAGMA foreign_keys=ON;",
        )
        .map_err(|error| format!("configure database: {error}"))?;
    let version = connection
        .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
        .map_err(|error| format!("read database schema version: {error}"))?;
    if version > DATABASE_SCHEMA_VERSION {
        return Err(format!(
            "database schema {version} is newer than supported {DATABASE_SCHEMA_VERSION}"
        ));
    }

    let transaction = connection
        .transaction()
        .map_err(|error| format!("begin database migration: {error}"))?;
    transaction
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS lifetime_totals (
               id INTEGER PRIMARY KEY CHECK(id=1), sequence INTEGER NOT NULL DEFAULT 0,
               requests INTEGER NOT NULL DEFAULT 0,
               successful_requests INTEGER NOT NULL DEFAULT 0,
               failed_requests INTEGER NOT NULL DEFAULT 0,
               input_tokens INTEGER NOT NULL DEFAULT 0,
               output_tokens INTEGER NOT NULL DEFAULT 0,
               reasoning_tokens INTEGER NOT NULL DEFAULT 0,
               cached_tokens INTEGER NOT NULL DEFAULT 0,
               cache_read_tokens INTEGER NOT NULL DEFAULT 0,
               cache_write_tokens INTEGER NOT NULL DEFAULT 0,
               total_tokens INTEGER NOT NULL DEFAULT 0,
               estimated_usd_micros INTEGER,
               usd_complete INTEGER NOT NULL DEFAULT 1
             );
             INSERT OR IGNORE INTO lifetime_totals (id) VALUES (1);
             CREATE TABLE IF NOT EXISTS usage_events (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               sequence INTEGER NOT NULL UNIQUE,
               requested_at TEXT NOT NULL,
               reporting_day TEXT NOT NULL,
               provider TEXT NOT NULL,
               executor_type TEXT NOT NULL,
               model TEXT NOT NULL,
               alias TEXT NOT NULL,
               reasoning_effort TEXT NOT NULL,
               service_tier TEXT NOT NULL,
               auth_index TEXT NOT NULL,
               auth_type TEXT NOT NULL,
               failed INTEGER NOT NULL,
               status_code INTEGER NOT NULL,
               latency_ms INTEGER NOT NULL,
               ttft_ms INTEGER NOT NULL,
               input_tokens INTEGER NOT NULL,
               output_tokens INTEGER NOT NULL,
               reasoning_tokens INTEGER NOT NULL,
               cached_tokens INTEGER NOT NULL,
               cache_read_tokens INTEGER NOT NULL,
               cache_write_tokens INTEGER NOT NULL,
               total_tokens INTEGER NOT NULL,
               pricing_version TEXT,
               estimated_usd_micros INTEGER
             );
             CREATE INDEX IF NOT EXISTS usage_events_day_idx ON usage_events(reporting_day);
             CREATE INDEX IF NOT EXISTS usage_events_model_idx ON usage_events(reporting_day, provider, model, reasoning_effort);
             CREATE INDEX IF NOT EXISTS usage_events_auth_idx ON usage_events(reporting_day, auth_index);
             CREATE TABLE IF NOT EXISTS quota_snapshots (
               auth_index TEXT PRIMARY KEY,
               provider TEXT NOT NULL,
               observed_at TEXT NOT NULL,
               quota_json TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS daily_model_usage (
               reporting_day TEXT NOT NULL,
               provider TEXT NOT NULL,
               model TEXT NOT NULL,
               reasoning_effort TEXT NOT NULL,
               requests INTEGER NOT NULL DEFAULT 0,
               successful_requests INTEGER NOT NULL DEFAULT 0,
               failed_requests INTEGER NOT NULL DEFAULT 0,
               input_tokens INTEGER NOT NULL DEFAULT 0,
               output_tokens INTEGER NOT NULL DEFAULT 0,
               reasoning_tokens INTEGER NOT NULL DEFAULT 0,
               cached_tokens INTEGER NOT NULL DEFAULT 0,
               cache_read_tokens INTEGER NOT NULL DEFAULT 0,
               cache_write_tokens INTEGER NOT NULL DEFAULT 0,
               total_tokens INTEGER NOT NULL DEFAULT 0,
               pricing_version TEXT,
               estimated_usd_micros INTEGER,
               usd_complete INTEGER NOT NULL DEFAULT 1,
               PRIMARY KEY(reporting_day, provider, model, reasoning_effort)
             );",
        )
        .map_err(|error| format!("create database schema: {error}"))?;

    if version < 2 {
        transaction
            .execute_batch(
                "INSERT INTO daily_model_usage (
                   reporting_day, provider, model, reasoning_effort, requests,
                   successful_requests, failed_requests, input_tokens, output_tokens,
                   reasoning_tokens, cached_tokens, cache_read_tokens, cache_write_tokens,
                   total_tokens, pricing_version, estimated_usd_micros, usd_complete
                 )
                 SELECT reporting_day, provider, model, reasoning_effort,
                   COUNT(*),
                   SUM(CASE WHEN failed=0 THEN 1 ELSE 0 END),
                   SUM(CASE WHEN failed<>0 THEN 1 ELSE 0 END),
                   SUM(input_tokens), SUM(output_tokens), SUM(reasoning_tokens),
                   SUM(cached_tokens), SUM(cache_read_tokens), SUM(cache_write_tokens),
                   SUM(total_tokens),
                   CASE WHEN MIN(pricing_version)=MAX(pricing_version)
                        THEN MAX(pricing_version) ELSE 'mixed' END,
                   CASE WHEN COUNT(*)=COUNT(estimated_usd_micros)
                        THEN SUM(estimated_usd_micros) ELSE NULL END,
                   CASE WHEN COUNT(*)=COUNT(estimated_usd_micros) THEN 1 ELSE 0 END
                 FROM usage_events
                 GROUP BY reporting_day, provider, model, reasoning_effort
                 ON CONFLICT(reporting_day, provider, model, reasoning_effort) DO NOTHING;",
            )
            .map_err(|error| format!("backfill daily model rollups: {error}"))?;
    }
    transaction
        .execute_batch(&format!("PRAGMA user_version={DATABASE_SCHEMA_VERSION};"))
        .map_err(|error| format!("set database schema version: {error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("commit database migration: {error}"))
}

fn load_state(connection: &Connection, timezone: &str) -> Result<RestoredState, String> {
    let day = reporting_day(Utc::now(), timezone);
    let (sequence, all_time) = connection
        .query_row(
            "SELECT sequence, requests, successful_requests, failed_requests,
                    input_tokens, output_tokens, reasoning_tokens, cached_tokens,
                    cache_read_tokens, cache_write_tokens, total_tokens,
                    estimated_usd_micros
             FROM lifetime_totals WHERE id=1",
            [],
            |row| Ok((row.get::<_, i64>(0)? as u64, totals_from_row(row, 1)?)),
        )
        .map_err(|error| format!("load lifetime totals: {error}"))?;

    let today = query_totals(
        connection,
        "SELECT COUNT(*),
                SUM(CASE WHEN failed=0 THEN 1 ELSE 0 END),
                SUM(CASE WHEN failed<>0 THEN 1 ELSE 0 END),
                SUM(input_tokens), SUM(output_tokens), SUM(reasoning_tokens),
                SUM(cached_tokens), SUM(cache_read_tokens), SUM(cache_write_tokens),
                SUM(total_tokens),
                CASE WHEN COUNT(*)=COUNT(estimated_usd_micros)
                     THEN SUM(estimated_usd_micros) ELSE NULL END
         FROM usage_events WHERE reporting_day=?1",
        &day,
    )?;

    let mut models = BTreeMap::new();
    let mut statement = connection
        .prepare(
            "SELECT provider, model, reasoning_effort,
                    COUNT(*),
                    SUM(CASE WHEN failed=0 THEN 1 ELSE 0 END),
                    SUM(CASE WHEN failed<>0 THEN 1 ELSE 0 END),
                    SUM(input_tokens), SUM(output_tokens), SUM(reasoning_tokens),
                    SUM(cached_tokens), SUM(cache_read_tokens), SUM(cache_write_tokens),
                    SUM(total_tokens),
                    CASE WHEN COUNT(*)=COUNT(estimated_usd_micros)
                         THEN SUM(estimated_usd_micros) ELSE NULL END
             FROM usage_events WHERE reporting_day=?1
             GROUP BY provider, model, reasoning_effort",
        )
        .map_err(|error| format!("prepare model totals: {error}"))?;
    let rows = statement
        .query_map([&day], |row| {
            Ok((
                (row.get(0)?, row.get(1)?, row.get(2)?),
                totals_from_row(row, 3)?,
            ))
        })
        .map_err(|error| format!("query model totals: {error}"))?;
    for row in rows {
        let (key, totals) = row.map_err(|error| format!("read model totals: {error}"))?;
        models.insert(key, totals);
    }

    let quotas = load_quotas(connection)?;
    let mut accounts = BTreeMap::new();
    let mut statement = connection
        .prepare(
            "SELECT auth_index, provider,
                    COUNT(*),
                    SUM(CASE WHEN failed=0 THEN 1 ELSE 0 END),
                    SUM(CASE WHEN failed<>0 THEN 1 ELSE 0 END),
                    SUM(input_tokens), SUM(output_tokens), SUM(reasoning_tokens),
                    SUM(cached_tokens), SUM(cache_read_tokens), SUM(cache_write_tokens),
                    SUM(total_tokens),
                    CASE WHEN COUNT(*)=COUNT(estimated_usd_micros)
                         THEN SUM(estimated_usd_micros) ELSE NULL END,
                    MAX(requested_at)
             FROM usage_events WHERE reporting_day=?1 AND auth_index<>''
             GROUP BY auth_index, provider",
        )
        .map_err(|error| format!("prepare account totals: {error}"))?;
    let rows = statement
        .query_map([&day], |row| {
            let auth_index: String = row.get(0)?;
            let provider: String = row.get(1)?;
            let totals = totals_from_row(row, 2)?;
            let updated: Option<String> = row.get(13)?;
            Ok((auth_index, provider, totals, updated))
        })
        .map_err(|error| format!("query account totals: {error}"))?;
    for row in rows {
        let (auth_index, provider, totals, updated) =
            row.map_err(|error| format!("read account totals: {error}"))?;
        let updated_at = updated
            .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())
            .map(|value| value.with_timezone(&Utc));
        let quota = quotas.get(&auth_index).cloned().unwrap_or_default();
        accounts.insert(auth_index, (provider, totals, quota, updated_at));
    }

    let last_event_at = connection
        .query_row("SELECT MAX(requested_at) FROM usage_events", [], |row| {
            row.get::<_, Option<String>>(0)
        })
        .optional()
        .map_err(|error| format!("load last event timestamp: {error}"))?
        .flatten()
        .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())
        .map(|value| value.with_timezone(&Utc));

    Ok(RestoredState {
        sequence,
        reporting_day: day,
        all_time,
        today,
        models,
        accounts,
        last_event_at,
    })
}

fn query_totals(connection: &Connection, sql: &str, value: &str) -> Result<UsageTotals, String> {
    connection
        .query_row(sql, [value], |row| totals_from_row(row, 0))
        .map_err(|error| format!("query totals: {error}"))
}

fn totals_from_row(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<UsageTotals> {
    Ok(UsageTotals {
        requests: row.get::<_, Option<i64>>(offset)?.unwrap_or(0),
        successful_requests: row.get::<_, Option<i64>>(offset + 1)?.unwrap_or(0),
        failed_requests: row.get::<_, Option<i64>>(offset + 2)?.unwrap_or(0),
        tokens: TokenUsage {
            input_tokens: row.get::<_, Option<i64>>(offset + 3)?.unwrap_or(0),
            output_tokens: row.get::<_, Option<i64>>(offset + 4)?.unwrap_or(0),
            reasoning_tokens: row.get::<_, Option<i64>>(offset + 5)?.unwrap_or(0),
            cached_tokens: row.get::<_, Option<i64>>(offset + 6)?.unwrap_or(0),
            cache_read_tokens: row.get::<_, Option<i64>>(offset + 7)?.unwrap_or(0),
            cache_write_tokens: row.get::<_, Option<i64>>(offset + 8)?.unwrap_or(0),
            total_tokens: row.get::<_, Option<i64>>(offset + 9)?.unwrap_or(0),
        },
        estimated_usd_micros: row.get(offset + 10)?,
    })
}

fn load_quotas(connection: &Connection) -> Result<BTreeMap<String, QuotaSnapshot>, String> {
    let mut out = BTreeMap::new();
    let mut statement = connection
        .prepare("SELECT auth_index, quota_json FROM quota_snapshots")
        .map_err(|error| format!("prepare quota snapshots: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| format!("query quota snapshots: {error}"))?;
    for row in rows {
        let (auth_index, raw) = row.map_err(|error| format!("read quota snapshot: {error}"))?;
        if let Ok(quota) = serde_json::from_str::<QuotaSnapshot>(&raw) {
            out.insert(auth_index, quota);
        }
    }
    Ok(out)
}

fn cleanup(
    connection: &Connection,
    raw_retention_days: i64,
    daily_retention_days: i64,
) -> Result<(), String> {
    connection
        .execute(
            "DELETE FROM usage_events WHERE julianday(requested_at) < julianday('now', ?1)",
            [format!("-{raw_retention_days} days")],
        )
        .map_err(|error| format!("clean old usage events: {error}"))?;
    connection
        .execute(
            "DELETE FROM daily_model_usage
             WHERE julianday(reporting_day) < julianday(date('now'), ?1)",
            [format!("-{daily_retention_days} days")],
        )
        .map_err(|error| format!("clean old daily model rollups: {error}"))?;
    Ok(())
}

fn ensure_parent(path: &str) -> Result<(), String> {
    let parent = Path::new(path)
        .parent()
        .ok_or_else(|| format!("database path has no parent: {path}"))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("create database directory {}: {error}", parent.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::SanitizedUsage;
    use std::collections::HashMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn persists_and_restores_privacy_minimized_usage() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("cpa-whale-{suffix}.db"));
        let config = PluginConfig {
            database: path.display().to_string(),
            queue_capacity: 8,
            ..PluginConfig::default()
        };
        let (storage, restored) = StorageHandle::open(&config).unwrap();
        assert_eq!(restored.sequence, 0);
        let at = Utc::now();
        let usage = SanitizedUsage {
            requested_at: at,
            provider: "codex".into(),
            executor_type: "codex".into(),
            model: "gpt-5.6-sol".into(),
            alias: "sol".into(),
            reasoning_effort: "xhigh".into(),
            service_tier: "default".into(),
            auth_index: "auth-a".into(),
            auth_type: "oauth".into(),
            failed: false,
            status_code: 200,
            latency_ms: 10,
            ttft_ms: 5,
            tokens: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
                total_tokens: 15,
                ..TokenUsage::default()
            },
            response_headers: HashMap::new(),
            quota: None,
            estimated_usd_micros: Some(12),
            pricing_version: Some("test".into()),
        };
        assert!(storage.try_enqueue(PersistedUsage {
            sequence: 1,
            reporting_day: reporting_day(at, &config.timezone),
            usage,
        }));
        storage.shutdown();
        let connection = Connection::open(&config.database).unwrap();
        let restored = load_state(&connection, &config.timezone).unwrap();
        assert_eq!(restored.sequence, 1);
        assert_eq!(restored.all_time.tokens.total_tokens, 15);
        let schema_version = connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap();
        assert_eq!(schema_version, DATABASE_SCHEMA_VERSION);
        let daily = connection
            .query_row(
                "SELECT requests, total_tokens, estimated_usd_micros
                 FROM daily_model_usage WHERE model='gpt-5.6-sol'",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(daily, (1, 15, Some(12)));
        drop(connection);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn migrates_existing_events_into_daily_rollups_and_cleans_both_tiers() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("cpa-whale-migrate-{suffix}.db"));
        let mut connection = Connection::open(&path).unwrap();
        migrate(&mut connection).unwrap();
        connection
            .execute_batch(
                "INSERT INTO usage_events (
                   sequence, requested_at, reporting_day, provider, executor_type, model, alias,
                   reasoning_effort, service_tier, auth_index, auth_type, failed, status_code,
                   latency_ms, ttft_ms, input_tokens, output_tokens, reasoning_tokens,
                   cached_tokens, cache_read_tokens, cache_write_tokens, total_tokens,
                   pricing_version, estimated_usd_micros
                 ) VALUES (
                   1, '2000-01-01T00:00:00Z', '2000-01-01', 'codex', 'codex',
                   'model-a', 'model-a', '', 'default', '', '', 0, 200, 0, 0,
                   10, 5, 0, 0, 0, 0, 15, 'test', 12
                 );
                 DROP TABLE daily_model_usage;
                 PRAGMA user_version=1;",
            )
            .unwrap();
        migrate(&mut connection).unwrap();
        let total = connection
            .query_row(
                "SELECT total_tokens FROM daily_model_usage WHERE model='model-a'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(total, 15);
        cleanup(&connection, 7, 90).unwrap();
        let raw_count = connection
            .query_row("SELECT COUNT(*) FROM usage_events", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        let daily_count = connection
            .query_row("SELECT COUNT(*) FROM daily_model_usage", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        assert_eq!((raw_count, daily_count), (0, 0));
        drop(connection);
        let _ = fs::remove_file(path);
    }
}
