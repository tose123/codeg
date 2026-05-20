use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde::{ser::SerializeStruct, Serialize, Serializer};
use tokio::sync::{broadcast, RwLock};

use crate::acp::{AcpEvent, EventBusMetrics, EventEnvelope, InternalEventBus, SessionState};

/// Broadcast-delivered event.
///
/// `payload` is wrapped in `Arc` so cloning across broadcast receivers is
/// refcount-only — avoids copying multi-MB JSON trees per subscriber.
#[derive(Clone, Debug)]
pub struct WebEvent {
    pub channel: String,
    pub payload: Arc<serde_json::Value>,
}

impl Serialize for WebEvent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("WebEvent", 2)?;
        state.serialize_field("channel", &self.channel)?;
        state.serialize_field("payload", self.payload.as_ref())?;
        state.end()
    }
}

pub struct WebEventBroadcaster {
    sender: broadcast::Sender<WebEvent>,
}

impl Default for WebEventBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

impl WebEventBroadcaster {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(4096);
        Self { sender }
    }

    /// Serialize `payload` once and broadcast. Returns the serialized
    /// `Value` so Tauri callers can reuse it without serializing twice.
    pub fn send(&self, channel: &str, payload: &impl Serialize) -> Option<Arc<serde_json::Value>> {
        let value = Arc::new(serde_json::to_value(payload).ok()?);
        if self.sender.receiver_count() > 0 {
            let _ = self.sender.send(WebEvent {
                channel: channel.to_string(),
                payload: value.clone(),
            });
        }
        Some(value)
    }

    /// Broadcast a pre-serialized `Value` without re-serialization.
    pub fn send_value(&self, channel: &str, payload: Arc<serde_json::Value>) {
        if self.sender.receiver_count() == 0 {
            return;
        }
        let _ = self.sender.send(WebEvent {
            channel: channel.to_string(),
            payload,
        });
    }

    pub fn subscribe(&self) -> broadcast::Receiver<WebEvent> {
        self.sender.subscribe()
    }
}

/// Abstraction over event emission targets.
///
/// Three concerns layered together:
/// - **Tauri webview** (`Tauri` variant): events delivered to the desktop
///   webview via `app.emit`. Looked-up state (`Arc<WebEventBroadcaster>`,
///   `Arc<InternalEventBus>`) goes through `app.try_state`, registered in
///   `lib.rs::run` setup.
/// - **WS clients** (`WebOnly` variant): standalone server mode. Carries
///   the broadcaster directly because there's no AppHandle to look it up
///   from.
/// - **In-process consumers** (lifecycle / pet / chat-channel): receive
///   typed `Arc<EventEnvelope>` from `InternalEventBus`. Both `Tauri` and
///   `WebOnly` resolve to the same bus (via `acp_event_bus()`).
///
/// `Noop` drops everything — used for legacy non-streaming call paths and
/// in tests that don't observe events.
#[derive(Clone)]
pub enum EventEmitter {
    #[cfg(feature = "tauri-runtime")]
    Tauri(tauri::AppHandle),
    /// Standalone server runtime. Carries the broadcaster (transport-bound
    /// JSON delivery to WS clients on non-ACP channels) and the internal
    /// bus (typed envelope delivery to in-process subscribers).
    WebOnly {
        broadcaster: Arc<WebEventBroadcaster>,
        bus: Arc<InternalEventBus>,
    },
    /// Silent no-op emitter — drops all events. Used when streaming progress
    /// is not needed (e.g. legacy non-streaming call paths).
    Noop,
}

impl EventEmitter {
    /// Convenience constructor for the standalone server runtime path.
    /// Mirrors how `Tauri` resolves the same two pieces of state via
    /// `app.try_state`.
    pub fn web_only(broadcaster: Arc<WebEventBroadcaster>, bus: Arc<InternalEventBus>) -> Self {
        EventEmitter::WebOnly { broadcaster, bus }
    }

    /// Resolve the `InternalEventBus` for ACP-typed event delivery.
    ///
    /// In Tauri mode, looks up `Arc<InternalEventBus>` registered with
    /// `app.manage` during setup. Returns `None` if the bus isn't
    /// registered (only happens in degraded test setups) — the caller
    /// treats this as "no in-process consumers wired".
    pub fn acp_event_bus(&self) -> Option<Arc<InternalEventBus>> {
        match self {
            #[cfg(feature = "tauri-runtime")]
            EventEmitter::Tauri(app) => {
                use tauri::Manager;
                app.try_state::<Arc<InternalEventBus>>()
                    .map(|s| Arc::clone(&s))
            }
            EventEmitter::WebOnly { bus, .. } => Some(Arc::clone(bus)),
            EventEmitter::Noop => None,
        }
    }

    /// Resolve the `EventBusMetrics` handle. Same lookup rules as
    /// `acp_event_bus()`.
    pub fn metrics(&self) -> Option<Arc<EventBusMetrics>> {
        self.acp_event_bus().map(|bus| Arc::clone(bus.metrics()))
    }

    /// Test-only convenience: build a `WebOnly` emitter with a fresh,
    /// orphan `InternalEventBus`. Tests that assert against the
    /// broadcaster don't need to wire the bus through their own setup.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn test_web_only(broadcaster: Arc<WebEventBroadcaster>) -> Self {
        let metrics = Arc::new(EventBusMetrics::default());
        let bus = Arc::new(InternalEventBus::new(metrics));
        EventEmitter::WebOnly { broadcaster, bus }
    }
}

/// Unified event emission: serializes the payload exactly once and dispatches
/// the shared `Arc<Value>` to both the Tauri webview and the web broadcaster.
pub fn emit_event(emitter: &EventEmitter, event: &str, payload: impl Serialize) {
    match emitter {
        #[cfg(feature = "tauri-runtime")]
        EventEmitter::Tauri(app) => {
            use tauri::{Emitter, Manager};
            let Ok(value) = serde_json::to_value(&payload) else {
                return;
            };
            let shared = Arc::new(value);
            // `&Value` is Copy, so Tauri's `Clone` bound is satisfied without
            // copying the payload — Tauri serializes through the reference.
            let _ = app.emit(event, shared.as_ref());
            if let Some(web) = app.try_state::<Arc<WebEventBroadcaster>>() {
                web.send_value(event, shared);
            }
        }
        EventEmitter::WebOnly { broadcaster, .. } => {
            let _ = broadcaster.send(event, &payload);
        }
        EventEmitter::Noop => {}
    }
}

/// 统一 ACP 事件发射入口。
///
/// 流程：
/// 1. 写锁拿到 `SessionState`
/// 2. `apply_event` 把事件应用到 state（也更新 `last_activity_at`）
/// 3. `event_seq += 1`
/// 4. 用新 seq 构造 `EventEnvelope`，推入 ring buffer，记录淘汰计数
/// 5. 释放写锁
/// 6. 分发到三条路径：
///    - 每连接 `ConnectionEventStream`（WS attach 协议主路径）
///    - 进程内 `InternalEventBus`（lifecycle / pet / chat-channel 订阅者）
///    - Tauri 模式下额外 `app.emit("acp://event", ...)` 给 webview
///
/// 不再向 `WebEventBroadcaster` 上的 `acp://event` 频道广播——所有 ACP
/// 事件消费者要么走 per-connection stream（WS 客户端），要么走
/// InternalEventBus（进程内订阅者），要么走 Tauri `app.emit`（桌面 webview）。
/// 删除该全局广播是 Phase 5 架构清理的核心：它消除了 WS 客户端 receiver-side
/// 去重 (`attachManagedConnectionIdsRef`) 的必要性。
pub async fn emit_with_state(
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    payload: AcpEvent,
) {
    let (envelope_arc, stream, evicted) = {
        let mut s = state.write().await;
        s.apply_event(&payload);
        s.event_seq += 1;
        let envelope = Arc::new(EventEnvelope {
            seq: s.event_seq,
            connection_id: s.connection_id.clone(),
            payload,
        });
        let evicted = s.push_recent_event(Arc::clone(&envelope));
        (envelope, s.event_stream(), evicted)
    };

    // Per-connection broadcaster — primary delivery path for web/remote-
    // desktop transports (they use Subscribe-with-Snapshot attach for ACP
    // events).
    stream.send(Arc::clone(&envelope_arc));

    // In-process consumers (lifecycle, pet, chat-channel). Typed envelope —
    // no JSON parse on the receiver side. Plus surface ring-buffer pressure
    // and bus emit-rate via metrics so operators can see when things drift.
    match emitter {
        #[cfg(feature = "tauri-runtime")]
        EventEmitter::Tauri(app) => {
            use tauri::{Emitter, Manager};
            // Tauri webview listener is the desktop frontend's only ACP path
            // (it subscribes via `app.listen`, not the WS attach protocol).
            let _ = app.emit("acp://event", envelope_arc.as_ref());
            if let Some(bus) = app.try_state::<Arc<InternalEventBus>>() {
                bus.send(Arc::clone(&envelope_arc));
                if evicted > 0 {
                    bus.metrics()
                        .ring_buffer_evict_count
                        .fetch_add(evicted as u64, Ordering::Relaxed);
                }
            }
        }
        EventEmitter::WebOnly { bus, .. } => {
            bus.send(Arc::clone(&envelope_arc));
            if evicted > 0 {
                bus.metrics()
                    .ring_buffer_evict_count
                    .fetch_add(evicted as u64, Ordering::Relaxed);
            }
        }
        EventEmitter::Noop => {}
    }
}
