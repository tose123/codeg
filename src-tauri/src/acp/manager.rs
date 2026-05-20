use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ActiveValue::Set, DatabaseConnection, EntityTrait,
    TransactionTrait,
};

use crate::acp::connection::{spawn_agent_connection, AgentConnection, ConnectionCommand};
use crate::acp::error::AcpError;
use crate::acp::types::{
    AcpEvent, ConnectionInfo, ConnectionStatus, ForkResultInfo, PromptInputBlock,
};
use crate::db::entities::conversation::{self, ConversationStatus};
use crate::db::service::conversation_service;
use crate::db::AppDatabase;
use crate::models::agent::AgentType;
use crate::web::event_bridge::{emit_with_state, EventEmitter};

/// Composite key identifying a logical agent session for spawn-time dedup.
/// Two `acp_connect` calls with the same triple race for the same `Mutex`,
/// so the second one observes the first's freshly-spawned connection in
/// `find_connection_for_reuse` instead of starting a duplicate process.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct SpawnDedupKey {
    agent_type: AgentType,
    working_dir: Option<PathBuf>,
    session_id: String,
}

/// Default upper bound on how long `spawn_agent` will hold the per-session
/// dedup lock waiting for `SessionStarted`. Picked to comfortably cover
/// cold-start agents (claude-code/codex warm: <2s; npx-fetched cold: 10–30s)
/// without deadlocking the next concurrent acp_connect when an agent is
/// genuinely broken.
pub(crate) const SPAWN_HANDSHAKE_TIMEOUT_SECS: u64 = 60;

/// Read the spawn-handshake timeout from `CODEG_ACP_SPAWN_HANDSHAKE_TIMEOUT_SECS`,
/// falling back to `SPAWN_HANDSHAKE_TIMEOUT_SECS`. Returns the configured
/// `Duration`. Tests can construct the manager with a custom value via
/// `with_spawn_handshake_timeout` instead of mutating env.
fn spawn_handshake_timeout_from_env() -> Duration {
    let secs = std::env::var("CODEG_ACP_SPAWN_HANDSHAKE_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(SPAWN_HANDSHAKE_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

/// Outcome of the `spawn_agent` dedup wait. Logged so production can audit
/// how often the timeout fires vs. the agent handshake completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HandshakeWaitOutcome {
    /// `SessionStarted` applied; `external_id` is now set on the state.
    Ready,
    /// Sender was dropped before SessionStarted fired (typically the
    /// connection died during init — `run_connection` returned Err).
    Aborted,
    /// Timeout elapsed before either of the above. Releases the dedup lock
    /// so the next caller can proceed; the slow agent is no worse off.
    TimedOut,
}

impl HandshakeWaitOutcome {
    fn as_str(self) -> &'static str {
        match self {
            HandshakeWaitOutcome::Ready => "ready",
            HandshakeWaitOutcome::Aborted => "aborted",
            HandshakeWaitOutcome::TimedOut => "timeout",
        }
    }
}

/// Wait for the spawn-time `SessionStarted` signal, bounded by `timeout`.
/// Extracted so the outcome enum can be unit-tested without spawning a
/// real agent process.
async fn wait_for_session_started(
    rx: tokio::sync::oneshot::Receiver<()>,
    timeout: Duration,
) -> (HandshakeWaitOutcome, Duration) {
    let start = std::time::Instant::now();
    let outcome = match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(())) => HandshakeWaitOutcome::Ready,
        Ok(Err(_)) => HandshakeWaitOutcome::Aborted,
        Err(_) => HandshakeWaitOutcome::TimedOut,
    };
    (outcome, start.elapsed())
}

pub struct ConnectionManager {
    pub(crate) connections: Arc<Mutex<HashMap<String, AgentConnection>>>,
    /// Per-(agent, working_dir, session_id) async mutex. Held across the
    /// dedup-lookup + spawn + SessionStarted-wait critical section so two
    /// concurrent `spawn_agent` calls for the same logical session can't
    /// both miss dedup during the handshake window. Entries persist for
    /// process lifetime — bounded by the number of distinct sessions ever
    /// connected.
    spawn_locks: Arc<Mutex<HashMap<SpawnDedupKey, Arc<Mutex<()>>>>>,
    /// Bound on how long `spawn_agent` waits for the agent's handshake
    /// before releasing the dedup lock. Configurable per-instance for
    /// tests; in production initialized from env via
    /// `spawn_handshake_timeout_from_env`.
    spawn_handshake_timeout: Duration,
}

impl Default for ConnectionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ConnectionManager {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(Mutex::new(HashMap::new())),
            spawn_locks: Arc::new(Mutex::new(HashMap::new())),
            spawn_handshake_timeout: spawn_handshake_timeout_from_env(),
        }
    }

    /// Returns a shallow clone sharing the same underlying connection map.
    pub fn clone_ref(&self) -> Self {
        Self {
            connections: self.connections.clone(),
            spawn_locks: self.spawn_locks.clone(),
            spawn_handshake_timeout: self.spawn_handshake_timeout,
        }
    }

    /// Test-only constructor that overrides the spawn-handshake timeout.
    /// Production code should use `new()`.
    #[cfg(test)]
    fn with_spawn_handshake_timeout(timeout: Duration) -> Self {
        Self {
            connections: Arc::new(Mutex::new(HashMap::new())),
            spawn_locks: Arc::new(Mutex::new(HashMap::new())),
            spawn_handshake_timeout: timeout,
        }
    }

    /// Insert a synthetic `AgentConnection` for tests that need to exercise
    /// downstream code (attach, event broadcast, conversation linking)
    /// without spawning a real agent process. The returned connection is
    /// marked `Connected` and has a dropped `cmd_tx` receiver, so any
    /// attempt to send a prompt resolves to `ProcessExited` — fine for
    /// tests asserting on event-bus or session-state behavior.
    ///
    /// Gated behind `cfg(test)` (in-crate unit tests) and the `test-utils`
    /// feature (integration tests in `tests/*.rs`); the item is physically
    /// uncompiled in release builds so no production caller can reach it.
    #[cfg(any(test, feature = "test-utils"))]
    pub async fn insert_test_connection(
        &self,
        id: &str,
        agent_type: AgentType,
        working_dir: Option<PathBuf>,
        emitter: EventEmitter,
    ) {
        use crate::acp::session_state::SessionState;
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let mut state = SessionState::new(
            id.to_string(),
            agent_type,
            working_dir,
            "test-window".to_string(),
            None,
        );
        state.status = ConnectionStatus::Connected;
        let conn = AgentConnection {
            id: id.to_string(),
            agent_type,
            status: ConnectionStatus::Connected,
            owner_window_label: "test-window".to_string(),
            cmd_tx: tx,
            state: Arc::new(tokio::sync::RwLock::new(state)),
            emitter,
            prompt_lock: Arc::new(tokio::sync::Mutex::new(())),
        };
        let mut map = self.connections.lock().await;
        map.insert(id.to_string(), conn);
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn spawn_agent(
        &self,
        agent_type: AgentType,
        working_dir: Option<String>,
        session_id: Option<String>,
        runtime_env: BTreeMap<String, String>,
        owner_window_label: String,
        emitter: EventEmitter,
        preferred_mode_id: Option<String>,
        preferred_config_values: BTreeMap<String, String>,
    ) -> Result<String, AcpError> {
        // Connection dedup: when resuming an agent session (session_id is
        // Some), look for a live AgentConnection that already represents
        // the same external session in the same working_dir for the same
        // agent_type and is not torn down. If found, reuse it instead of
        // spawning a fresh process — this is what makes a browser refresh
        // mid-turn re-attach to the existing live state rather than orphan it.
        let working_dir_path = working_dir.as_ref().map(PathBuf::from);

        // Acquire a per-(agent, working_dir, session_id) async mutex so two
        // concurrent connects for the same logical session can't both miss
        // dedup during the handshake window. The lookup → spawn → wait-for-
        // SessionStarted critical section runs under this lock; the second
        // waiter, on entry, observes the first call's connection with
        // `state.external_id` already populated and returns its id via
        // `find_connection_for_reuse`. Skipped entirely when `session_id`
        // is None (fresh sessions can't dedup — by design — since the
        // agent assigns the id).
        let session_id_for_log = session_id.clone();
        let dedup_lock = if let Some(sid) = session_id.as_deref() {
            let key = SpawnDedupKey {
                agent_type,
                working_dir: working_dir_path.clone(),
                session_id: sid.to_string(),
            };
            let mu = {
                let mut locks = self.spawn_locks.lock().await;
                locks
                    .entry(key)
                    .or_insert_with(|| Arc::new(Mutex::new(())))
                    .clone()
            };
            Some(mu.lock_owned().await)
        } else {
            None
        };

        if let Some(existing) = self
            .find_connection_for_reuse(agent_type, working_dir_path.as_ref(), session_id.as_deref())
            .await
        {
            eprintln!(
                "[ACP] reusing connection id={} for session_id={}",
                existing,
                session_id.as_deref().unwrap_or("")
            );
            return Ok(existing);
        }

        let connection_id = uuid::Uuid::new_v4().to_string();
        eprintln!(
            "[ACP] spawning connection id={} owner_window={} agent={:?}",
            connection_id, owner_window_label, agent_type
        );

        // `spawn_agent_connection` inserts the entry into `self.connections`,
        // installs the SessionStarted dedup signal on the state, registers
        // a cleanup hook, and returns the rx half of the signal. Any spawn
        // failure short-circuits before we touch the rx wait.
        let session_started_rx = spawn_agent_connection(
            connection_id.clone(),
            agent_type,
            working_dir,
            session_id,
            runtime_env,
            owner_window_label,
            emitter,
            self.connections.clone(),
            preferred_mode_id,
            preferred_config_values,
        )
        .await?;

        // When dedup is active, hold the lock until the agent's
        // SessionStarted has applied (so external_id is populated for the
        // next waiter), aborted (connection died), or the timeout fires.
        // Logged on every wait so production can audit real-world handshake
        // latencies and tune `CODEG_ACP_SPAWN_HANDSHAKE_TIMEOUT_SECS`.
        if dedup_lock.is_some() {
            let timeout = self.spawn_handshake_timeout;
            let (outcome, elapsed) = wait_for_session_started(session_started_rx, timeout).await;
            eprintln!(
                "[ACP] dedup_wait connection_id={} session_id={} outcome={} \
                 elapsed_ms={} timeout_ms={}",
                connection_id,
                session_id_for_log.as_deref().unwrap_or(""),
                outcome.as_str(),
                elapsed.as_millis(),
                timeout.as_millis(),
            );
        }
        // session_started_rx (in the no-dedup branch) is dropped here. tx
        // staying inside SessionState gets dropped naturally when the
        // connection terminates, no leak.

        drop(dedup_lock);

        Ok(connection_id)
    }

    /// Bump `last_activity_at` for a live connection so the idle sweep
    /// won't reap it. Used by the frontend keepalive loop to protect
    /// connections backing currently-open conversation tabs (the
    /// frontend is the only side that knows which tabs the user has
    /// open). Silently no-ops if the connection is missing or already
    /// in a terminal state — touch must never resurrect a dead
    /// connection or contend with the spawn/disconnect paths.
    pub async fn touch(&self, conn_id: &str) -> bool {
        let state_arc = {
            let connections = self.connections.lock().await;
            match connections.get(conn_id) {
                Some(conn) => conn.state.clone(),
                None => return false,
            }
        };
        let mut state = state_arc.write().await;
        if matches!(
            state.status,
            ConnectionStatus::Disconnected | ConnectionStatus::Error
        ) {
            return false;
        }
        state.last_activity_at = chrono::Utc::now();
        true
    }

    /// Disconnect connections that have been idle longer than `idle_timeout`.
    /// "Idle" means: status is `Connected`, no `pending_permission`, no
    /// activity (no events, no commands) for at least `idle_timeout`.
    /// `Prompting` connections are always preserved (a turn is in flight).
    /// Returns the number of connections that were disconnected.
    pub async fn sweep_idle(&self, idle_timeout: Duration) -> usize {
        let now = chrono::Utc::now();
        let timeout = match chrono::Duration::from_std(idle_timeout) {
            Ok(d) => d,
            Err(_) => return 0,
        };
        let to_disconnect: Vec<String> = {
            let connections = self.connections.lock().await;
            let mut victims = Vec::new();
            for (id, conn) in connections.iter() {
                let Ok(state) = conn.state.try_read() else {
                    // Per-state writer holds the lock; a future tick will
                    // re-evaluate this entry. Don't block the connections
                    // mutex on it.
                    continue;
                };
                if state.status != ConnectionStatus::Connected {
                    continue;
                }
                if state.pending_permission.is_some() {
                    continue;
                }
                let elapsed = now.signed_duration_since(state.last_activity_at);
                if elapsed >= timeout {
                    victims.push(id.clone());
                }
            }
            victims
        };
        let mut disconnected = 0;
        for id in to_disconnect {
            eprintln!("[ACP] idle sweep disconnecting connection={}", id);
            if self.disconnect(&id).await.is_ok() {
                disconnected += 1;
            }
        }
        disconnected
    }

    /// Look up an existing live connection that we can reuse instead of
    /// spawning a new process. Reuse criteria, ALL must hold:
    /// - `session_id` is Some (we never dedup speculative / fresh connects)
    /// - the connection's `state.external_id` equals `session_id`
    /// - the connection's `agent_type` equals the requested one
    /// - the connection's `working_dir` equals the requested one (compared as
    ///   `Option<PathBuf>` so canonicalization is the caller's concern)
    /// - the connection's `state.status` is neither `Disconnected` nor `Error`
    ///
    /// Per-session state is acquired via `read().await` rather than `try_read`:
    /// the only writer is `emit_with_state`, whose critical section is
    /// microseconds (apply_event + seq++ + broadcast::send), so contention
    /// resolves quickly and the previous "skip on writer" behavior was just
    /// trading correctness (false-negative dedup → duplicate process spawn)
    /// for an imperceptible latency win. The connections-map mutex is held
    /// across the awaits — fine because no path takes `state.write()` while
    /// holding the connections mutex (no lock-cycle).
    pub(crate) async fn find_connection_for_reuse(
        &self,
        agent_type: AgentType,
        working_dir: Option<&PathBuf>,
        session_id: Option<&str>,
    ) -> Option<String> {
        // No session_id → caller is opening a fresh session; never dedup.
        let session_id = session_id?;
        let connections = self.connections.lock().await;
        for (id, conn) in connections.iter() {
            if conn.agent_type != agent_type {
                continue;
            }
            let state = conn.state.read().await;
            if state.external_id.as_deref() != Some(session_id) {
                continue;
            }
            if state.working_dir.as_ref() != working_dir {
                continue;
            }
            if matches!(
                state.status,
                ConnectionStatus::Disconnected | ConnectionStatus::Error
            ) {
                continue;
            }
            return Some(id.clone());
        }
        None
    }

    /// Forwards a prompt to the connection's command channel without
    /// touching `prompt_lock`. Internal helper — both `send_prompt` and
    /// `send_prompt_linked` acquire the lock externally and then call
    /// this. Re-entering through `send_prompt` from `send_prompt_linked`
    /// while holding the lock would deadlock, hence the split.
    async fn send_prompt_inner(
        &self,
        conn_id: &str,
        blocks: Vec<PromptInputBlock>,
    ) -> Result<(), AcpError> {
        let cmd_tx = {
            let connections = self.connections.lock().await;
            let conn = connections
                .get(conn_id)
                .ok_or_else(|| AcpError::ConnectionNotFound(conn_id.into()))?;
            conn.cmd_tx.clone()
        };
        cmd_tx
            .send(ConnectionCommand::Prompt { blocks })
            .await
            .map_err(|_| AcpError::ProcessExited)
    }

    /// Clone the connection's `prompt_lock` under a short connections-map lock.
    /// Returned Arc allows the caller to hold the prompt lock without
    /// keeping the connections map locked.
    async fn clone_prompt_lock(
        &self,
        conn_id: &str,
    ) -> Result<Arc<tokio::sync::Mutex<()>>, AcpError> {
        let connections = self.connections.lock().await;
        let conn = connections
            .get(conn_id)
            .ok_or_else(|| AcpError::ConnectionNotFound(conn_id.into()))?;
        Ok(conn.prompt_lock.clone())
    }

    pub async fn send_prompt(
        &self,
        conn_id: &str,
        blocks: Vec<PromptInputBlock>,
    ) -> Result<(), AcpError> {
        let prompt_lock = self.clone_prompt_lock(conn_id).await?;
        let _guard = prompt_lock.lock_owned().await;
        self.send_prompt_inner(conn_id, blocks).await
    }

    /// Send a prompt while ensuring a `Conversation` DB row is bound to this
    /// connection. On the first call (when `state.conversation_id` is None),
    /// either:
    /// - **Caller-supplied path** — if `conversation_id` is `Some(id)`, the
    ///   caller (the frontend) has already created the row and we adopt it via
    ///   `ConversationLinked`. Requires `folder_id` to be `Some` so the event
    ///   carries both ids without forcing subscribers to re-query the DB.
    /// - **Backend-creates path** — if `conversation_id` is `None`, we create
    ///   the row from `folder_id` (required) and emit `ConversationLinked`.
    ///   Returns an error if `folder_id` is also `None`.
    ///
    /// Subsequent calls (when state is already linked) ignore both
    /// `folder_id` and `conversation_id` and just forward the prompt.
    pub async fn send_prompt_linked(
        &self,
        db: &AppDatabase,
        conn_id: &str,
        blocks: Vec<PromptInputBlock>,
        folder_id: Option<i32>,
        conversation_id: Option<i32>,
    ) -> Result<(), AcpError> {
        // Caller-supplied conversation_id requires folder_id (we include it in
        // the emitted ConversationLinked event so subscribers don't have to
        // re-query the DB). Validate before touching any state.
        if conversation_id.is_some() && folder_id.is_none() {
            return Err(AcpError::protocol(
                "conversation_id provided without folder_id".to_string(),
            ));
        }

        // Acquire the per-connection prompt lock for the entire link-check
        // + DB write + emit + cmd_tx.send sequence. Two concurrent prompts
        // (multiple browser tabs of the same conversation; chat-channel
        // racing the UI) are now strictly serialized — the second waiter
        // observes `already_linked == true` after the first commits, so
        // it can't double-create a conversation row.
        let prompt_lock = self.clone_prompt_lock(conn_id).await?;
        let _prompt_guard = prompt_lock.lock_owned().await;

        // Snapshot what we need from the connection map under one short lock.
        // The conversation-linked check happens INSIDE the prompt lock so
        // any racing send sees a consistent post-link state.
        let (state_arc, emitter, agent_type, already_linked) = {
            let connections = self.connections.lock().await;
            let conn = connections
                .get(conn_id)
                .ok_or_else(|| AcpError::ConnectionNotFound(conn_id.into()))?;
            let already = {
                let s = conn.state.read().await;
                s.conversation_id.is_some()
            };
            (
                conn.state.clone(),
                conn.emitter.clone(),
                conn.agent_type,
                already,
            )
        };

        if !already_linked {
            match (conversation_id, folder_id) {
                // Branch A: caller already owns a row — adopt it. No DB write.
                (Some(caller_conv_id), Some(caller_folder_id)) => {
                    emit_with_state(
                        &state_arc,
                        &emitter,
                        AcpEvent::ConversationLinked {
                            conversation_id: caller_conv_id,
                            folder_id: caller_folder_id,
                        },
                    )
                    .await;
                }
                // Function-entry guard rejects this combination.
                (Some(_), None) => unreachable!(
                    "conversation_id without folder_id should have been rejected at function entry"
                ),
                // Branch B: backend creates the row from caller-supplied
                // folder_id. Phase 3c-1 made folder_id required here — every
                // production caller that reaches this branch passes one, and
                // silent fallback to working_dir-based find-or-create masked
                // contract violations.
                (None, Some(folder_id)) => {
                    let row =
                        conversation_service::create(&db.conn, folder_id, agent_type, None, None)
                            .await
                            .map_err(|e| AcpError::protocol(e.to_string()))?;
                    emit_with_state(
                        &state_arc,
                        &emitter,
                        AcpEvent::ConversationLinked {
                            conversation_id: row.id,
                            folder_id,
                        },
                    )
                    .await;
                }
                (None, None) => {
                    return Err(AcpError::protocol(
                        "folder_id required for new conversation row".to_string(),
                    ));
                }
            }

            // UI new-conversation path: SessionStarted applied state.external_id
            // back during acp_connect, but conversation_id was None then so the
            // lifecycle subscriber's SessionStarted handler skipped the DB write.
            // Now that we just linked the row in the same prompt_lock critical
            // section, snapshot external_id and persist it synchronously — no
            // dependence on broadcaster eventual consistency. The chat_channel
            // reverse-order path (link before SessionStarted) is unaffected and
            // continues to be handled by the lifecycle subscriber.
            let (cid_opt, eid_opt) = {
                let s = state_arc.read().await;
                (s.conversation_id, s.external_id.clone())
            };
            if let (Some(cid), Some(eid)) = (cid_opt, eid_opt) {
                conversation_service::update_external_id(&db.conn, cid, eid)
                    .await
                    .map_err(|e| AcpError::protocol(e.to_string()))?;
            } else if cid_opt.is_some() {
                eprintln!(
                    "[manager] send_prompt_linked: conversation linked but \
                     external_id not yet on state (conn={conn_id}); lifecycle \
                     subscriber will catch up when SessionStarted arrives"
                );
            }
        }

        // Centralized status transition: every prompt send flips the
        // conversation row to InProgress. This MUST happen on every call
        // (including the already-linked path) so that a follow-up turn whose
        // row is currently `pending_review` correctly transitions back. The
        // DB write precedes the event emit so any subscriber observing
        // `ConversationStatusChanged` can assume the row is consistent.
        // `update_status` is a single UPDATE — idempotent with respect to
        // the same status value, so re-writing `InProgress` is a benign no-op
        // on the row (touches `updated_at` only).
        let conversation_id_for_status = state_arc.read().await.conversation_id;
        if let Some(cid) = conversation_id_for_status {
            conversation_service::update_status(&db.conn, cid, ConversationStatus::InProgress)
                .await
                .map_err(|e| AcpError::protocol(e.to_string()))?;
            emit_with_state(
                &state_arc,
                &emitter,
                AcpEvent::ConversationStatusChanged {
                    conversation_id: cid,
                    status: ConversationStatus::InProgress,
                },
            )
            .await;
        }

        // We hold `_prompt_guard` here, so call the lock-free inner helper —
        // re-entering `send_prompt` would try to acquire the same mutex and
        // deadlock. On failure (channel closed, process exited), flip the
        // row to `Cancelled` so the UI doesn't strand on `in_progress`. No
        // `TurnComplete` will ever arrive for a prompt that never reached
        // the agent, so without this rollback the lifecycle subscriber's
        // PendingReview write also never fires — the row would be stuck
        // until a follow-up `send_prompt_linked` happened to re-flip it.
        match self.send_prompt_inner(conn_id, blocks).await {
            Ok(()) => Ok(()),
            Err(send_err) => {
                if let Some(cid) = conversation_id_for_status {
                    match conversation_service::update_status(
                        &db.conn,
                        cid,
                        ConversationStatus::Cancelled,
                    )
                    .await
                    {
                        Ok(_) => {
                            emit_with_state(
                                &state_arc,
                                &emitter,
                                AcpEvent::ConversationStatusChanged {
                                    conversation_id: cid,
                                    status: ConversationStatus::Cancelled,
                                },
                            )
                            .await;
                        }
                        Err(rollback_err) => {
                            // Best-effort: original send error is the load-bearing
                            // signal; rollback failure is logged but not surfaced.
                            eprintln!(
                                "[ACP][ERROR] failed to mark conversation {cid} cancelled \
                                 after send failure (original={send_err}): {rollback_err}"
                            );
                        }
                    }
                }
                Err(send_err)
            }
        }
    }

    pub async fn set_mode(&self, conn_id: &str, mode_id: String) -> Result<(), AcpError> {
        let cmd_tx = {
            let connections = self.connections.lock().await;
            let conn = connections
                .get(conn_id)
                .ok_or_else(|| AcpError::ConnectionNotFound(conn_id.into()))?;
            conn.cmd_tx.clone()
        };
        cmd_tx
            .send(ConnectionCommand::SetMode { mode_id })
            .await
            .map_err(|_| AcpError::ProcessExited)
    }

    pub async fn set_config_option(
        &self,
        conn_id: &str,
        config_id: String,
        value_id: String,
    ) -> Result<(), AcpError> {
        let cmd_tx = {
            let connections = self.connections.lock().await;
            let conn = connections
                .get(conn_id)
                .ok_or_else(|| AcpError::ConnectionNotFound(conn_id.into()))?;
            conn.cmd_tx.clone()
        };
        cmd_tx
            .send(ConnectionCommand::SetConfigOption {
                config_id,
                value_id,
            })
            .await
            .map_err(|_| AcpError::ProcessExited)
    }

    pub async fn cancel(&self, db: &DatabaseConnection, conn_id: &str) -> Result<(), AcpError> {
        let (cmd_tx, state_arc, emitter) = {
            let connections = self.connections.lock().await;
            let conn = connections
                .get(conn_id)
                .ok_or_else(|| AcpError::ConnectionNotFound(conn_id.into()))?;
            (
                conn.cmd_tx.clone(),
                conn.state.clone(),
                conn.emitter.clone(),
            )
        };
        cmd_tx
            .send(ConnectionCommand::Cancel)
            .await
            .map_err(|_| AcpError::ProcessExited)?;

        // Eagerly flip the row to `Cancelled` so the sidebar/tabs leave the
        // "running" state immediately. The agent typically replies with
        // `TurnComplete{cancelled}` which the lifecycle subscriber ignores,
        // and stays connected (so `handle_terminal_event` doesn't fire either)
        // — without this write the row would strand on `InProgress`.
        // CAS-guarded so we don't overwrite a `PendingReview`/`Completed`
        // status if the turn happened to end just before the user clicked.
        let conversation_id = state_arc.read().await.conversation_id;
        if let Some(cid) = conversation_id {
            match conversation_service::update_status_if(
                db,
                cid,
                ConversationStatus::InProgress,
                ConversationStatus::Cancelled,
            )
            .await
            {
                Ok(true) => {
                    emit_with_state(
                        &state_arc,
                        &emitter,
                        AcpEvent::ConversationStatusChanged {
                            conversation_id: cid,
                            status: ConversationStatus::Cancelled,
                        },
                    )
                    .await;
                }
                Ok(false) => {}
                Err(e) => {
                    eprintln!(
                        "[ACP][ERROR] failed to mark conversation {cid} cancelled \
                         on user cancel (conn={conn_id}): {e}"
                    );
                }
            }
        }

        Ok(())
    }

    pub async fn respond_permission(
        &self,
        conn_id: &str,
        request_id: &str,
        option_id: &str,
    ) -> Result<(), AcpError> {
        let cmd_tx = {
            let connections = self.connections.lock().await;
            let conn = connections
                .get(conn_id)
                .ok_or_else(|| AcpError::ConnectionNotFound(conn_id.into()))?;
            conn.cmd_tx.clone()
        };
        cmd_tx
            .send(ConnectionCommand::RespondPermission {
                request_id: request_id.into(),
                option_id: option_id.into(),
            })
            .await
            .map_err(|_| AcpError::ProcessExited)
    }

    /// Fork the agent's session and persist the resulting two-row layout in
    /// one backend call: the current row gets re-pointed at S2 (the forked
    /// session) with a `[Fork]` title prefix, and a freshly-created sibling
    /// row preserves the pre-fork (S1) history at `PendingReview`. Frontend
    /// no longer touches `external_id` or fork-related row creation —
    /// the wire `ForkResultInfo` carries `sibling_conversation_id` for tab/UI
    /// reconciliation.
    pub async fn fork_session(
        &self,
        db: &AppDatabase,
        conn_id: &str,
    ) -> Result<ForkResultInfo, AcpError> {
        let (state_arc, cmd_tx) = {
            let connections = self.connections.lock().await;
            let conn = connections
                .get(conn_id)
                .ok_or_else(|| AcpError::ConnectionNotFound(conn_id.into()))?;
            (conn.state.clone(), conn.cmd_tx.clone())
        };

        // Fork requires a linked conversation row — the sibling we're about
        // to create exists to preserve THIS row's pre-fork history. Without
        // a current row, fork would either orphan S1 or violate the
        // no-pre-prompt-row invariant.
        let conversation_id = state_arc.read().await.conversation_id.ok_or_else(|| {
            AcpError::protocol("fork_session requires a linked conversation row".to_string())
        })?;

        // Protocol-only round trip — no DB writes inside the connection loop.
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        cmd_tx
            .send(ConnectionCommand::Fork { reply: reply_tx })
            .await
            .map_err(|_| AcpError::ProcessExited)?;
        let protocol_result = reply_rx
            .await
            .map_err(|_| AcpError::protocol("Fork reply channel closed".to_string()))??;

        let forked_session_id = protocol_result.forked_session_id;
        let original_session_id = protocol_result.original_session_id;

        // Persist the fork outcome in one transaction:
        //   UPDATE current  (title + external_id → S2)
        //   INSERT sibling  (full row pre-set: external_id=S1, status=PendingReview)
        // Atomic so a mid-sequence failure can't leak: if INSERT fails we don't
        // re-point the current row at S2 (it stays bound to S1; the lifecycle
        // subscriber's eventual SessionStarted{S2} write would still occur, but
        // the user-visible row layout stays consistent until then). If UPDATE
        // fails we never insert a sibling — no orphan S1 row.
        let forked_for_tx = forked_session_id.clone();
        let original_for_tx = original_session_id.clone();
        let sibling_id = db
            .conn
            .transaction::<_, i32, sea_orm::DbErr>(|txn| {
                Box::pin(async move {
                    let current = conversation::Entity::find_by_id(conversation_id)
                        .one(txn)
                        .await?
                        .ok_or_else(|| {
                            sea_orm::DbErr::Custom(format!(
                                "conversation {conversation_id} not found"
                            ))
                        })?;

                    // Strip any `[Fork]` prefix tolerantly (matches the prior
                    // frontend regex `/^\[Fork]\s*/g` behaviour for both spaced
                    // and no-space variants). None title stays None on both rows.
                    let clean_title: Option<String> = current.title.as_ref().map(|t| {
                        t.strip_prefix("[Fork]")
                            .map(str::trim_start)
                            .unwrap_or(t.as_str())
                            .to_string()
                    });

                    let folder_id = current.folder_id;
                    let agent_type_str = current.agent_type.clone();
                    let git_branch = current.git_branch.clone();
                    let now = chrono::Utc::now();

                    // UPDATE current row → S2. Writing external_id explicitly
                    // here closes the race against `refreshConversations()`
                    // after this fn returns; the lifecycle subscriber's later
                    // SessionStarted{S2} write is an idempotent no-op.
                    let mut active: conversation::ActiveModel = current.into();
                    if let Some(ref clean) = clean_title {
                        active.title = Set(Some(format!("[Fork] {clean}")));
                    }
                    active.external_id = Set(Some(forked_for_tx));
                    active.updated_at = Set(now);
                    active.update(txn).await?;

                    // INSERT sibling row preserving pre-fork (S1) history.
                    // PendingReview because no live agent is attached to S1.
                    let sibling = conversation::ActiveModel {
                        id: NotSet,
                        folder_id: Set(folder_id),
                        title: Set(clean_title),
                        agent_type: Set(agent_type_str),
                        status: Set(ConversationStatus::PendingReview),
                        model: Set(None),
                        git_branch: Set(git_branch),
                        external_id: Set(Some(original_for_tx)),
                        parent_id: Set(None),
                        message_count: Set(0),
                        created_at: Set(now),
                        updated_at: Set(now),
                        deleted_at: Set(None),
                    };
                    let inserted = sibling.insert(txn).await?;
                    Ok(inserted.id)
                })
            })
            .await
            .map_err(|e| AcpError::protocol(e.to_string()))?;

        Ok(ForkResultInfo {
            forked_session_id,
            original_session_id,
            sibling_conversation_id: sibling_id,
        })
    }

    pub async fn disconnect(&self, conn_id: &str) -> Result<(), AcpError> {
        let cmd_tx = {
            let mut connections = self.connections.lock().await;
            connections.remove(conn_id).map(|conn| conn.cmd_tx)
        };
        if let Some(cmd_tx) = cmd_tx {
            eprintln!("[ACP] disconnect connection={}", conn_id);
            let _ = cmd_tx.send(ConnectionCommand::Disconnect).await;
            Ok(())
        } else {
            Err(AcpError::ConnectionNotFound(conn_id.into()))
        }
    }

    pub async fn disconnect_by_owner_window(&self, owner_window_label: &str) -> usize {
        let cmd_txs = {
            let mut connections = self.connections.lock().await;
            let ids: Vec<String> = connections
                .iter()
                .filter_map(|(id, conn)| {
                    if conn.owner_window_label == owner_window_label {
                        Some(id.clone())
                    } else {
                        None
                    }
                })
                .collect();

            let mut txs = Vec::with_capacity(ids.len());
            for id in ids {
                if let Some(conn) = connections.remove(&id) {
                    txs.push(conn.cmd_tx);
                }
            }
            txs
        };

        let disconnected = cmd_txs.len();
        for cmd_tx in cmd_txs {
            let _ = cmd_tx.send(ConnectionCommand::Disconnect).await;
        }
        eprintln!(
            "[ACP] disconnect by owner window owner_window={} count={}",
            owner_window_label, disconnected
        );
        disconnected
    }

    pub async fn disconnect_all(&self) -> usize {
        let cmd_txs: Vec<_> = {
            let mut connections = self.connections.lock().await;
            connections.drain().map(|(_, conn)| conn.cmd_tx).collect()
        };
        let disconnected = cmd_txs.len();
        for cmd_tx in cmd_txs {
            let _ = cmd_tx.send(ConnectionCommand::Disconnect).await;
        }
        eprintln!("[ACP] disconnect_all count={}", disconnected);
        disconnected
    }

    pub async fn list_connections(&self) -> Vec<ConnectionInfo> {
        let connections = self.connections.lock().await;
        connections.values().map(|c| c.info()).collect()
    }

    /// Clone the `Arc<RwLock<SessionState>>` for a given connection id so the
    /// caller can read/write state without holding the connections mutex.
    /// Returns `None` if no such connection is registered.
    pub async fn get_state(
        &self,
        conn_id: &str,
    ) -> Option<std::sync::Arc<tokio::sync::RwLock<crate::acp::SessionState>>> {
        let connections = self.connections.lock().await;
        connections.get(conn_id).map(|conn| conn.state.clone())
    }

    /// Like `get_state`, but also clones the connection's `EventEmitter`.
    /// Used by the lifecycle subscriber when it needs to both update the
    /// per-session state and re-broadcast a derived event (e.g. emitting
    /// `ConversationStatusChanged` after writing the row's status).
    /// One short lock on the connections map; both pieces are cheap to clone.
    pub async fn get_state_and_emitter(
        &self,
        conn_id: &str,
    ) -> Option<(
        std::sync::Arc<tokio::sync::RwLock<crate::acp::SessionState>>,
        EventEmitter,
    )> {
        let connections = self.connections.lock().await;
        connections
            .get(conn_id)
            .map(|conn| (conn.state.clone(), conn.emitter.clone()))
    }

    /// Resolve a conversation_id to its currently-active connection id, if any.
    /// Used by the by-conversation snapshot endpoint and the LifecycleSubscriber.
    /// Per-session state is acquired via `read().await` to avoid the
    /// `try_read`-skip false negative that would intermittently return None
    /// while `emit_with_state` is mid-update — the wait is microseconds.
    pub async fn find_connection_by_conversation_id(&self, conversation_id: i32) -> Option<String> {
        let connections = self.connections.lock().await;
        for (id, conn) in connections.iter() {
            let state = conn.state.read().await;
            if state.conversation_id == Some(conversation_id) {
                return Some(id.clone());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::connection::AgentConnection;
    use crate::acp::session_state::SessionState;
    use crate::acp::types::ConnectionStatus;
    use crate::web::event_bridge::{EventEmitter, WebEvent, WebEventBroadcaster};
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::{broadcast, mpsc, RwLock};

    fn fake_connection(id: &str, conv_id: Option<i32>) -> AgentConnection {
        let (tx, _rx) = mpsc::channel(1);
        let mut state = SessionState::new(
            id.to_string(),
            crate::models::agent::AgentType::ClaudeCode,
            None,
            "test-window".to_string(),
            None,
        );
        state.conversation_id = conv_id;
        state.status = ConnectionStatus::Connected;
        AgentConnection {
            id: id.to_string(),
            agent_type: crate::models::agent::AgentType::ClaudeCode,
            status: ConnectionStatus::Connected,
            owner_window_label: "test-window".to_string(),
            cmd_tx: tx,
            state: Arc::new(RwLock::new(state)),
            emitter: EventEmitter::Noop,
            prompt_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Build a broadcaster + subscribed receiver. Subscribing here (not lazily
    /// inside the test) ensures events emitted between construction and the
    /// first `recv` are buffered rather than dropped.
    fn make_test_broadcaster() -> (Arc<WebEventBroadcaster>, broadcast::Receiver<WebEvent>) {
        let bcast = Arc::new(WebEventBroadcaster::new());
        let rx = bcast.subscribe();
        (bcast, rx)
    }

    /// Thin wrapper around `ConnectionManager::insert_test_connection` so the
    /// existing in-crate tests keep their `insert_fake_connection(mgr, ...)`
    /// call shape after the public test helper landed.
    async fn insert_fake_connection(
        mgr: &ConnectionManager,
        id: &str,
        agent_type: crate::models::agent::AgentType,
        working_dir: Option<PathBuf>,
        emitter: EventEmitter,
    ) {
        mgr.insert_test_connection(id, agent_type, working_dir, emitter)
            .await;
    }

    /// Subscribe directly to the per-connection event stream. Phase 4b
    /// removed the dual-broadcast through the global `WebEventBroadcaster`
    /// for ACP events; the per-connection stream is now the only delivery
    /// path tests can observe. Subscribe BEFORE triggering the producing
    /// call so events emitted between subscribe and recv buffer rather
    /// than drop.
    async fn subscribe_conn_stream(
        mgr: &ConnectionManager,
        conn_id: &str,
    ) -> broadcast::Receiver<std::sync::Arc<crate::acp::types::EventEnvelope>> {
        let state = mgr
            .get_state(conn_id)
            .await
            .expect("connection should be registered");
        let stream = state.read().await.event_stream();
        stream.subscribe()
    }

    /// Receive the first envelope from a per-connection stream. Times out
    /// after 200 ms to keep tests honest.
    async fn recv_first_acp_event(
        rx: &mut broadcast::Receiver<std::sync::Arc<crate::acp::types::EventEnvelope>>,
    ) -> crate::acp::types::EventEnvelope {
        let evt = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("timed out waiting for acp event")
            .expect("per-connection stream closed");
        (*evt).clone()
    }

    #[tokio::test]
    async fn get_state_returns_arc_for_known_connection() {
        let mgr = ConnectionManager::new();
        {
            let mut map = mgr.connections.lock().await;
            map.insert("c1".to_string(), fake_connection("c1", None));
        }
        let state = mgr.get_state("c1").await.expect("state should be found");
        assert_eq!(state.read().await.connection_id, "c1");
    }

    #[tokio::test]
    async fn get_state_returns_none_for_unknown_connection() {
        let mgr = ConnectionManager::new();
        assert!(mgr.get_state("does-not-exist").await.is_none());
    }

    #[tokio::test]
    async fn find_connection_by_conversation_id_matches_when_bound() {
        let mgr = ConnectionManager::new();
        {
            let mut map = mgr.connections.lock().await;
            map.insert("c1".to_string(), fake_connection("c1", Some(42)));
            map.insert("c2".to_string(), fake_connection("c2", None));
        }
        let found = mgr
            .find_connection_by_conversation_id(42)
            .await
            .expect("should find c1");
        assert_eq!(found, "c1");
        assert!(mgr.find_connection_by_conversation_id(999).await.is_none());
    }

    #[tokio::test]
    async fn send_prompt_linked_creates_conversation_on_first_call_only() {
        use crate::db::test_helpers;
        let db = test_helpers::fresh_in_memory_db().await;
        let folder_id = test_helpers::seed_folder(&db, "/tmp/test").await;

        let mgr = ConnectionManager::new();
        let conn_id = "c1";
        {
            let mut map = mgr.connections.lock().await;
            // Note: cmd_tx receiver is dropped, so send_prompt's mpsc.send will fail
            // with ProcessExited. That's fine — we only verify the linkage side
            // effect, not the actual prompt forwarding.
            map.insert(conn_id.into(), fake_connection(conn_id, None));
        }

        // First call: creates conversation row, sets state.conversation_id.
        // The mpsc send error after linking is expected and ignored here.
        let _ = mgr
            .send_prompt_linked(&db, conn_id, vec![], Some(folder_id), None)
            .await;
        let snap = mgr
            .get_state(conn_id)
            .await
            .unwrap()
            .read()
            .await
            .to_snapshot();
        assert!(
            snap.conversation_id.is_some(),
            "conversation_id should be set"
        );
        assert_eq!(snap.folder_id, Some(folder_id));
        let first_id = snap.conversation_id.unwrap();

        // Second call: ignores folder_id, does NOT create another row.
        let _ = mgr
            .send_prompt_linked(&db, conn_id, vec![], Some(folder_id), None)
            .await;
        let snap2 = mgr
            .get_state(conn_id)
            .await
            .unwrap()
            .read()
            .await
            .to_snapshot();
        assert_eq!(snap2.conversation_id, Some(first_id));
    }

    #[tokio::test]
    async fn send_prompt_linked_errors_when_no_folder_id() {
        use crate::db::test_helpers;
        let db = test_helpers::fresh_in_memory_db().await;
        let mgr = ConnectionManager::new();
        let conn_id = "c1";
        {
            let mut map = mgr.connections.lock().await;
            map.insert(conn_id.into(), fake_connection(conn_id, None));
        }
        let result = mgr
            .send_prompt_linked(&db, conn_id, vec![], None, None)
            .await;
        assert!(
            result.is_err(),
            "should error when folder_id is not provided for a new conversation row"
        );
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("folder_id"),
            "error should mention missing folder_id, got: {err_str}"
        );
    }

    /// Count of `conversation` rows (ignoring soft-delete) — used by the
    /// caller-supplied conversation_id tests to assert no new row was created.
    async fn count_conversation_rows(db: &crate::db::AppDatabase) -> usize {
        use crate::db::entities::conversation;
        use sea_orm::EntityTrait;
        conversation::Entity::find()
            .all(&db.conn)
            .await
            .unwrap()
            .len()
    }

    #[tokio::test]
    async fn send_prompt_linked_uses_caller_conversation_id_when_provided() {
        use crate::db::test_helpers;
        let db = test_helpers::fresh_in_memory_db().await;
        let folder_id = test_helpers::seed_folder(&db, "/tmp/caller-id").await;
        // Pre-create a conversation row the caller will reference.
        let pre_existing =
            conversation_service::create(&db.conn, folder_id, AgentType::ClaudeCode, None, None)
                .await
                .unwrap();

        let mgr = ConnectionManager::new();
        let (broadcaster, _rx) = make_test_broadcaster();
        let conn_id = "conn-caller-id";
        insert_fake_connection(
            &mgr,
            conn_id,
            AgentType::ClaudeCode,
            Some(PathBuf::from("/tmp/caller-id")),
            EventEmitter::test_web_only(broadcaster.clone()),
        )
        .await;
        let mut rx = subscribe_conn_stream(&mgr, conn_id).await;

        // Count rows before
        let before = count_conversation_rows(&db).await;

        // Send with caller-supplied conversation_id + folder_id.
        let _ = mgr
            .send_prompt_linked(&db, conn_id, vec![], Some(folder_id), Some(pre_existing.id))
            .await;

        // No new conversation row was created.
        let after = count_conversation_rows(&db).await;
        assert_eq!(after, before, "no new row should be created");

        // State now has the caller-supplied conversation_id.
        let state = mgr.get_state(conn_id).await.unwrap();
        assert_eq!(state.read().await.conversation_id, Some(pre_existing.id));

        // ConversationLinked event was emitted with the caller's id.
        let env = recv_first_acp_event(&mut rx).await;
        match env.payload {
            AcpEvent::ConversationLinked {
                conversation_id,
                folder_id: emitted_folder,
            } => {
                assert_eq!(conversation_id, pre_existing.id);
                assert_eq!(emitted_folder, folder_id);
            }
            other => panic!("expected ConversationLinked, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn send_prompt_linked_rejects_conversation_id_without_folder_id() {
        use crate::db::test_helpers;
        let db = test_helpers::fresh_in_memory_db().await;
        let mgr = ConnectionManager::new();
        let (broadcaster, _rx) = make_test_broadcaster();
        let conn_id = "conn-bad-args";
        insert_fake_connection(
            &mgr,
            conn_id,
            AgentType::ClaudeCode,
            Some(PathBuf::from("/tmp/x")),
            EventEmitter::test_web_only(broadcaster),
        )
        .await;

        let err = mgr
            .send_prompt_linked(&db, conn_id, vec![], None, Some(42))
            .await
            .expect_err("should reject conversation_id without folder_id");
        assert!(matches!(err, AcpError::Protocol(_)));
    }

    #[tokio::test]
    async fn send_prompt_linked_caller_id_is_noop_when_already_linked() {
        use crate::db::test_helpers;
        let db = test_helpers::fresh_in_memory_db().await;
        let folder_id = test_helpers::seed_folder(&db, "/tmp/already").await;
        let pre =
            conversation_service::create(&db.conn, folder_id, AgentType::ClaudeCode, None, None)
                .await
                .unwrap();

        let mgr = ConnectionManager::new();
        let (broadcaster, _rx) = make_test_broadcaster();
        let conn_id = "conn-already";
        insert_fake_connection(
            &mgr,
            conn_id,
            AgentType::ClaudeCode,
            Some(PathBuf::from("/tmp/already")),
            EventEmitter::test_web_only(broadcaster.clone()),
        )
        .await;
        // Pre-link the connection state.
        {
            let state = mgr.get_state(conn_id).await.unwrap();
            state.write().await.conversation_id = Some(pre.id);
        }
        let mut rx = subscribe_conn_stream(&mgr, conn_id).await;

        let before = count_conversation_rows(&db).await;
        let _ = mgr
            .send_prompt_linked(&db, conn_id, vec![], Some(folder_id), Some(pre.id))
            .await;
        let after = count_conversation_rows(&db).await;
        assert_eq!(after, before);

        // No ConversationLinked event was emitted (already linked). The
        // centralized status transition fires InProgress; then because the
        // dropped cmd_tx receiver makes `send_prompt_inner` return
        // ProcessExited, the rollback path fires Cancelled. Two events,
        // strictly ordered.
        let env_in_progress = recv_first_acp_event(&mut rx).await;
        match env_in_progress.payload {
            AcpEvent::ConversationStatusChanged {
                conversation_id,
                status,
            } => {
                assert_eq!(conversation_id, pre.id);
                assert_eq!(status, ConversationStatus::InProgress);
            }
            other => {
                panic!("first event must be ConversationStatusChanged(InProgress), got {other:?}")
            }
        }
        let env_cancelled = recv_first_acp_event(&mut rx).await;
        match env_cancelled.payload {
            AcpEvent::ConversationStatusChanged {
                conversation_id,
                status,
            } => {
                assert_eq!(conversation_id, pre.id);
                assert_eq!(status, ConversationStatus::Cancelled);
            }
            other => panic!(
                "second event must be ConversationStatusChanged(Cancelled) after send failure, got {other:?}"
            ),
        }
    }

    // ---------- Phase: status centralization ----------

    #[tokio::test]
    async fn send_prompt_linked_writes_in_progress_and_emits_event() {
        use crate::db::entities::conversation;
        use crate::db::test_helpers;
        use sea_orm::EntityTrait;

        let db = test_helpers::fresh_in_memory_db().await;
        let folder_id = test_helpers::seed_folder(&db, "/tmp/status").await;

        let mgr = ConnectionManager::new();
        let (broadcaster, _rx) = make_test_broadcaster();
        let conn_id = "conn-status-1";
        insert_fake_connection(
            &mgr,
            conn_id,
            AgentType::ClaudeCode,
            Some(PathBuf::from("/tmp/status")),
            EventEmitter::test_web_only(broadcaster.clone()),
        )
        .await;
        let mut rx = subscribe_conn_stream(&mgr, conn_id).await;

        // First call: backend creates the conversation row and links it.
        // The cmd_tx receiver in `insert_fake_connection` has been dropped,
        // so `send_prompt_inner` returns ProcessExited — exercising the new
        // Cancelled-rollback path. We expect THREE events in order:
        //   1. ConversationLinked
        //   2. ConversationStatusChanged(InProgress)  [pre-send write]
        //   3. ConversationStatusChanged(Cancelled)   [rollback after send failure]
        let _ = mgr
            .send_prompt_linked(&db, conn_id, vec![], Some(folder_id), None)
            .await;

        let env1 = recv_first_acp_event(&mut rx).await;
        let conv_id = match env1.payload {
            AcpEvent::ConversationLinked {
                conversation_id,
                folder_id: emitted_folder,
            } => {
                assert_eq!(emitted_folder, folder_id);
                conversation_id
            }
            other => panic!("first event must be ConversationLinked, got {other:?}"),
        };
        let env2 = recv_first_acp_event(&mut rx).await;
        match env2.payload {
            AcpEvent::ConversationStatusChanged {
                conversation_id,
                status,
            } => {
                assert_eq!(conversation_id, conv_id);
                assert_eq!(status, ConversationStatus::InProgress);
            }
            other => {
                panic!("second event must be ConversationStatusChanged(InProgress), got {other:?}")
            }
        }
        let env3 = recv_first_acp_event(&mut rx).await;
        match env3.payload {
            AcpEvent::ConversationStatusChanged {
                conversation_id,
                status,
            } => {
                assert_eq!(conversation_id, conv_id);
                assert_eq!(status, ConversationStatus::Cancelled);
            }
            other => panic!(
                "third event must be ConversationStatusChanged(Cancelled) on send failure, got {other:?}"
            ),
        }
        // Ordering invariant: ConversationLinked < InProgress < Cancelled.
        assert!(
            env2.seq > env1.seq && env3.seq > env2.seq,
            "event seqs must be strictly monotonic: linked={} in_progress={} cancelled={}",
            env1.seq,
            env2.seq,
            env3.seq
        );

        // DB row settles at Cancelled (the rollback after send failure). The
        // intermediate InProgress write is observable only via the event,
        // not by the time the test reads the row.
        let row = conversation::Entity::find_by_id(conv_id)
            .one(&db.conn)
            .await
            .unwrap()
            .expect("conversation row exists");
        assert_eq!(row.status, ConversationStatus::Cancelled);

        // Second send: already-linked path also writes + emits InProgress
        // and then Cancelled (same send-failure rollback). Pre-flip the row
        // to PendingReview to observe the transition flip forward — mirrors
        // the "follow-up turn after a TurnComplete" scenario.
        conversation_service::update_status(&db.conn, conv_id, ConversationStatus::PendingReview)
            .await
            .unwrap();

        let _ = mgr
            .send_prompt_linked(&db, conn_id, vec![], Some(folder_id), None)
            .await;

        let env4 = recv_first_acp_event(&mut rx).await;
        match env4.payload {
            AcpEvent::ConversationStatusChanged {
                conversation_id,
                status,
            } => {
                assert_eq!(conversation_id, conv_id);
                assert_eq!(status, ConversationStatus::InProgress);
            }
            other => panic!(
                "second send must re-emit ConversationStatusChanged(InProgress) first, got {other:?}"
            ),
        }
        let env5 = recv_first_acp_event(&mut rx).await;
        match env5.payload {
            AcpEvent::ConversationStatusChanged {
                conversation_id,
                status,
            } => {
                assert_eq!(conversation_id, conv_id);
                assert_eq!(status, ConversationStatus::Cancelled);
            }
            other => {
                panic!("second send must rollback to Cancelled after send failure, got {other:?}")
            }
        }
        let row2 = conversation::Entity::find_by_id(conv_id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row2.status, ConversationStatus::Cancelled);
    }

    // ---------- Phase: connection dedup ----------

    #[tokio::test]
    async fn find_connection_for_reuse_returns_none_when_session_id_is_none() {
        let mgr = ConnectionManager::new();
        let (broadcaster, _rx) = make_test_broadcaster();
        // Insert a connection that *would* match if session_id were Some.
        let id = "c1";
        insert_fake_connection(
            &mgr,
            id,
            AgentType::ClaudeCode,
            Some(PathBuf::from("/tmp/reuse")),
            EventEmitter::test_web_only(broadcaster),
        )
        .await;
        {
            let state = mgr.get_state(id).await.unwrap();
            state.write().await.external_id = Some("ext-1".into());
        }
        let found = mgr
            .find_connection_for_reuse(
                AgentType::ClaudeCode,
                Some(&PathBuf::from("/tmp/reuse")),
                None,
            )
            .await;
        assert!(
            found.is_none(),
            "no session_id means we never dedup speculative connects"
        );
    }

    #[tokio::test]
    async fn spawn_agent_reuses_existing_connection_when_session_id_matches() {
        // Direct unit test for the lookup helper that spawn_agent calls
        // before its (process-spawning) block. We test the helper directly so
        // the test never tries to launch an agent process.
        let mgr = ConnectionManager::new();
        let (broadcaster, _rx) = make_test_broadcaster();
        let existing_id = "preexisting-conn";
        let working_dir = PathBuf::from("/tmp/reuse-match");
        insert_fake_connection(
            &mgr,
            existing_id,
            AgentType::ClaudeCode,
            Some(working_dir.clone()),
            EventEmitter::test_web_only(broadcaster.clone()),
        )
        .await;
        {
            let state = mgr.get_state(existing_id).await.unwrap();
            let mut s = state.write().await;
            s.external_id = Some("ext-1".into());
            s.status = ConnectionStatus::Connected;
        }

        // Same session_id + same agent + same working_dir -> reuse.
        let found = mgr
            .find_connection_for_reuse(AgentType::ClaudeCode, Some(&working_dir), Some("ext-1"))
            .await;
        assert_eq!(found.as_deref(), Some(existing_id));

        // Different session_id -> no reuse.
        assert!(mgr
            .find_connection_for_reuse(AgentType::ClaudeCode, Some(&working_dir), Some("other-ext"))
            .await
            .is_none());

        // Different working_dir -> no reuse.
        assert!(mgr
            .find_connection_for_reuse(
                AgentType::ClaudeCode,
                Some(&PathBuf::from("/tmp/different")),
                Some("ext-1")
            )
            .await
            .is_none());

        // Different agent_type -> no reuse.
        assert!(mgr
            .find_connection_for_reuse(AgentType::Codex, Some(&working_dir), Some("ext-1"))
            .await
            .is_none());
    }

    #[tokio::test]
    async fn find_connection_for_reuse_skips_disconnected_or_errored() {
        let mgr = ConnectionManager::new();
        let (broadcaster, _rx) = make_test_broadcaster();
        let working_dir = PathBuf::from("/tmp/torn-down");
        insert_fake_connection(
            &mgr,
            "torn",
            AgentType::ClaudeCode,
            Some(working_dir.clone()),
            EventEmitter::test_web_only(broadcaster.clone()),
        )
        .await;
        {
            let state = mgr.get_state("torn").await.unwrap();
            let mut s = state.write().await;
            s.external_id = Some("ext-1".into());
            s.status = ConnectionStatus::Disconnected;
        }
        assert!(
            mgr.find_connection_for_reuse(
                AgentType::ClaudeCode,
                Some(&working_dir),
                Some("ext-1"),
            )
            .await
            .is_none(),
            "Disconnected connection must not be reused"
        );

        // Flip to Error — also excluded.
        {
            let state = mgr.get_state("torn").await.unwrap();
            state.write().await.status = ConnectionStatus::Error;
        }
        assert!(
            mgr.find_connection_for_reuse(
                AgentType::ClaudeCode,
                Some(&working_dir),
                Some("ext-1"),
            )
            .await
            .is_none(),
            "Errored connection must not be reused"
        );
    }

    /// Helper that backdates a connection's `last_activity_at` so the
    /// idle sweep sees it as having crossed its threshold.
    async fn backdate_last_activity(mgr: &ConnectionManager, conn_id: &str, secs_ago: i64) {
        let state = mgr.get_state(conn_id).await.expect("connection exists");
        let mut s = state.write().await;
        s.last_activity_at = chrono::Utc::now() - chrono::Duration::seconds(secs_ago);
    }

    #[tokio::test]
    async fn sweep_idle_disconnects_idle_connected_connections() {
        let mgr = ConnectionManager::new();
        insert_fake_connection(
            &mgr,
            "stale",
            AgentType::ClaudeCode,
            Some(PathBuf::from("/tmp/stale")),
            EventEmitter::Noop,
        )
        .await;
        backdate_last_activity(&mgr, "stale", 600).await;

        let n = mgr.sweep_idle(Duration::from_secs(300)).await;
        assert_eq!(n, 1);
        assert!(
            mgr.connections.lock().await.get("stale").is_none(),
            "Idle connection must be removed after sweep"
        );
    }

    #[tokio::test]
    async fn sweep_idle_skips_recently_active_connection() {
        let mgr = ConnectionManager::new();
        insert_fake_connection(
            &mgr,
            "fresh",
            AgentType::ClaudeCode,
            None,
            EventEmitter::Noop,
        )
        .await;
        // last_activity_at defaults to "now" inside SessionState::new — no
        // backdating, so it should NOT be swept.
        let n = mgr.sweep_idle(Duration::from_secs(300)).await;
        assert_eq!(n, 0);
        assert!(mgr.connections.lock().await.contains_key("fresh"));
    }

    #[tokio::test]
    async fn sweep_idle_skips_prompting_connection() {
        let mgr = ConnectionManager::new();
        insert_fake_connection(
            &mgr,
            "prompting",
            AgentType::ClaudeCode,
            None,
            EventEmitter::Noop,
        )
        .await;
        backdate_last_activity(&mgr, "prompting", 600).await;
        // Override status to Prompting — a turn is in flight; never sweep.
        {
            let state = mgr.get_state("prompting").await.unwrap();
            state.write().await.status = ConnectionStatus::Prompting;
        }
        let n = mgr.sweep_idle(Duration::from_secs(300)).await;
        assert_eq!(n, 0);
        assert!(mgr.connections.lock().await.contains_key("prompting"));
    }

    #[tokio::test]
    async fn sweep_idle_skips_pending_permission() {
        use crate::acp::session_state::PendingPermissionState;
        let mgr = ConnectionManager::new();
        insert_fake_connection(
            &mgr,
            "permission",
            AgentType::ClaudeCode,
            None,
            EventEmitter::Noop,
        )
        .await;
        backdate_last_activity(&mgr, "permission", 600).await;
        {
            let state = mgr.get_state("permission").await.unwrap();
            state.write().await.pending_permission = Some(PendingPermissionState {
                request_id: "req-1".into(),
                tool_call_id: "tc-1".into(),
                tool_call: serde_json::json!({ "toolCallId": "tc-1", "title": "test" }),
                options: vec![],
                created_at: chrono::Utc::now(),
            });
        }
        let n = mgr.sweep_idle(Duration::from_secs(300)).await;
        assert_eq!(
            n, 0,
            "Connection with pending permission must not be swept (user is mid-decision)"
        );
        assert!(mgr.connections.lock().await.contains_key("permission"));
    }

    #[tokio::test]
    async fn sweep_idle_picks_only_qualifying_subset() {
        let mgr = ConnectionManager::new();
        for id in ["a", "b", "c"] {
            insert_fake_connection(&mgr, id, AgentType::ClaudeCode, None, EventEmitter::Noop).await;
        }
        // a: idle (sweep target), b: fresh (not idle), c: idle but Prompting (skipped).
        backdate_last_activity(&mgr, "a", 600).await;
        backdate_last_activity(&mgr, "c", 600).await;
        {
            let state = mgr.get_state("c").await.unwrap();
            state.write().await.status = ConnectionStatus::Prompting;
        }
        let n = mgr.sweep_idle(Duration::from_secs(300)).await;
        assert_eq!(n, 1);
        let map = mgr.connections.lock().await;
        assert!(!map.contains_key("a"));
        assert!(map.contains_key("b"));
        assert!(map.contains_key("c"));
    }

    /// When two `spawn_agent` calls race for the same logical session id,
    /// the per-key dedup mutex makes the second one observe the first's
    /// freshly-spawned connection and reuse it. Without the mutex, both
    /// would have missed dedup during the connecting window.
    ///
    /// Simulates the race by pre-inserting a "first call's connection" with
    /// `external_id` set; what's tested is that two concurrent
    /// `find_connection_for_reuse` calls under the same lock see consistent
    /// state. The `spawn_locks` map being shared via `clone_ref` is the
    /// invariant we need.
    #[tokio::test]
    async fn spawn_locks_are_shared_across_clone_ref() {
        let mgr = ConnectionManager::new();
        let cloned = mgr.clone_ref();
        // Both clones must reference the same map. Insert via one,
        // observe via the other.
        let key = SpawnDedupKey {
            agent_type: AgentType::ClaudeCode,
            working_dir: Some(PathBuf::from("/tmp/dedup-test")),
            session_id: "ext-shared".into(),
        };
        {
            let mut locks = mgr.spawn_locks.lock().await;
            locks
                .entry(key.clone())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())));
        }
        let cloned_locks = cloned.spawn_locks.lock().await;
        assert!(
            cloned_locks.contains_key(&key),
            "spawn_locks must be shared between original and clone_ref"
        );
    }

    /// Two concurrent `send_prompt_linked` calls on the SAME connection
    /// must serialize through the per-connection `prompt_lock` so the
    /// backend-creates branch can't fire twice and produce duplicate
    /// conversation rows. The second call observes `already_linked == true`
    /// (set by the first under the lock) and skips creation.
    #[tokio::test]
    async fn send_prompt_linked_serializes_concurrent_callers() {
        use crate::db::test_helpers;
        let db = test_helpers::fresh_in_memory_db().await;
        let folder_id = test_helpers::seed_folder(&db, "/tmp/race").await;

        let mgr = Arc::new(ConnectionManager::new());
        let conn_id = "race-conn";
        {
            let mut map = mgr.connections.lock().await;
            map.insert(conn_id.into(), fake_connection(conn_id, None));
        }

        let before = count_conversation_rows(&db).await;
        // tokio::join! polls the two futures concurrently in the SAME
        // task — they can borrow `&db` and `mgr` without the 'static
        // requirement that `tokio::spawn` would impose.
        let mgr_ref = mgr.as_ref();
        tokio::join!(
            async {
                let _ = mgr_ref
                    .send_prompt_linked(&db, conn_id, vec![], Some(folder_id), None)
                    .await;
            },
            async {
                let _ = mgr_ref
                    .send_prompt_linked(&db, conn_id, vec![], Some(folder_id), None)
                    .await;
            },
        );

        let after = count_conversation_rows(&db).await;
        assert_eq!(
            after - before,
            1,
            "exactly one new conversation row across two concurrent send_prompt_linked"
        );
    }

    // ---------- Phase: spawn handshake wait helper ----------

    #[tokio::test]
    async fn wait_for_session_started_returns_ready_when_sender_fires() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        // Fire immediately on a separate task so the wait future actually
        // gets to register.
        tokio::spawn(async move {
            let _ = tx.send(());
        });
        let (outcome, elapsed) = wait_for_session_started(rx, Duration::from_millis(500)).await;
        assert_eq!(outcome, HandshakeWaitOutcome::Ready);
        assert!(
            elapsed < Duration::from_millis(500),
            "Ready outcome must resolve well before timeout, got {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn wait_for_session_started_returns_aborted_when_sender_drops() {
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        // Drop the sender — emulates "connection died before SessionStarted",
        // i.e. SessionState's tx was dropped during cleanup.
        drop(tx);
        let (outcome, elapsed) = wait_for_session_started(rx, Duration::from_millis(500)).await;
        assert_eq!(outcome, HandshakeWaitOutcome::Aborted);
        assert!(
            elapsed < Duration::from_millis(500),
            "Aborted outcome must resolve well before timeout, got {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn wait_for_session_started_returns_timed_out_when_neither_happens() {
        let (_tx, rx) = tokio::sync::oneshot::channel::<()>();
        // Hold the sender alive but never fire and never drop. Tight
        // timeout so the test stays fast; production timeout is 60s.
        let (outcome, elapsed) = wait_for_session_started(rx, Duration::from_millis(40)).await;
        assert_eq!(outcome, HandshakeWaitOutcome::TimedOut);
        assert!(
            elapsed >= Duration::from_millis(40),
            "TimedOut must wait at least the full timeout, got {elapsed:?}"
        );
    }

    #[test]
    fn spawn_handshake_timeout_from_env_uses_default_when_unset() {
        // Snapshot env, mutate, restore. Single test owns this var to avoid
        // cross-test contention.
        let prev = std::env::var("CODEG_ACP_SPAWN_HANDSHAKE_TIMEOUT_SECS").ok();
        std::env::remove_var("CODEG_ACP_SPAWN_HANDSHAKE_TIMEOUT_SECS");
        let default = spawn_handshake_timeout_from_env();
        assert_eq!(default, Duration::from_secs(SPAWN_HANDSHAKE_TIMEOUT_SECS));

        std::env::set_var("CODEG_ACP_SPAWN_HANDSHAKE_TIMEOUT_SECS", "5");
        assert_eq!(spawn_handshake_timeout_from_env(), Duration::from_secs(5));

        std::env::set_var("CODEG_ACP_SPAWN_HANDSHAKE_TIMEOUT_SECS", "garbage");
        assert_eq!(
            spawn_handshake_timeout_from_env(),
            Duration::from_secs(SPAWN_HANDSHAKE_TIMEOUT_SECS),
            "invalid value falls back to default"
        );

        // Restore.
        match prev {
            Some(v) => std::env::set_var("CODEG_ACP_SPAWN_HANDSHAKE_TIMEOUT_SECS", v),
            None => std::env::remove_var("CODEG_ACP_SPAWN_HANDSHAKE_TIMEOUT_SECS"),
        }
    }

    #[test]
    fn with_spawn_handshake_timeout_overrides_default_for_tests() {
        let mgr = ConnectionManager::with_spawn_handshake_timeout(Duration::from_secs(7));
        assert_eq!(mgr.spawn_handshake_timeout, Duration::from_secs(7));
    }

    /// When `send_prompt_inner` fails (process gone, channel closed) the row
    /// must end up `Cancelled`, NOT stuck on `in_progress`. Without this
    /// rollback the lifecycle subscriber's TurnComplete write never fires
    /// (no turn ever started), so the only thing that could later un-stick
    /// the row is a follow-up prompt happening to succeed — fragile, and on
    /// the server-side / chat-channel paths there may be no follow-up at all.
    #[tokio::test]
    async fn send_prompt_linked_rolls_back_to_cancelled_on_send_failure() {
        use crate::db::entities::conversation;
        use crate::db::test_helpers;
        use sea_orm::EntityTrait;

        let db = test_helpers::fresh_in_memory_db().await;
        let folder_id = test_helpers::seed_folder(&db, "/tmp/cancel-rollback").await;

        let mgr = ConnectionManager::new();
        let (broadcaster, _rx) = make_test_broadcaster();
        let conn_id = "conn-cancel";
        // insert_fake_connection drops the cmd_tx receiver, so send_prompt_inner
        // returns ProcessExited — exactly the failure mode this test targets.
        insert_fake_connection(
            &mgr,
            conn_id,
            AgentType::ClaudeCode,
            Some(PathBuf::from("/tmp/cancel-rollback")),
            EventEmitter::test_web_only(broadcaster.clone()),
        )
        .await;
        let mut rx = subscribe_conn_stream(&mgr, conn_id).await;

        let result = mgr
            .send_prompt_linked(&db, conn_id, vec![], Some(folder_id), None)
            .await;
        assert!(
            matches!(result, Err(AcpError::ProcessExited)),
            "send_prompt_inner must propagate ProcessExited up to the caller; got {result:?}"
        );

        // Drain events: ConversationLinked → InProgress → Cancelled, in order.
        let env_linked = recv_first_acp_event(&mut rx).await;
        let conv_id = match env_linked.payload {
            AcpEvent::ConversationLinked {
                conversation_id, ..
            } => conversation_id,
            other => panic!("expected ConversationLinked first, got {other:?}"),
        };
        let env_in_progress = recv_first_acp_event(&mut rx).await;
        match env_in_progress.payload {
            AcpEvent::ConversationStatusChanged { status, .. } => {
                assert_eq!(status, ConversationStatus::InProgress);
            }
            other => {
                panic!("expected ConversationStatusChanged(InProgress) before send, got {other:?}")
            }
        }
        let env_cancelled = recv_first_acp_event(&mut rx).await;
        match env_cancelled.payload {
            AcpEvent::ConversationStatusChanged {
                conversation_id,
                status,
            } => {
                assert_eq!(conversation_id, conv_id);
                assert_eq!(
                    status,
                    ConversationStatus::Cancelled,
                    "send_prompt failure must roll the row forward to Cancelled, not leave InProgress"
                );
            }
            other => panic!(
                "expected ConversationStatusChanged(Cancelled) on send failure, got {other:?}"
            ),
        }

        // Strict ordering: linked < in_progress < cancelled. The lifecycle
        // contract says the Cancelled emit cannot precede the InProgress one
        // — UIs that animate based on "previous → current" depend on this.
        assert!(
            env_in_progress.seq > env_linked.seq && env_cancelled.seq > env_in_progress.seq,
            "event seq must be strictly monotonic: linked={} in_progress={} cancelled={}",
            env_linked.seq,
            env_in_progress.seq,
            env_cancelled.seq,
        );

        // DB row settles at Cancelled — final ground truth read.
        let row = conversation::Entity::find_by_id(conv_id)
            .one(&db.conn)
            .await
            .unwrap()
            .expect("conversation row exists");
        assert_eq!(row.status, ConversationStatus::Cancelled);
    }

    // ---------- fork_session ----------

    /// Build a connection whose cmd_rx is drained by a spawned task that
    /// fakes the protocol-level fork reply. Returns the manager so the test
    /// can call `fork_session`. The fake reply task lives until it processes
    /// one Fork command, then exits.
    async fn manager_with_fake_fork(
        conn_id: &str,
        conversation_id: i32,
        forked_session_id: &str,
        original_session_id: &str,
    ) -> (Arc<ConnectionManager>, tokio::task::JoinHandle<()>) {
        use crate::acp::connection::ConnectionCommand;
        let (tx, mut rx) = mpsc::channel::<ConnectionCommand>(4);
        let mut state = SessionState::new(
            conn_id.to_string(),
            crate::models::agent::AgentType::ClaudeCode,
            None,
            "test-window".to_string(),
            None,
        );
        state.conversation_id = Some(conversation_id);
        state.status = ConnectionStatus::Connected;
        let conn = AgentConnection {
            id: conn_id.to_string(),
            agent_type: crate::models::agent::AgentType::ClaudeCode,
            status: ConnectionStatus::Connected,
            owner_window_label: "test-window".to_string(),
            cmd_tx: tx,
            state: Arc::new(RwLock::new(state)),
            emitter: EventEmitter::Noop,
            prompt_lock: Arc::new(tokio::sync::Mutex::new(())),
        };
        let mgr = Arc::new(ConnectionManager::new());
        {
            let mut map = mgr.connections.lock().await;
            map.insert(conn_id.to_string(), conn);
        }

        let forked = forked_session_id.to_string();
        let original = original_session_id.to_string();
        let join = tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                if let ConnectionCommand::Fork { reply } = cmd {
                    let _ = reply.send(Ok(crate::acp::types::ForkProtocolResult {
                        forked_session_id: forked.clone(),
                        original_session_id: original.clone(),
                    }));
                    return;
                }
            }
        });
        (mgr, join)
    }

    #[tokio::test]
    async fn fork_session_writes_atomic_two_row_layout() {
        use crate::db::test_helpers;
        let db = test_helpers::fresh_in_memory_db().await;
        let folder_id = test_helpers::seed_folder(&db, "/tmp/fork-happy").await;

        // Pre-existing row: stands in for the conversation about to be forked.
        // Title gets a `[Fork] ` prefix; sibling row inherits the clean title.
        let pre = conversation_service::create(
            &db.conn,
            folder_id,
            AgentType::ClaudeCode,
            Some("Original Topic".into()),
            Some("feature/x".into()),
        )
        .await
        .unwrap();
        // External_id starts as S1 — manager.fork_session will swap to S2.
        conversation_service::update_external_id(&db.conn, pre.id, "session-S1".into())
            .await
            .unwrap();

        let (mgr, join) =
            manager_with_fake_fork("c-fork", pre.id, "session-S2", "session-S1").await;
        let result = mgr
            .fork_session(&db, "c-fork")
            .await
            .expect("fork_session should succeed");
        let _ = join.await;

        assert_eq!(result.forked_session_id, "session-S2");
        assert_eq!(result.original_session_id, "session-S1");
        let sibling_id = result.sibling_conversation_id;
        assert_ne!(sibling_id, pre.id, "sibling row must be a fresh row");

        // Current row: external_id=S2, title prefixed.
        let current = conversation_service::get_by_id(&db.conn, pre.id)
            .await
            .unwrap();
        assert_eq!(current.external_id.as_deref(), Some("session-S2"));
        assert_eq!(current.title.as_deref(), Some("[Fork] Original Topic"));

        // Sibling row: external_id=S1, clean title, PendingReview, same folder/git_branch.
        let sibling = conversation_service::get_by_id(&db.conn, sibling_id)
            .await
            .unwrap();
        assert_eq!(sibling.external_id.as_deref(), Some("session-S1"));
        assert_eq!(sibling.title.as_deref(), Some("Original Topic"));
        assert_eq!(sibling.status, "pending_review");
        assert_eq!(sibling.folder_id, folder_id);
        assert_eq!(sibling.git_branch.as_deref(), Some("feature/x"));
    }

    #[tokio::test]
    async fn fork_session_strips_existing_fork_prefix_without_stacking() {
        use crate::db::test_helpers;
        let db = test_helpers::fresh_in_memory_db().await;
        let folder_id = test_helpers::seed_folder(&db, "/tmp/fork-restack").await;

        // Title already has `[Fork] ` — re-fork must not produce `[Fork] [Fork] ...`.
        let pre = conversation_service::create(
            &db.conn,
            folder_id,
            AgentType::ClaudeCode,
            Some("[Fork] Topic".into()),
            None,
        )
        .await
        .unwrap();
        let (mgr, join) =
            manager_with_fake_fork("c-restack", pre.id, "session-S2", "session-S1").await;
        let result = mgr.fork_session(&db, "c-restack").await.unwrap();
        let _ = join.await;

        let current = conversation_service::get_by_id(&db.conn, pre.id)
            .await
            .unwrap();
        assert_eq!(
            current.title.as_deref(),
            Some("[Fork] Topic"),
            "should re-stack as single [Fork] prefix, not [Fork] [Fork] ..."
        );
        let sibling = conversation_service::get_by_id(&db.conn, result.sibling_conversation_id)
            .await
            .unwrap();
        assert_eq!(sibling.title.as_deref(), Some("Topic"));
    }

    #[tokio::test]
    async fn fork_session_strips_no_space_fork_prefix() {
        // Defensive: a title produced outside the normal flow could lack the
        // space (e.g. external import). The frontend regex `/^\[Fork]\s*/g`
        // tolerated this; the backend strip must too, otherwise re-fork would
        // produce `[Fork] [Fork]xxx`.
        use crate::db::test_helpers;
        let db = test_helpers::fresh_in_memory_db().await;
        let folder_id = test_helpers::seed_folder(&db, "/tmp/fork-no-space").await;

        let pre = conversation_service::create(
            &db.conn,
            folder_id,
            AgentType::ClaudeCode,
            Some("[Fork]NoSpaceTitle".into()),
            None,
        )
        .await
        .unwrap();
        let (mgr, join) =
            manager_with_fake_fork("c-nosp", pre.id, "session-S2", "session-S1").await;
        mgr.fork_session(&db, "c-nosp").await.unwrap();
        let _ = join.await;

        let current = conversation_service::get_by_id(&db.conn, pre.id)
            .await
            .unwrap();
        assert_eq!(
            current.title.as_deref(),
            Some("[Fork] NoSpaceTitle"),
            "no-space prefix must be tolerantly stripped before re-stacking"
        );
    }

    #[tokio::test]
    async fn fork_session_rejects_unbound_connection() {
        // Without a linked conversation_id the sibling row would orphan S1
        // history (no row to point at it). fork_session must refuse early —
        // BEFORE sending the Fork command to the agent, so we don't burn an
        // ACP round-trip on a request we can't persist.
        use crate::db::test_helpers;
        let db = test_helpers::fresh_in_memory_db().await;
        let mgr = ConnectionManager::new();
        {
            let mut map = mgr.connections.lock().await;
            map.insert("c-unbound".into(), fake_connection("c-unbound", None));
        }
        let err = mgr
            .fork_session(&db, "c-unbound")
            .await
            .expect_err("unbound fork must error");
        assert!(
            err.to_string().contains("linked conversation row"),
            "error should mention missing linkage, got: {err}"
        );
    }
}
