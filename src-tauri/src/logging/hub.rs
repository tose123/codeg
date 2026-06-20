//! Process-global logging runtime: the in-memory ring buffer the viewer reads,
//! the reload handle for runtime level changes, and the event emitter wired in
//! once `AppState` exists.
//!
//! The non-blocking file appender's `WorkerGuard` is **not** held here — it is
//! returned by [`crate::logging::init`] and kept alive in each binary's `main`
//! scope so it flushes on a graceful exit (statics are never dropped).

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use serde::Serialize;

use crate::logging::init::{build_env_filter, ReloadHandle};
use crate::logging::{LogSettings, LOG_APPENDED_EVENT};
use crate::web::event_bridge::{emit_event, EventEmitter};

/// Hard cap on retained records regardless of byte total — defense against a
/// flood of tiny events. Generous: the viewer shows recent history and the
/// durable record is the on-disk file.
pub const LOG_BUFFER_MAX_COUNT: usize = 5_000;

/// Byte ceiling for the ring buffer. RAM-bound only; sized so a burst of large
/// messages cannot grow the buffer without bound.
pub const LOG_BUFFER_MAX_BYTES: usize = 4 * 1024 * 1024;

/// One enclosing span in an event's scope: its name plus the key-value fields
/// recorded on it (at creation or via `Span::record`). Ordered root→leaf in
/// [`LogRecord::spans`].
#[derive(Debug, Clone, Serialize)]
pub struct SpanInfo {
    pub name: String,
    pub fields: BTreeMap<String, String>,
}

/// One captured log event — the unit the viewer renders and live-tails.
///
/// `level` is tracing's uppercase string (`"ERROR"`..`"TRACE"`); `target` is
/// the emitting module path. For migrated `eprintln!` the human `[TAG]` stays
/// inline in `message`, so the viewer text matches the old stderr output.
///
/// `fields` holds the event's own key-value fields (everything except the
/// `message`); `spans` holds the enclosing span chain (root→leaf). Both are
/// empty for the plain-message logs that make up the migrated call sites, so
/// those render exactly as before. `BTreeMap` keeps field order deterministic
/// for rendering and tests.
#[derive(Debug, Clone, Serialize)]
pub struct LogRecord {
    pub seq: u64,
    pub timestamp_ms: u64,
    pub level: &'static str,
    pub target: String,
    pub message: String,
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
    #[serde(default)]
    pub spans: Vec<SpanInfo>,
}

/// Severity rank for a tracing level string, for `min_level` filtering. Higher
/// = more severe; unknown strings rank 0. Consistent with [`LogLevel::rank`].
pub fn level_rank(level: &str) -> u8 {
    match level {
        "ERROR" => 5,
        "WARN" => 4,
        "INFO" => 3,
        "DEBUG" => 2,
        "TRACE" => 1,
        _ => 0,
    }
}

/// Cheap byte estimate for a record's footprint in the ring buffer — no serde
/// round-trip on the hot logging path. Counts the variable-length strings plus
/// a fixed per-entry/per-pair overhead, so the byte cap still bounds RAM as
/// records grow with structured fields and span context.
fn estimate_size(rec: &LogRecord) -> usize {
    let mut size = rec.target.len() + rec.message.len() + 48;
    for (k, v) in &rec.fields {
        size += k.len() + v.len() + 16;
    }
    for span in &rec.spans {
        size += span.name.len() + 16;
        for (k, v) in &span.fields {
            size += k.len() + v.len() + 16;
        }
    }
    size
}

/// Bounded ring buffer of recent records, enforcing a count cap and a byte cap
/// together via FIFO eviction. Mirrors `acp::event_stream::RecentEventsBuffer`.
struct RingBuffer {
    records: VecDeque<(usize, LogRecord)>,
    byte_total: usize,
}

impl RingBuffer {
    fn new() -> Self {
        Self {
            records: VecDeque::with_capacity(256),
            byte_total: 0,
        }
    }

    fn push(&mut self, rec: LogRecord) {
        let size = estimate_size(&rec);
        self.records.push_back((size, rec));
        self.byte_total = self.byte_total.saturating_add(size);
        while self.records.len() > LOG_BUFFER_MAX_COUNT || self.byte_total > LOG_BUFFER_MAX_BYTES {
            match self.records.pop_front() {
                Some((s, _)) => self.byte_total = self.byte_total.saturating_sub(s),
                None => break,
            }
        }
    }

    fn snapshot(&self) -> Vec<LogRecord> {
        self.records.iter().map(|(_, r)| r.clone()).collect()
    }
}

/// Process-global logging state. One instance per process, installed into
/// [`LOG_HUB`]; never dropped until exit.
pub struct LogHub {
    seq: AtomicU64,
    buffer: Mutex<RingBuffer>,
    emitter: RwLock<Option<EventEmitter>>,
    reload: ReloadHandle,
}

static LOG_HUB: OnceLock<Arc<LogHub>> = OnceLock::new();

/// The process-global [`LogHub`], or `None` before init (or in `codeg-mcp`,
/// which installs a stderr-only subscriber with no hub).
pub fn log_hub() -> Option<&'static Arc<LogHub>> {
    LOG_HUB.get()
}

impl LogHub {
    /// Build and install the global hub. Idempotent: a second call is ignored,
    /// so a stray re-init can't swap the live instance.
    pub(crate) fn install(reload: ReloadHandle) {
        let hub = Arc::new(Self {
            seq: AtomicU64::new(0),
            buffer: Mutex::new(RingBuffer::new()),
            emitter: RwLock::new(None),
            reload,
        });
        let _ = LOG_HUB.set(hub);
    }

    /// Monotonic per-record sequence number.
    pub fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }

    /// Append a record to the ring buffer and, if an emitter is wired, emit it
    /// on [`LOG_APPENDED_EVENT`].
    ///
    /// The buffer lock is taken and released within the first statement, before
    /// the emitter is cloned out and used — so the emit path (which may itself
    /// log) can never deadlock against the buffer lock, and re-entry is stopped
    /// by the layer's thread-local guard.
    pub fn record(&self, rec: LogRecord) {
        self.buffer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(rec.clone());
        let emitter = self
            .emitter
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(emitter) = emitter {
            emit_event(&emitter, LOG_APPENDED_EVENT, &rec);
        }
    }

    /// Newest-last snapshot of the buffered records.
    pub fn snapshot(&self) -> Vec<LogRecord> {
        self.buffer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .snapshot()
    }

    /// Wire the event emitter in after `AppState` / `AppHandle` exists (the
    /// subscriber is installed far earlier, at process start).
    pub fn set_emitter(&self, emitter: EventEmitter) {
        *self.emitter.write().unwrap_or_else(|e| e.into_inner()) = Some(emitter);
    }

    /// Apply the full logging settings (global level + per-target overrides)
    /// live, with zero restart (swaps the `EnvFilter` behind the reload layer).
    /// Takes the whole `LogSettings` rather than a bare level so the rebuilt
    /// filter can't silently drop the per-target directives.
    pub fn apply_settings(&self, settings: &LogSettings) {
        let _ = self.reload.modify(|filter| {
            *filter = build_env_filter(settings);
        });
    }

    /// Construct a hub NOT installed into the process-global slot, for tests.
    /// Its reload handle is detached (not wired to an installed subscriber).
    #[cfg(any(test, feature = "test-utils"))]
    pub fn new_detached() -> Arc<LogHub> {
        Arc::new(Self {
            seq: AtomicU64::new(0),
            buffer: Mutex::new(RingBuffer::new()),
            emitter: RwLock::new(None),
            reload: crate::logging::init::detached_reload_handle(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(seq: u64, level: &'static str, target: &str, message: &str) -> LogRecord {
        LogRecord {
            seq,
            timestamp_ms: 0,
            level,
            target: target.to_string(),
            message: message.to_string(),
            fields: BTreeMap::new(),
            spans: Vec::new(),
        }
    }

    #[test]
    fn ring_buffer_count_cap_evicts_oldest() {
        let mut buf = RingBuffer::new();
        let n = (LOG_BUFFER_MAX_COUNT + 10) as u64;
        for i in 0..n {
            buf.push(rec(i, "INFO", "t", "m"));
        }
        let snap = buf.snapshot();
        assert_eq!(snap.len(), LOG_BUFFER_MAX_COUNT);
        assert_eq!(snap.first().unwrap().seq, 10, "first 10 should be evicted");
        assert_eq!(snap.last().unwrap().seq, n - 1);
    }

    #[test]
    fn ring_buffer_byte_cap_evicts_to_stay_under_limit() {
        let mut buf = RingBuffer::new();
        let big = "x".repeat(64 * 1024);
        let n = (LOG_BUFFER_MAX_BYTES / (64 * 1024)) as u64 + 10;
        for i in 0..n {
            buf.push(rec(i, "INFO", "t", &big));
        }
        assert!(
            buf.byte_total <= LOG_BUFFER_MAX_BYTES,
            "byte_total {} exceeded cap {}",
            buf.byte_total,
            LOG_BUFFER_MAX_BYTES
        );
        assert!(buf.snapshot().len() <= LOG_BUFFER_MAX_COUNT);
    }

    #[test]
    fn estimate_size_counts_fields_and_spans() {
        let mut r = rec(1, "INFO", "t", "m");
        let base = estimate_size(&r);
        r.fields.insert("key".into(), "value".into());
        r.spans.push(SpanInfo {
            name: "span".into(),
            fields: BTreeMap::from([("a".to_string(), "b".to_string())]),
        });
        assert!(
            estimate_size(&r) > base,
            "fields/spans must increase the byte estimate so the cap still bounds RAM"
        );
    }

    #[test]
    fn level_rank_orders_severity() {
        assert!(level_rank("ERROR") > level_rank("WARN"));
        assert!(level_rank("WARN") > level_rank("INFO"));
        assert!(level_rank("INFO") > level_rank("DEBUG"));
        assert!(level_rank("DEBUG") > level_rank("TRACE"));
        assert_eq!(level_rank("???"), 0);
    }

    #[tokio::test]
    async fn record_buffers_and_emits_without_deadlock() {
        use crate::web::event_bridge::{EventEmitter, WebEventBroadcaster};

        let hub = LogHub::new_detached();
        let broadcaster = Arc::new(WebEventBroadcaster::new());
        let mut rx = broadcaster.subscribe();
        hub.set_emitter(EventEmitter::test_web_only(broadcaster.clone()));

        // Must return (no deadlock between the buffer lock and the emit path)
        // and both buffer and emit the record.
        hub.record(rec(1, "ERROR", "tgt", "boom"));

        let snap = hub.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].message, "boom");

        let evt = rx.try_recv().expect("append event delivered");
        assert_eq!(evt.channel, LOG_APPENDED_EVENT);
        let payload = &*evt.payload;
        assert_eq!(payload["message"], "boom");
        assert_eq!(payload["level"], "ERROR");
    }
}
