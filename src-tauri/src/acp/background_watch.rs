//! Transcript-tail watcher: surfaces Claude Code's OUT-OF-TURN activity.
//!
//! Claude Code produces activity outside any codeg-driven prompt turn:
//! `<task-notification>` completions of async sub-agents (`Agent` launched in
//! the background) and background shell tasks, the agent's continued work
//! after such a notification (which can run for many minutes), and cron//loop
//! autonomous turns. None of that has a reliable ACP wire representation —
//! out-of-turn `session/update`s are forwarded but carry no turn settlement,
//! and cron turns produce NO wire events at all (claude-agent-acp #270). The
//! session's own JSONL transcript is the only complete source, so this module
//! tails it:
//!
//! * **Accounting** — launch acks are detected from the record-level
//!   `toolUseResult` (`status:"async_launched"` → `agentId`, or
//!   `backgroundTaskId` for background shells; these fields exist ONLY on
//!   disk, never on the wire), settled by `<task-notification>` records
//!   (matching `<task-id>`), re-armed when the main agent resumes a settled
//!   sub-agent via `SendMessage`, and expired past
//!   [`background_keepalive_max_age`]. The outstanding count is mirrored into
//!   `SessionState` (via `apply_event`) to exempt the connection from both
//!   idle sweeps — disconnecting kills the agent CLI, and the background work
//!   dies with it.
//!
//! * **Rendering** — new transcript records that do NOT belong to a
//!   codeg-sent prompt turn are assembled into turns with the SAME Stage-A/
//!   Stage-B code the detail parser uses ([`ClaudeRecordAccumulator`] +
//!   [`group_into_turns`]) and emitted as `AcpEvent::BackgroundActivity`
//!   upserts for the frontend's overlay slice. Foreground turns are excluded
//!   by the **prompt ledger**: every prompt codeg sends is fingerprinted, and
//!   a transcript turn whose initiating user record matches an unconsumed
//!   fingerprint is the wire-rendered foreground turn (each fingerprint is
//!   consumed exactly once, so a cron//loop re-fire of the SAME text later
//!   correctly classifies as out-of-turn).
//!
//! The watcher is connection-scoped on purpose: background work cannot outlive
//! the agent CLI process, whose lifetime IS the connection's. Poll ticks are
//! mtime-gated (an unchanged file costs one `stat`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

use crate::acp::session_state::{background_keepalive_max_age, SessionState};
use crate::acp::types::{AcpEvent, BackgroundSettledInfo};
use crate::models::agent::AgentType;
use crate::models::message::MessageTurn;
use crate::parsers::claude::{
    capture_tag, find_session_file, group_into_turns, is_meta_message, slash_command_display,
    task_notification_status_regex, task_notification_summary_regex,
    task_notification_task_id_regex, ClaudeRecordAccumulator, CONTEXT_CONTINUATION_PREFIX,
};
use crate::web::event_bridge::{emit_with_state, EventEmitter};

/// Poll cadence while background work is outstanding or the transcript moved
/// recently — tight enough that a completed task surfaces within a beat.
const POLL_ACTIVE: Duration = Duration::from_secs(1);
/// Cadence when nothing is pending: cron//loop turns can still land at any
/// time, so the watch never stops — an unchanged file costs one `stat`.
const POLL_IDLE: Duration = Duration::from_secs(3);
/// How long after the last transcript growth the tight cadence is kept.
/// Sized to cover the agent's reaction to a settled task: after the
/// task-notification lands (last growth) the model may take 10–20s to write
/// its first reply block for a heavy synthesis (observed ~16s for a
/// two-agent digest) — dropping to the idle cadence inside that window adds
/// up to POLL_IDLE of avoidable surfacing latency. An unchanged file costs
/// one `stat` per tick, so the wider window is effectively free.
const RECENT_ACTIVITY_WINDOW: Duration = Duration::from_secs(30);
/// Prompt fingerprints older than this are dropped unconsumed — a rejected /
/// never-persisted prompt must not linger and swallow a later cron re-fire of
/// the same text.
const LEDGER_TTL: Duration = Duration::from_secs(600);
/// Max fingerprints kept (oldest evicted first). Far above any realistic
/// number of prompts in flight between transcript flushes.
const LEDGER_CAP: usize = 32;
/// Rotate the episode accumulator at the next out-of-turn boundary once it
/// holds this many messages, bounding per-tick regroup cost during very long
/// autonomous stretches. Already-emitted turns stay valid in the frontend
/// overlay; rotation only re-bases the id namespace for what follows.
const MAX_EPISODE_MESSAGES: usize = 512;
/// Absolute episode bound: a SINGLE autonomous turn can exceed
/// `MAX_EPISODE_MESSAGES` without ever hitting a boundary (a heavy /loop
/// iteration runs hundreds of tool calls in one turn), and every tick
/// clones + regroups + re-hashes the whole episode — unbounded, that's
/// O(n²) work over the turn. Past this valve the episode is force-rotated
/// mid-turn: the in-progress turn renders split across two overlay cards (a
/// visible seam, corrected by the next detail refetch) in exchange for a
/// hard cap on per-tick work. Double the boundary threshold so normal
/// boundary rotation always wins for multi-turn episodes.
const FORCE_ROTATE_MESSAGES: usize = MAX_EPISODE_MESSAGES * 2;

/// Fingerprints of prompts codeg itself sent on this connection, so the
/// watcher can tell wire-rendered foreground turns apart from out-of-turn
/// activity. Shared between the connection loop (writer, on every
/// `ConnectionCommand::Prompt`) and the watcher tick (consumer). A std mutex
/// is deliberate: both sides take it for microseconds and the watcher locks it
/// from a blocking context.
pub(crate) struct PromptLedger {
    entries: Mutex<VecDeque<LedgerEntry>>,
}

struct LedgerEntry {
    fingerprint: String,
    recorded_at: Instant,
}

impl PromptLedger {
    pub(crate) fn shared() -> Arc<Self> {
        Arc::new(Self {
            entries: Mutex::new(VecDeque::new()),
        })
    }

    /// Record the fingerprint of a prompt codeg is about to send: the first
    /// text block, trimmed. Attachment/resource blocks are excluded on
    /// purpose — the CLI may persist those differently, while the leading
    /// text lands verbatim at the start of the transcript's user record.
    pub(crate) fn record_prompt_blocks(&self, blocks: &[crate::acp::types::PromptInputBlock]) {
        let text = blocks.iter().find_map(|b| match b {
            crate::acp::types::PromptInputBlock::Text { text } => {
                let t = text.trim();
                (!t.is_empty()).then(|| t.to_string())
            }
            _ => None,
        });
        let Some(fingerprint) = text else {
            // A prompt with no text (image-only) can't be fingerprinted; its
            // turn will classify as out-of-turn and reconcile via refetch.
            tracing::debug!("[bg-watch] prompt without text block — no fingerprint recorded");
            return;
        };
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        entries.push_back(LedgerEntry {
            fingerprint,
            recorded_at: Instant::now(),
        });
        while entries.len() > LEDGER_CAP {
            entries.pop_front();
        }
    }

    /// Match `initiator_text` (the transcript turn's initiating user text)
    /// against the unconsumed fingerprints; on match the entry is consumed —
    /// exactly once per sent prompt, so a later same-text autonomous re-fire
    /// finds no entry and classifies as out-of-turn. The record may carry
    /// appended wrapper content after the sent text, hence prefix matching.
    fn consume_matching(&self, initiator_text: &str) -> bool {
        let text = initiator_text.trim();
        if text.is_empty() {
            return false;
        }
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        entries.retain(|e| e.recorded_at.elapsed() < LEDGER_TTL);
        if let Some(pos) = entries
            .iter()
            .position(|e| text == e.fingerprint || text.starts_with(e.fingerprint.as_str()))
        {
            entries.remove(pos);
            return true;
        }
        false
    }

    #[cfg(test)]
    fn record_text(&self, text: &str) {
        self.record_prompt_blocks(&[crate::acp::types::PromptInputBlock::Text {
            text: text.to_string(),
        }]);
    }
}

/// Aborts the watcher task when the owning conversation loop exits
/// (disconnect or fork restart — the restarted loop arms a fresh watcher).
pub(crate) struct BackgroundWatchGuard(tokio::task::JoinHandle<()>);

impl Drop for BackgroundWatchGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Arm the transcript watcher for a Claude connection; other agents have no
/// transcript-notification mechanism and get no watcher (returns `None`).
pub(crate) fn spawn_if_claude(
    conn_id: &str,
    agent_type: AgentType,
    state: Arc<RwLock<SessionState>>,
    emitter: EventEmitter,
    cwd: String,
    ledger: Arc<PromptLedger>,
) -> Option<BackgroundWatchGuard> {
    if agent_type != AgentType::ClaudeCode {
        return None;
    }
    let conn_id = conn_id.to_string();
    let handle = tokio::spawn(async move {
        run_watch(conn_id, state, emitter, cwd, ledger).await;
    });
    Some(BackgroundWatchGuard(handle))
}

async fn run_watch(
    conn_id: String,
    state: Arc<RwLock<SessionState>>,
    emitter: EventEmitter,
    cwd: String,
    ledger: Arc<PromptLedger>,
) {
    let mut ws = WatchState::new();
    // Wall-clock boundary between pre-existing transcript history (renders
    // via the detail fetch) and records that belong to THIS watch's lifetime.
    // Captured BEFORE the session is established: a NEW session's file is
    // created strictly after this instant, so every record in it — including
    // a first prompt or launch ack written before the watcher learns the
    // session id (SessionStarted can lag file creation by seconds) — is ours
    // to account, classify, and ledger-consume. A RESUMED session's history
    // predates this instant and is skipped. Baselining blindly at EOF on
    // first discovery used to drop that pre-discovery window: the ack never
    // registered (no keep-alive/chip) and the first prompt's ledger entry
    // lingered, able to swallow a later same-text cron refire.
    let spawn_epoch = std::time::SystemTime::now();
    let mut first_arm_done = false;
    loop {
        tokio::time::sleep(ws.poll_delay()).await;

        let (session_id, session_changed_at) = {
            let s = state.read().await;
            (s.external_id.clone(), s.external_id_changed_at)
        };
        let Some(session_id) = session_id else {
            continue; // session not established yet
        };
        if ws.session_id.as_deref() != Some(session_id.as_str()) {
            // New/resumed/forked session: re-locate and re-baseline. History
            // up to now renders via the normal detail fetch, not the overlay.
            // The first arm keeps the spawn epoch (see above). A LATER re-arm
            // (fork / re-resume) baselines at the instant the session id
            // actually CHANGED — not at this tick: the frontend fires its
            // follow-up prompt the moment the fork resolves, so records can
            // land in the forked transcript seconds before this poll notices;
            // a tick-time epoch would misclassify them as copied history (the
            // copy carries ORIGINAL, pre-fork timestamps — the changed-at
            // instant cleanly separates the two).
            let epoch = if first_arm_done {
                session_changed_at.unwrap_or_else(std::time::SystemTime::now)
            } else {
                spawn_epoch
            };
            first_arm_done = true;
            ws.rearm(session_id.clone(), epoch);
        }

        // File I/O + JSON parsing + turn grouping are blocking work; a large
        // tail after a long foreground turn must not stall the runtime.
        let ledger_ref = Arc::clone(&ledger);
        let cwd_for_tick = cwd.clone();
        let conn_for_tick = conn_id.clone();
        let joined = tokio::task::spawn_blocking(move || {
            let mut ws = ws;
            let event = ws.tick(&ledger_ref, &cwd_for_tick, &conn_for_tick);
            (ws, event)
        })
        .await;
        let event = match joined {
            Ok((returned, event)) => {
                ws = returned;
                event
            }
            Err(e) => {
                // Tick panicked (never expected — it is written to skip bad
                // input). Start over with a fresh baseline rather than killing
                // the watch for the rest of the connection's life.
                tracing::warn!("[bg-watch] tick panicked, re-arming: {e}");
                ws = WatchState::new();
                continue;
            }
        };

        if let Some(event) = event {
            if let AcpEvent::BackgroundActivity {
                turns,
                settled,
                outstanding,
                watermark,
                ..
            } = &event
            {
                tracing::info!(
                    "[bg-watch] surfacing connection={} turns={} settled={} outstanding={} watermark={}",
                    conn_id,
                    turns.len(),
                    settled.len(),
                    outstanding,
                    watermark
                );
            }
            emit_with_state(&state, &emitter, event).await;
        }
    }
}

/// One launched-but-unresolved background task.
struct TaskEntry {
    kind: &'static str,
    started_at: Instant,
}

/// The current out-of-turn episode: a contiguous run of transcript records
/// not belonging to any codeg-sent prompt turn, assembled into turns via the
/// detail parser's own Stage A/B.
struct Episode {
    /// Byte offset of the episode's initiating record — the stable base of
    /// this episode's overlay turn ids (`bg-<start_offset>-<idx>`).
    start_offset: u64,
    acc: ClaudeRecordAccumulator,
    /// turn id → content hash at last emission, for changed-turn upserts.
    emitted_hashes: HashMap<String, u64>,
}

enum Mode {
    /// Records belong to a codeg-sent prompt turn — the wire renders them.
    Foreground,
    /// Records are out-of-turn — the overlay renders them.
    Background,
}

pub(crate) struct WatchState {
    session_id: Option<String>,
    file: Option<PathBuf>,
    /// Bytes consumed through the last complete line — the emitted watermark.
    committed: u64,
    /// Bytes after `committed`: a trailing partial line awaiting its newline.
    carry: Vec<u8>,
    /// Last observed (mtime, len): the cheap "did anything change" gate.
    last_stat: Option<(Option<std::time::SystemTime>, u64)>,
    mode: Mode,
    episode: Option<Episode>,
    tasks: HashMap<String, TaskEntry>,
    /// Task ids that have settled at least once — a later `SendMessage` to
    /// such an id re-arms it (the resumed sub-agent will notify again).
    settled_ids: HashSet<String>,
    last_disk_activity: Option<Instant>,
    last_emitted_outstanding: Option<u32>,
    armed_logged: bool,
    /// Base of the most recently created episode's id namespace. Episode
    /// bases must be STRICTLY increasing: two episodes created while
    /// processing one tick's batch would otherwise share `committed` (it
    /// advances per batch, not per record) and collide their `bg-<base>-…`
    /// ids — the frontend upserts by id, so a collision conflates turns.
    last_episode_base: u64,
    /// Wall-clock boundary for the arm baseline: records at/after this
    /// instant belong to this watch's lifetime and are processed even when
    /// they were written before the transcript file was first discovered;
    /// records before it are pre-existing history. Set by `rearm`.
    epoch: Option<std::time::SystemTime>,
}

impl WatchState {
    pub(crate) fn new() -> Self {
        Self {
            session_id: None,
            file: None,
            committed: 0,
            carry: Vec::new(),
            last_stat: None,
            mode: Mode::Foreground,
            episode: None,
            tasks: HashMap::new(),
            settled_ids: HashSet::new(),
            last_disk_activity: None,
            // Some(0), not None: consumers assume zero until told otherwise,
            // so the first tick must not emit an accounting-only event for a
            // connection with no background work.
            last_emitted_outstanding: Some(0),
            armed_logged: false,
            last_episode_base: 0,
            epoch: None,
        }
    }

    /// Allocate the id-namespace base for a new episode: the current byte
    /// offset, tie-broken upward so consecutive episodes never collide even
    /// when created within a single tick's batch.
    fn next_episode_base(&mut self) -> u64 {
        let base = self.committed.max(self.last_episode_base + 1);
        self.last_episode_base = base;
        base
    }

    fn rearm(&mut self, session_id: String, epoch: std::time::SystemTime) {
        let tasks = std::mem::take(&mut self.tasks);
        let settled_ids = std::mem::take(&mut self.settled_ids);
        *self = Self::new();
        // Keep the accounting across a fork/resume of the same CLI process —
        // the background work is still running in it; the max-age valve and
        // future notifications (same task-id) still resolve these entries.
        // `settled_ids` must survive too: a post-fork `SendMessage(to: <id>)`
        // resumes a sub-agent that settled BEFORE the fork, and without the
        // set the re-arm is missed — outstanding stays 0 and closing the tab
        // could kill the resumed work.
        self.tasks = tasks;
        self.settled_ids = settled_ids;
        self.session_id = Some(session_id);
        self.epoch = Some(epoch);
    }

    fn poll_delay(&self) -> Duration {
        let recently_active = self
            .last_disk_activity
            .is_some_and(|at| at.elapsed() < RECENT_ACTIVITY_WINDOW);
        if !self.tasks.is_empty() || recently_active {
            POLL_ACTIVE
        } else {
            POLL_IDLE
        }
    }

    /// One poll tick: stat-gate, tail-read complete lines, account + classify
    /// each record, regroup the episode, and decide what (if anything) to
    /// emit. Never panics on malformed input — bad lines are skipped.
    pub(crate) fn tick(
        &mut self,
        ledger: &PromptLedger,
        cwd: &str,
        conn_id: &str,
    ) -> Option<AcpEvent> {
        let session_id = self.session_id.clone()?;

        // Expire tasks past the keep-alive max age so a lost completion can't
        // pin the connection alive forever; the emitted outstanding drop also
        // releases the frontend's sweep exemption mirror.
        let max_age = background_keepalive_max_age()
            .to_std()
            .unwrap_or(Duration::from_secs(3600));
        let before = self.tasks.len();
        self.tasks.retain(|id, t| {
            let keep = t.started_at.elapsed() < max_age;
            if !keep {
                tracing::info!(
                    "[bg-watch] expiring {} task={id} after max-age (completion never observed)",
                    t.kind
                );
            }
            keep
        });
        let expired_any = self.tasks.len() != before;

        // Locate the transcript (it may not exist yet for a brand-new
        // session; retry every tick until it does).
        if self.file.is_none() {
            if let Some(f) = find_session_file(&session_id) {
                self.adopt_file(f);
            }
            if let Some(f) = &self.file {
                if !self.armed_logged {
                    self.armed_logged = true;
                    tracing::info!(
                        "[bg-watch] armed connection={} session={} baseline={} file={}",
                        conn_id,
                        session_id,
                        self.committed,
                        f.display()
                    );
                }
            }
        }
        let path = self.file.clone()?;

        // Cheap gate: unchanged (mtime, len) and no pending partial line means
        // nothing to read this tick.
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("[bg-watch] stat failed for {}: {e}", path.display());
                self.file = None; // session file may have moved; re-locate
                return None;
            }
        };
        let stat = (meta.modified().ok(), meta.len());
        let unchanged = self.last_stat.as_ref() == Some(&stat);
        self.last_stat = Some(stat);

        let mut changed_turns: Vec<MessageTurn> = Vec::new();
        let mut settled: Vec<BackgroundSettledInfo> = Vec::new();

        if meta.len() < self.committed {
            // Truncated/rewritten out from under us: re-baseline at EOF. The
            // frontend overlay reconciles on its next detail refetch.
            tracing::warn!(
                "[bg-watch] transcript shrank ({} -> {}), re-baselining",
                self.committed,
                meta.len()
            );
            self.committed = meta.len();
            self.carry.clear();
            self.episode = None;
            self.mode = Mode::Foreground;
        } else if !unchanged {
            match self.read_new_lines(&path) {
                Ok(lines) => {
                    if !lines.is_empty() {
                        self.last_disk_activity = Some(Instant::now());
                    }
                    for line in &lines {
                        let value: serde_json::Value = match serde_json::from_str(line) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        self.account(&value, &mut settled);
                        self.classify_and_feed(&value, ledger, cwd, &mut changed_turns);
                    }
                    if !lines.is_empty() {
                        // Regroup once per tick (not per record): collect the
                        // episode's turns whose content changed since the last
                        // emission.
                        self.collect_changed_turns(cwd, &mut changed_turns);
                    }
                }
                Err(e) => {
                    tracing::warn!("[bg-watch] read failed for {}: {e}", path.display());
                    return None;
                }
            }
        }

        let outstanding = self.tasks.len() as u32;
        let accounting_changed =
            expired_any || self.last_emitted_outstanding != Some(outstanding);
        if changed_turns.is_empty() && settled.is_empty() && !accounting_changed {
            return None;
        }
        self.last_emitted_outstanding = Some(outstanding);
        Some(AcpEvent::BackgroundActivity {
            session_id,
            turns: changed_turns,
            outstanding,
            settled,
            watermark: self.committed,
        })
    }

    /// Adopt a just-discovered transcript file, choosing the arm baseline.
    /// History (records before `epoch`) renders via the detail fetch and is
    /// skipped; records at/after `epoch` — a first prompt or launch ack
    /// written between session creation and this discovery — are processed
    /// like any other appended record. Without an epoch (never armed via
    /// `rearm`), falls back to EOF, the pure-history behavior.
    fn adopt_file(&mut self, f: PathBuf) {
        if let Ok(meta) = std::fs::metadata(&f) {
            // `baseline_offset_since` never lands inside a trailing partial
            // line; the EOF fallback covers only an unreadable file (where
            // the tail reader will re-locate anyway).
            self.committed = self
                .epoch
                .and_then(|e| baseline_offset_since(&f, e))
                .unwrap_or(meta.len());
            // Deliberately do NOT pre-seed `last_stat`: the stat gate below
            // must see this tick as changed so a baseline that landed BEFORE
            // EOF (pre-discovery records to process) is read immediately, not
            // on the next unrelated append.
        }
        self.file = Some(f);
    }

    /// Read bytes appended since `committed`, returning COMPLETE lines only;
    /// a trailing partial line stays in `carry` until its newline arrives.
    fn read_new_lines(&mut self, path: &PathBuf) -> std::io::Result<Vec<String>> {
        let mut f = std::fs::File::open(path)?;
        f.seek(SeekFrom::Start(self.committed + self.carry.len() as u64))?;
        let mut fresh = Vec::new();
        f.read_to_end(&mut fresh)?;
        if fresh.is_empty() {
            return Ok(Vec::new());
        }
        self.carry.extend_from_slice(&fresh);

        let mut lines = Vec::new();
        while let Some(nl) = self.carry.iter().position(|b| *b == b'\n') {
            let rest = self.carry.split_off(nl + 1);
            let mut line_bytes = std::mem::replace(&mut self.carry, rest);
            line_bytes.pop(); // the '\n'
            self.committed += nl as u64 + 1;
            // Mirror the detail parser: a non-UTF-8 line is skipped, but its
            // bytes still count toward the watermark.
            if let Ok(line) = String::from_utf8(line_bytes) {
                lines.push(line);
            }
        }
        Ok(lines)
    }

    /// Task accounting for one record: launch acks, `<task-notification>`
    /// settlements, and `SendMessage` re-arms of settled sub-agents.
    fn account(&mut self, value: &serde_json::Value, settled: &mut Vec<BackgroundSettledInfo>) {
        let record_type = value.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match record_type {
            "user" => {
                if let Some(tur) = value.get("toolUseResult") {
                    if tur.get("status").and_then(|s| s.as_str()) == Some("async_launched") {
                        if let Some(id) = tur
                            .get("agentId")
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.is_empty())
                        {
                            tracing::info!("[bg-watch] registered async agent task={id}");
                            self.tasks.insert(
                                id.to_string(),
                                TaskEntry {
                                    kind: "agent",
                                    started_at: Instant::now(),
                                },
                            );
                        }
                    } else if let Some(id) = tur
                        .get("backgroundTaskId")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                    {
                        tracing::info!("[bg-watch] registered background shell task={id}");
                        self.tasks.insert(
                            id.to_string(),
                            TaskEntry {
                                kind: "shell",
                                started_at: Instant::now(),
                            },
                        );
                    }
                }
                if let Some(text) = user_record_text(value) {
                    let trimmed = text.trim_start();
                    if trimmed.starts_with("<task-notification>") {
                        let task_id = capture_tag(task_notification_task_id_regex(), trimmed);
                        let status = capture_tag(task_notification_status_regex(), trimmed)
                            .unwrap_or_else(|| "completed".into());
                        let summary = capture_tag(task_notification_summary_regex(), trimmed);
                        if let Some(id) = task_id {
                            let known = self.tasks.remove(&id).is_some();
                            self.settled_ids.insert(id.clone());
                            tracing::info!(
                                "[bg-watch] settled task={id} status={status} known={known}"
                            );
                            settled.push(BackgroundSettledInfo {
                                task_id: id,
                                status,
                                summary,
                            });
                        }
                    }
                }
            }
            "assistant" => {
                // A settled sub-agent can be resumed: `SendMessage(to: <id>)`
                // re-arms its accounting entry (it will notify again — the
                // notification's own <note> documents multi-notify).
                let Some(blocks) = value
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                else {
                    return;
                };
                for block in blocks {
                    if block.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                        continue;
                    }
                    if block.get("name").and_then(|n| n.as_str()) != Some("SendMessage") {
                        continue;
                    }
                    let Some(to) = block
                        .get("input")
                        .and_then(|i| i.get("to"))
                        .and_then(|t| t.as_str())
                    else {
                        continue;
                    };
                    if self.settled_ids.remove(to) {
                        tracing::info!("[bg-watch] re-armed resumed task={to}");
                        self.tasks.insert(
                            to.to_string(),
                            TaskEntry {
                                kind: "agent",
                                started_at: Instant::now(),
                            },
                        );
                    }
                }
            }
            _ => {}
        }
    }

    /// Classify a record against the prompt ledger and feed out-of-turn ones
    /// into the current episode.
    fn classify_and_feed(
        &mut self,
        value: &serde_json::Value,
        ledger: &PromptLedger,
        cwd: &str,
        changed_turns: &mut Vec<MessageTurn>,
    ) {
        if let Some(initiator_text) = turn_initiator_text(value) {
            if ledger.consume_matching(&initiator_text) {
                // A codeg-sent prompt: the wire renders this turn. Close any
                // open episode first (flush its final state) and go silent.
                tracing::debug!("[bg-watch] foreground turn matched ledger");
                self.collect_changed_turns(cwd, changed_turns);
                self.episode = None;
                self.mode = Mode::Foreground;
                return;
            }
            tracing::debug!(
                "[bg-watch] out-of-turn initiator: {:?}",
                initiator_text.chars().take(60).collect::<String>()
            );
            let rotate = self
                .episode
                .as_ref()
                .is_some_and(|e| e.acc.messages.len() >= MAX_EPISODE_MESSAGES);
            if matches!(self.mode, Mode::Foreground) || self.episode.is_none() || rotate {
                if rotate {
                    self.collect_changed_turns(cwd, changed_turns);
                }
                // Any stable, strictly-increasing base works — turn ids only
                // need to be unique and stable within the watch.
                self.episode = Some(Episode {
                    start_offset: self.next_episode_base(),
                    acc: ClaudeRecordAccumulator::new(
                        self.file.clone().unwrap_or_else(|| PathBuf::from("")),
                    ),
                    emitted_hashes: HashMap::new(),
                });
            }
            self.mode = Mode::Background;
        }

        if matches!(self.mode, Mode::Background) {
            // Mid-turn safety valve: one giant turn with no boundary would
            // otherwise grow the episode — and the per-tick regroup over it —
            // without bound. Flush and re-base; the seam is cosmetic and the
            // next detail refetch renders the turn whole.
            let force_rotate = self
                .episode
                .as_ref()
                .is_some_and(|e| e.acc.messages.len() >= FORCE_ROTATE_MESSAGES);
            if force_rotate {
                tracing::warn!(
                    "[bg-watch] episode reached {FORCE_ROTATE_MESSAGES} messages without a turn \
                     boundary — force-rotating (the in-progress turn renders split until the \
                     next detail refetch)"
                );
                self.collect_changed_turns(cwd, changed_turns);
                self.episode = Some(Episode {
                    start_offset: self.next_episode_base(),
                    acc: ClaudeRecordAccumulator::new(
                        self.file.clone().unwrap_or_else(|| PathBuf::from("")),
                    ),
                    emitted_hashes: HashMap::new(),
                });
            }
            if let Some(episode) = self.episode.as_mut() {
                episode.acc.feed_value(value.clone());
            }
        }
    }

    /// Regroup the open episode with the detail parser's Stage B + post-
    /// processing and append turns whose content changed since last emission.
    fn collect_changed_turns(&mut self, cwd: &str, out: &mut Vec<MessageTurn>) {
        let Some(episode) = self.episode.as_mut() else {
            return;
        };
        if episode.acc.messages.is_empty() {
            return;
        }
        let mut messages = episode.acc.messages.clone();
        // An autonomous turn can itself launch background work; fold any
        // ack+notification pairs seen within this episode, same as the
        // detail parse does.
        episode.acc.apply_background_lifecycle(&mut messages);
        let mut turns = group_into_turns(messages);
        crate::parsers::relocate_orphaned_tool_results(&mut turns);
        crate::parsers::structurize_read_tool_output(&mut turns);
        crate::parsers::resolve_patch_line_numbers(&mut turns, Some(cwd));
        for (idx, mut turn) in turns.into_iter().enumerate() {
            turn.id = format!("bg-{}-{}", episode.start_offset, idx);
            let hash = hash_turn(&turn);
            if episode.emitted_hashes.get(&turn.id) == Some(&hash) {
                continue;
            }
            episode.emitted_hashes.insert(turn.id.clone(), hash);
            out.push(turn);
        }
    }

    #[cfg(test)]
    fn with_file_for_test(session_id: &str, file: PathBuf) -> Self {
        let mut ws = Self::new();
        ws.session_id = Some(session_id.to_string());
        ws.file = Some(file);
        ws
    }
}

/// Extract the text of a user record's content: bare string form, or the
/// concatenated text blocks of the array form. `None` for non-user records.
fn user_record_text(value: &serde_json::Value) -> Option<String> {
    if value.get("type").and_then(|t| t.as_str()) != Some("user") {
        return None;
    }
    let content = value.get("message")?.get("content")?;
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    let arr = content.as_array()?;
    let texts: Vec<&str> = arr
        .iter()
        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
        .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
        .collect();
    if texts.is_empty() {
        return None;
    }
    Some(texts.join("\n"))
}

/// The text of a record that STARTS a new logical turn, or `None` for records
/// that continue the current one. Ground rules (from real transcripts):
///
/// * tool-result-only user records continue the in-progress turn;
/// * `isMeta` ARRAY records are slash-command expansions — part of the
///   foreground turn that issued the command (a cron prompt is `isMeta` too,
///   but always STRING content, so it still initiates);
/// * auto-compaction continuation summaries land MID-turn while the wire is
///   still rendering it — never a boundary;
/// * everything else user-typed/injected (real prompts, `<task-notification>`
///   records, cron prompts) initiates.
fn turn_initiator_text(value: &serde_json::Value) -> Option<String> {
    if value.get("type").and_then(|t| t.as_str()) != Some("user") {
        return None;
    }
    let content = value.get("message")?.get("content")?;

    if let Some(s) = content.as_str() {
        if s.starts_with(CONTEXT_CONTINUATION_PREFIX) {
            return None;
        }
        // A slash command persists as command tags; codeg sent the display
        // form ("/name args"), so match the ledger against that.
        if let Some(display) = slash_command_display(s) {
            return Some(display);
        }
        return Some(s.to_string());
    }

    let arr = content.as_array()?;
    if !arr.is_empty()
        && arr
            .iter()
            .all(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
    {
        return None;
    }
    if is_meta_message(value) {
        return None;
    }
    let text = user_record_text(value)?;
    if text.starts_with(CONTEXT_CONTINUATION_PREFIX) {
        return None;
    }
    Some(text)
}

/// The arm baseline separating pre-existing history from records written
/// during this watch's lifetime: the byte offset of the first COMPLETE
/// CONVERSATION line (a `user`/`assistant` record) whose timestamp is at or
/// after `epoch`, or — when no such record exists yet — the end of the last
/// COMPLETE line. The fallback deliberately excludes a trailing partial line:
/// a fragment present at arm time is a record being flushed RIGHT NOW
/// (post-epoch by definition), and baselining past it (EOF) would leave only
/// its unparseable suffix for the tail reader, silently dropping the very
/// record the epoch baseline exists to preserve.
///
/// Only `user`/`assistant` records delimit the boundary; every other record
/// type (and any line without a parseable timestamp) is skipped. This is
/// load-bearing for FORK: a fork copies the parent transcript into the new
/// session file preserving each record's ORIGINAL, pre-fork timestamp, then
/// writes fresh metadata records (`queue-operation`, `mode`, …) at the FILE
/// HEAD stamped at fork time. Those head records are AHEAD of the copied
/// history by byte offset but AFTER it by timestamp, so keying the boundary on
/// "first record at/after epoch" of ANY type would return the head metadata's
/// offset and drag the entire copied history (which renders via the detail
/// fetch) into the out-of-turn overlay — duplicating it. Restricting the
/// boundary to conversation records lands it on the first genuinely-new turn,
/// with the copied history (older timestamps) correctly on the history side.
/// The skipped metadata carries no turn or accounting the watcher consumes.
/// `None` only when the file can't be read. One-shot cost at arm time (runs
/// inside the tick's `spawn_blocking`).
fn baseline_offset_since(path: &PathBuf, epoch: std::time::SystemTime) -> Option<u64> {
    let bytes = std::fs::read(path).ok()?;
    let epoch: chrono::DateTime<chrono::Utc> = epoch.into();
    let mut offset = 0u64;
    for line in bytes.split_inclusive(|b| *b == b'\n') {
        if line.last() != Some(&b'\n') {
            break;
        }
        let start = offset;
        offset += line.len() as u64;
        let Ok(text) = std::str::from_utf8(line) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
            continue;
        };
        // Only real conversation records anchor the boundary; fork-time head
        // metadata (see the doc comment) must never be the boundary.
        let record_type = value.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if record_type != "user" && record_type != "assistant" {
            continue;
        }
        let Some(ts) = value.get("timestamp").and_then(|t| t.as_str()) else {
            continue;
        };
        let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(ts) else {
            continue;
        };
        if parsed.with_timezone(&chrono::Utc) >= epoch {
            return Some(start);
        }
    }
    Some(offset)
}

fn hash_turn(turn: &MessageTurn) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    match serde_json::to_string(turn) {
        Ok(s) => s.hash(&mut hasher),
        Err(_) => turn.blocks.len().hash(&mut hasher),
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_session(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("session-1.jsonl")
    }

    fn write_lines(path: &PathBuf, lines: &[&str]) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
    }

    /// Append raw bytes WITHOUT a newline — simulates a mid-flush fragment.
    fn append_raw(path: &PathBuf, chunk: &str) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        write!(f, "{chunk}").unwrap();
    }

    /// Real-shape async sub-agent launch ack (structured `toolUseResult`
    /// sibling as captured from a live transcript on 2026-07-07).
    fn agent_ack(agent_id: &str) -> String {
        format!(
            r#"{{"type":"user","timestamp":"2026-07-07T03:46:14.514Z","uuid":"u-ack-{agent_id}","message":{{"role":"user","content":[{{"tool_use_id":"toolu_01","type":"tool_result","content":[{{"type":"text","text":"Async agent launched successfully. agentId: {agent_id}"}}]}}]}},"toolUseResult":{{"isAsync":true,"status":"async_launched","agentId":"{agent_id}","description":"Run pnpm build"}}}}"#
        )
    }

    /// Real-shape background shell ack (`toolUseResult.backgroundTaskId`).
    fn bash_ack(task_id: &str) -> String {
        format!(
            r#"{{"type":"user","timestamp":"2026-07-07T03:46:15.000Z","uuid":"u-bash-{task_id}","message":{{"role":"user","content":[{{"tool_use_id":"toolu_02","type":"tool_result","content":"Command running in background with ID: {task_id}."}}]}},"toolUseResult":{{"stdout":"","stderr":"","interrupted":false,"backgroundTaskId":"{task_id}"}}}}"#
        )
    }

    /// Real-shape `<task-notification>` completion record (string content).
    fn notification(task_id: &str, status: &str) -> String {
        let inner = format!(
            "<task-notification>\\n<task-id>{task_id}</task-id>\\n<tool-use-id>toolu_01</tool-use-id>\\n<status>{status}</status>\\n<summary>Agent \\\"Run pnpm build\\\" finished</summary>\\n<result>Build OK</result>\\n</task-notification>"
        );
        format!(
            r#"{{"type":"user","timestamp":"2026-07-07T03:47:00.000Z","uuid":"u-note-{task_id}","isSidechain":false,"message":{{"role":"user","content":"{inner}"}}}}"#
        )
    }

    fn assistant_text(uuid: &str, text: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"2026-07-07T03:47:08.000Z","uuid":"{uuid}","message":{{"role":"assistant","model":"claude-sonnet-5","content":[{{"type":"text","text":"{text}"}}]}}}}"#
        )
    }

    fn user_prompt_array(uuid: &str, text: &str) -> String {
        format!(
            r#"{{"type":"user","timestamp":"2026-07-07T03:48:00.000Z","uuid":"{uuid}","message":{{"role":"user","content":[{{"type":"text","text":"{text}"}}]}}}}"#
        )
    }

    /// Real-shape cron-fired prompt: `isMeta:true` with bare STRING content.
    fn cron_prompt(text: &str) -> String {
        format!(
            r#"{{"type":"user","timestamp":"2026-07-07T03:49:00.000Z","uuid":"u-cron","isMeta":true,"userType":"external","message":{{"role":"user","content":"{text}"}}}}"#
        )
    }

    fn tick_now(ws: &mut WatchState, ledger: &PromptLedger) -> Option<AcpEvent> {
        ws.tick(ledger, "/tmp", "conn-test")
    }

    fn unpack(
        event: AcpEvent,
    ) -> (Vec<MessageTurn>, u32, Vec<BackgroundSettledInfo>, u64) {
        match event {
            AcpEvent::BackgroundActivity {
                turns,
                outstanding,
                settled,
                watermark,
                ..
            } => (turns, outstanding, settled, watermark),
            other => panic!("expected BackgroundActivity, got {other:?}"),
        }
    }

    #[test]
    fn force_rotates_a_single_giant_turn_and_bounds_the_episode() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());

        // One out-of-turn initiator, then one giant turn: FORCE + 3 assistant
        // records with NO further boundary. Written and read in a single tick
        // batch to also exercise the same-batch episode-base tie-break.
        let mut lines: Vec<String> = vec![cron_prompt("iterate forever")];
        for i in 0..(FORCE_ROTATE_MESSAGES + 3) {
            lines.push(assistant_text(&format!("a-{i}"), "chunk"));
        }
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        write_lines(&path, &refs);

        let (turns, ..) = unpack(tick_now(&mut ws, &ledger).expect("turns event"));

        // The episode was re-based mid-turn: what remains buffered is only the
        // post-rotation fragment, never the whole giant turn.
        let buffered = ws.episode.as_ref().map(|e| e.acc.messages.len()).unwrap();
        assert!(
            buffered < FORCE_ROTATE_MESSAGES,
            "episode must be bounded after force-rotation, still holds {buffered}"
        );
        // Both namespaces surfaced this tick, under distinct (non-colliding)
        // bases even though both episodes were created within one batch.
        let bases: std::collections::HashSet<&str> = turns
            .iter()
            .map(|t| t.id.rsplit_once('-').expect("bg id shape").0)
            .collect();
        assert!(
            bases.len() >= 2,
            "expected pre- and post-rotation id namespaces, got {bases:?}"
        );
        assert_eq!(
            turns.len(),
            turns
                .iter()
                .map(|t| t.id.as_str())
                .collect::<std::collections::HashSet<_>>()
                .len(),
            "turn ids must be unique across the forced rotation"
        );
    }

    fn epoch(ts: &str) -> std::time::SystemTime {
        chrono::DateTime::parse_from_rfc3339(ts)
            .unwrap()
            .with_timezone(&chrono::Utc)
            .into()
    }

    #[test]
    fn baseline_offset_since_finds_first_record_at_or_after_epoch() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        let first = agent_ack("agent1"); // timestamp 03:46:14.514Z
        write_lines(&path, &[&first, &notification("agent1", "completed")]); // 03:47:00Z

        // Epoch between the two records: boundary at the second line.
        assert_eq!(
            baseline_offset_since(&path, epoch("2026-07-07T03:46:30.000Z")),
            Some(first.len() as u64 + 1)
        );
        // Epoch before everything: whole file is ours.
        assert_eq!(
            baseline_offset_since(&path, epoch("2020-01-01T00:00:00Z")),
            Some(0)
        );
        // Epoch after everything: pure history — baseline after the last
        // COMPLETE line (== EOF here, the file ends with a newline).
        let full = std::fs::metadata(&path).unwrap().len();
        assert_eq!(
            baseline_offset_since(&path, epoch("2030-01-01T00:00:00Z")),
            Some(full)
        );
        // A trailing partial flush is a record being written NOW: the
        // fallback baseline must sit BEFORE it so it reconstructs.
        append_raw(&path, r#"{"type":"user","half"#);
        assert_eq!(
            baseline_offset_since(&path, epoch("2030-01-01T00:00:00Z")),
            Some(full)
        );
    }

    /// FORK layout regression. A fork copies the parent transcript (records keep
    /// their ORIGINAL, pre-fork timestamps) and writes fresh `queue-operation`
    /// metadata at the FILE HEAD stamped at fork time. That head record is
    /// post-epoch by timestamp but sits BEFORE the copied history by byte
    /// offset, so a boundary keyed on "first record at/after epoch" of ANY type
    /// would land at offset 0 and pull the entire copied history into the
    /// out-of-turn overlay — duplicating what the detail fetch already renders.
    /// The boundary must skip non-conversation records and land on the first
    /// genuinely-new turn.
    #[test]
    fn baseline_skips_fork_head_metadata_and_lands_on_the_first_new_turn() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        // Fork-time metadata at the head (post-epoch stamp), then the copied
        // history (original pre-fork stamps), then the genuinely-new prompt.
        let queue_op = r#"{"type":"queue-operation","timestamp":"2026-07-07T03:50:00.100Z","uuid":"q-1"}"#;
        let copied_user = r#"{"type":"user","timestamp":"2026-07-07T03:46:00.000Z","uuid":"u-hi","message":{"role":"user","content":[{"type":"text","text":"hi"}]}}"#;
        let copied_asst = r#"{"type":"assistant","timestamp":"2026-07-07T03:46:05.000Z","uuid":"a-hi","message":{"role":"assistant","content":[{"type":"text","text":"Hi!"}]}}"#;
        let new_user = r#"{"type":"user","timestamp":"2026-07-07T03:50:10.000Z","uuid":"u-hello","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}"#;
        write_lines(&path, &[queue_op, copied_user, copied_asst, new_user]);

        // Fork epoch sits after the copied history but before the new turn. The
        // boundary must be the new turn's byte offset (past the head metadata
        // AND the copied history), never offset 0.
        let new_turn_offset = (queue_op.len() + 1) as u64
            + (copied_user.len() + 1) as u64
            + (copied_asst.len() + 1) as u64;
        assert_eq!(
            baseline_offset_since(&path, epoch("2026-07-07T03:50:00.000Z")),
            Some(new_turn_offset),
            "fork-time head metadata must not drag the copied history past the baseline"
        );
    }

    /// A new session's file can be discovered mid-flush of its FIRST record:
    /// no complete post-epoch line exists yet, and an EOF fallback would
    /// baseline past the fragment — its completing suffix then reads as
    /// unparseable garbage and the record (a launch ack here) is lost.
    #[test]
    fn adopt_with_trailing_partial_line_keeps_it_ahead_of_the_baseline() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        let ledger = PromptLedger::shared();
        // Complete pre-epoch history line, then half of an ack record.
        write_lines(&path, &[&notification("older", "completed")]);
        let ack = agent_ack("agentY");
        let (head, tail) = ack.split_at(ack.len() / 2);
        append_raw(&path, head);

        let mut ws = WatchState::new();
        ws.session_id = Some("s1".into());
        ws.epoch = Some(epoch("2030-01-01T00:00:00Z"));
        ws.adopt_file(path.clone());

        // First tick buffers the fragment (no complete line — no event).
        assert!(tick_now(&mut ws, &ledger).is_none());

        // The flush completes: the reconstructed ack must account.
        append_raw(&path, tail);
        append_raw(&path, "\n");
        let (_, outstanding, ..) =
            unpack(tick_now(&mut ws, &ledger).expect("ack event"));
        assert_eq!(outstanding, 1, "mid-flush ack must survive discovery");
    }

    /// Production fork timing: the frontend fires its follow-up prompt the
    /// moment the fork resolves, so post-fork records land in the forked
    /// transcript BEFORE the polling watcher's next tick notices the session
    /// change. The re-arm epoch is therefore the instant the session id
    /// CHANGED (stamped by SessionStarted in session state), not the tick
    /// time — records written in that gap must still process, while the
    /// fork-copied history (original, pre-fork timestamps) stays skipped.
    #[test]
    fn post_fork_records_written_before_the_watcher_notices_still_process() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[&agent_ack("agentX")]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());
        let _ = tick_now(&mut ws, &ledger);
        write_lines(&path, &[&notification("agentX", "completed")]);
        let _ = tick_now(&mut ws, &ledger);

        // Fork at 03:50 (all copied history predates it), and the resume
        // record (timestamp 03:52) lands BEFORE the watcher re-arms.
        let forked = dir.path().join("session-2.jsonl");
        std::fs::copy(&path, &forked).unwrap();
        let send = r#"{"type":"assistant","timestamp":"2026-07-07T03:52:00.000Z","uuid":"a-send3","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_05","name":"SendMessage","input":{"to":"agentX","summary":"continue","message":"go on"}}]}}"#;
        write_lines(&forked, &[send]);

        // The watcher's delayed tick finally observes the fork: epoch is the
        // session-change instant, so the already-written resume record is
        // ahead of the baseline and re-arms the accounting.
        ws.rearm("s2".into(), epoch("2026-07-07T03:50:00.000Z"));
        ws.adopt_file(forked.clone());
        let (_, outstanding, ..) =
            unpack(tick_now(&mut ws, &ledger).expect("resume event"));
        assert_eq!(
            outstanding, 1,
            "a resume written before the watcher noticed the fork must re-arm"
        );
    }

    /// `settled_ids` must survive a fork//re-resume re-arm: a post-fork
    /// `SendMessage(to: <id>)` resumes a sub-agent that settled BEFORE the
    /// fork, and missing the re-arm leaves outstanding at 0 — closing the
    /// tab could then kill the resumed background work.
    #[test]
    fn settled_ids_survive_rearm_so_post_fork_resume_rearms() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[&agent_ack("agentX")]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());
        let _ = tick_now(&mut ws, &ledger);
        write_lines(&path, &[&notification("agentX", "completed")]);
        let (_, outstanding, ..) = unpack(tick_now(&mut ws, &ledger).unwrap());
        assert_eq!(outstanding, 0);

        // Fork: new session id, new transcript file (history copied with
        // ORIGINAL timestamps — all before the re-arm epoch).
        let forked = dir.path().join("session-2.jsonl");
        std::fs::copy(&path, &forked).unwrap();
        ws.rearm("s2".into(), epoch("2030-01-01T00:00:00Z"));
        ws.adopt_file(forked.clone());
        assert!(
            tick_now(&mut ws, &ledger).is_none(),
            "copied history must not re-account"
        );

        let send = r#"{"type":"assistant","timestamp":"2026-07-07T03:52:00.000Z","uuid":"a-send2","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_04","name":"SendMessage","input":{"to":"agentX","summary":"continue","message":"go on"}}]}}"#;
        write_lines(&forked, &[send]);
        let (_, outstanding, ..) = unpack(tick_now(&mut ws, &ledger).unwrap());
        assert_eq!(outstanding, 1, "post-fork resume must re-arm");

        write_lines(&forked, &[&notification("agentX", "completed")]);
        let (_, outstanding, settled, _) =
            unpack(tick_now(&mut ws, &ledger).unwrap());
        assert_eq!(outstanding, 0);
        assert_eq!(settled.len(), 1);
    }

    /// The Critical arm-gap regression: a brand-new session's file (and its
    /// first prompt + launch ack) can exist BEFORE the watcher's first
    /// successful discovery — SessionStarted lags file creation by seconds.
    /// Those records must still be accounted and ledger-consumed; blindly
    /// baselining at EOF dropped them (outstanding never armed, and the
    /// unconsumed fingerprint could swallow a later same-text cron refire).
    #[test]
    fn pre_discovery_records_are_accounted_and_ledger_consumed() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        let ledger = PromptLedger::shared();
        ledger.record_text("do the thing");

        // On disk before discovery: codeg's first prompt, the reply, an ack.
        write_lines(
            &path,
            &[
                &user_prompt_array("u1", "do the thing"),
                &assistant_text("a1", "launching"),
                &agent_ack("agentX"),
            ],
        );

        let mut ws = WatchState::new();
        ws.session_id = Some("s1".into());
        // Spawn predates session creation for a new session.
        ws.epoch = Some(epoch("2020-01-01T00:00:00Z"));
        ws.adopt_file(path.clone());

        let (turns, outstanding, settled, _) =
            unpack(tick_now(&mut ws, &ledger).expect("accounting event"));
        assert_eq!(outstanding, 1, "pre-discovery ack must register");
        assert!(settled.is_empty());
        assert!(
            turns.is_empty(),
            "the codeg-sent prompt classifies foreground — the wire renders it"
        );

        // Its fingerprint was consumed, so a same-text out-of-turn refire
        // (cron//loop) classifies as background and surfaces.
        write_lines(
            &path,
            &[&cron_prompt("do the thing"), &assistant_text("a2", "pass")],
        );
        let (turns, ..) = unpack(tick_now(&mut ws, &ledger).expect("turns event"));
        assert!(
            !turns.is_empty(),
            "same-text refire must surface — a stale ledger entry would swallow it"
        );
    }

    #[test]
    fn resume_baselines_at_eof_and_skips_historical_acks() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        let ledger = PromptLedger::shared();
        // Historical, never-settled ack from a previous run of this session.
        write_lines(&path, &[&agent_ack("stale-old")]);

        let mut ws = WatchState::new();
        ws.session_id = Some("s1".into());
        // Resume: the watch armed long after that history was written.
        ws.epoch = Some(epoch("2030-01-01T00:00:00Z"));
        ws.adopt_file(path.clone());

        assert!(
            tick_now(&mut ws, &ledger).is_none(),
            "pure history yields no event"
        );

        // Only appended records are processed; the stale ack never registers.
        write_lines(
            &path,
            &[&cron_prompt("new pass"), &assistant_text("a9", "hi")],
        );
        let (turns, outstanding, ..) =
            unpack(tick_now(&mut ws, &ledger).expect("turns event"));
        assert_eq!(outstanding, 0, "historical ack must NOT register");
        assert_eq!(turns.len(), 1);
    }

    #[test]
    fn acks_register_outstanding_without_rendering_turns() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());

        write_lines(&path, &[&agent_ack("agent1"), &bash_ack("bash1")]);
        let (turns, outstanding, settled, watermark) =
            unpack(tick_now(&mut ws, &ledger).expect("accounting event"));
        assert!(turns.is_empty(), "acks are tool-result records, not turns");
        assert_eq!(outstanding, 2);
        assert!(settled.is_empty());
        assert!(watermark > 0);

        // Unchanged file → stat-gated, no event.
        assert!(tick_now(&mut ws, &ledger).is_none());
    }

    #[test]
    fn notification_settles_and_surfaces_the_response_turn() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[&agent_ack("agent1")]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());
        let _ = tick_now(&mut ws, &ledger); // consume the ack

        write_lines(
            &path,
            &[
                &notification("agent1", "completed"),
                &assistant_text("a1", "Build finished cleanly."),
            ],
        );
        let (turns, outstanding, settled, _) =
            unpack(tick_now(&mut ws, &ledger).expect("settle event"));
        assert_eq!(outstanding, 0);
        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].task_id, "agent1");
        assert_eq!(settled[0].status, "completed");
        assert_eq!(
            settled[0].summary.as_deref(),
            Some("Agent \"Run pnpm build\" finished")
        );
        // The notification record itself strips to nothing; the assistant
        // response is the rendered out-of-turn content.
        assert_eq!(turns.len(), 1);
        assert!(turns[0].id.starts_with("bg-"));
    }

    #[test]
    fn codeg_sent_prompt_is_foreground_and_not_surfaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());

        ledger.record_text("修复登录 bug");
        write_lines(
            &path,
            &[
                &user_prompt_array("u1", "修复登录 bug"),
                &assistant_text("a1", "On it."),
            ],
        );
        assert!(
            tick_now(&mut ws, &ledger).is_none(),
            "foreground turn must not surface as overlay"
        );
    }

    #[test]
    fn same_text_refire_without_ledger_entry_is_background() {
        // The /loop case: codeg sent the text once (consumed), the scheduler
        // re-fires the SAME text later — second occurrence must surface.
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());

        ledger.record_text("查询武汉当前天气");
        write_lines(
            &path,
            &[
                &user_prompt_array("u1", "查询武汉当前天气"),
                &assistant_text("a1", "24°C 多云"),
            ],
        );
        assert!(tick_now(&mut ws, &ledger).is_none());

        write_lines(
            &path,
            &[
                &cron_prompt("查询武汉当前天气"),
                &assistant_text("a2", "25°C 晴"),
            ],
        );
        let (turns, ..) = unpack(tick_now(&mut ws, &ledger).expect("cron turn surfaces"));
        assert_eq!(turns.len(), 1, "cron assistant response renders as overlay");
    }

    #[test]
    fn meta_array_expansion_does_not_flip_mode() {
        // A slash-command expansion (isMeta + ARRAY content) belongs to the
        // foreground turn that issued the command.
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());

        ledger.record_text("/init");
        let command_record = r#"{"type":"user","timestamp":"2026-07-07T03:50:00.000Z","uuid":"u-cmd","message":{"role":"user","content":"<command-name>/init</command-name><command-message>init</command-message><command-args></command-args>"}}"#;
        let expansion = r#"{"type":"user","timestamp":"2026-07-07T03:50:00.100Z","uuid":"u-exp","isMeta":true,"message":{"role":"user","content":[{"type":"text","text":"Please analyze this codebase..."}]}}"#;
        write_lines(
            &path,
            &[
                command_record,
                expansion,
                &assistant_text("a1", "Analyzing..."),
            ],
        );
        assert!(
            tick_now(&mut ws, &ledger).is_none(),
            "slash command turn is foreground end-to-end"
        );
    }

    #[test]
    fn growing_turn_reemits_with_same_id_and_partial_lines_carry() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());

        write_lines(&path, &[&notification("t1", "completed")]);
        let _ = tick_now(&mut ws, &ledger); // settle-only event

        write_lines(&path, &[&assistant_text("a1", "step one")]);
        let (turns1, ..) = unpack(tick_now(&mut ws, &ledger).expect("first turn"));
        assert_eq!(turns1.len(), 1);
        let id1 = turns1[0].id.clone();

        // Append a PARTIAL line (no newline yet): nothing must surface.
        let more = assistant_text("a2", "step two");
        let (head, tail) = more.split_at(more.len() / 2);
        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(head.as_bytes()).unwrap();
        }
        assert!(
            tick_now(&mut ws, &ledger).is_none(),
            "partial line must not parse"
        );

        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(tail.as_bytes()).unwrap();
            f.write_all(b"\n").unwrap();
        }
        let (turns2, ..) = unpack(tick_now(&mut ws, &ledger).expect("completed line surfaces"));
        // Same episode: a NEW assistant message is a NEW turn (bg-…-1); the
        // first turn's content didn't change so it is not re-emitted.
        assert_eq!(turns2.len(), 1);
        assert_ne!(turns2[0].id, id1);
        assert!(turns2[0].id.starts_with("bg-"));
    }

    #[test]
    fn truncation_rebaselines_without_stale_turns() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[&notification("t1", "completed")]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());
        let _ = tick_now(&mut ws, &ledger);

        std::fs::write(&path, b"").unwrap();
        // Shrink triggers re-baseline; no panic, no stale content.
        let event = tick_now(&mut ws, &ledger);
        if let Some(e) = event {
            let (turns, _, settled, watermark) = unpack(e);
            assert!(turns.is_empty());
            assert!(settled.is_empty());
            assert_eq!(watermark, 0);
        }
        write_lines(&path, &[&assistant_text("a9", "after rewrite")]);
        // Post-truncation content is foreground by default (no initiator seen).
        assert!(tick_now(&mut ws, &ledger).is_none());
    }

    #[test]
    fn send_message_rearms_settled_task() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[&agent_ack("agentX")]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());
        let _ = tick_now(&mut ws, &ledger);

        write_lines(&path, &[&notification("agentX", "completed")]);
        let (_, outstanding, ..) = unpack(tick_now(&mut ws, &ledger).unwrap());
        assert_eq!(outstanding, 0);

        let send = r#"{"type":"assistant","timestamp":"2026-07-07T03:52:00.000Z","uuid":"a-send","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_03","name":"SendMessage","input":{"to":"agentX","summary":"continue","message":"go on"}}]}}"#;
        write_lines(&path, &[send]);
        let (_, outstanding, ..) = unpack(tick_now(&mut ws, &ledger).unwrap());
        assert_eq!(outstanding, 1, "resumed sub-agent re-arms the keep-alive");
    }

    #[test]
    fn ledger_prefix_matches_and_consumes_once() {
        let ledger = PromptLedger::shared();
        ledger.record_text("deploy the app");
        assert!(ledger.consume_matching("deploy the app\n<system-hint>extra</system-hint>"));
        assert!(
            !ledger.consume_matching("deploy the app"),
            "an entry is consumed exactly once"
        );
    }

    #[test]
    fn initiator_classification_ground_rules() {
        // tool-result-only user record: continues the turn.
        let ack: serde_json::Value = serde_json::from_str(&agent_ack("x")).unwrap();
        assert!(turn_initiator_text(&ack).is_none());

        // task-notification string record: initiates (raw text, no ledger hit).
        let note: serde_json::Value =
            serde_json::from_str(&notification("x", "completed")).unwrap();
        assert!(turn_initiator_text(&note)
            .unwrap()
            .starts_with("<task-notification>"));

        // cron prompt (isMeta + string): initiates with the prompt text.
        let cron: serde_json::Value = serde_json::from_str(&cron_prompt("check weather")).unwrap();
        assert_eq!(turn_initiator_text(&cron).as_deref(), Some("check weather"));

        // context-continuation summary: never a boundary.
        let cont = format!(
            r#"{{"type":"user","uuid":"u-cont","message":{{"role":"user","content":"{}..."}}}}"#,
            CONTEXT_CONTINUATION_PREFIX
        );
        let cont: serde_json::Value = serde_json::from_str(&cont).unwrap();
        assert!(turn_initiator_text(&cont).is_none());

        // slash command record matches via its display form.
        let cmd = r#"{"type":"user","uuid":"u-cmd","message":{"role":"user","content":"<command-name>/init</command-name><command-args>now</command-args>"}}"#;
        let cmd: serde_json::Value = serde_json::from_str(cmd).unwrap();
        assert_eq!(turn_initiator_text(&cmd).as_deref(), Some("/init now"));
    }
}
