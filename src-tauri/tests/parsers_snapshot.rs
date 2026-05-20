//! Integration snapshot tests for the agent parsers.
//!
//! Each test materializes a minimal on-disk fixture under a `tempfile::tempdir`,
//! constructs the parser with `with_base_dir(...)`, and compares the
//! `list_conversations` + `get_conversation` outputs against committed `.snap`
//! files via `insta::assert_json_snapshot!`.
//!
//! Why redact timestamps: a few parser code paths fall back to `Utc::now()` when
//! a JSON value is missing a timestamp. Redacting `started_at`/`ended_at`/
//! `timestamp`/`completed_at` everywhere keeps snapshots stable even if such a
//! fallback fires unexpectedly.

use std::fs;
use std::path::Path;

use codeg_lib::parsers::{
    claude::ClaudeParser, cline::ClineParser, codex::CodexParser, gemini::GeminiParser,
    openclaw::OpenClawParser, opencode::OpenCodeParser, AgentParser,
};
use insta::assert_json_snapshot;
use serde_json::json;

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dir");
    }
    fs::write(path, contents).expect("write fixture file");
}

// ────────────────────────────────────────────────────────────────────────────
// Claude
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn claude_minimal_session_snapshot() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let base = temp.path().to_path_buf();
    // Claude stores conversations under `<base>/<encoded-folder>/<id>.jsonl`.
    let project_dir = base.join("-tmp-demo");
    let session_id = "claude-sess-001";
    let jsonl = format!(
        "{}\n{}\n",
        json!({
            "type": "user",
            "sessionId": session_id,
            "timestamp": "2026-03-01T10:00:00Z",
            "uuid": "u1",
            "cwd": "/tmp/demo",
            "gitBranch": "main",
            "message": { "content": [{"type": "text", "text": "hello"}] }
        }),
        json!({
            "type": "assistant",
            "sessionId": session_id,
            "timestamp": "2026-03-01T10:00:02Z",
            "uuid": "a1",
            "message": {
                "model": "claude-sonnet-4-6",
                "content": [{"type": "text", "text": "world"}],
                "usage": {
                    "input_tokens": 1000,
                    "output_tokens": 200,
                    "cache_creation_input_tokens": 300,
                    "cache_read_input_tokens": 400
                }
            }
        }),
    );
    write(&project_dir.join(format!("{session_id}.jsonl")), &jsonl);

    let parser = ClaudeParser::with_base_dir(base);
    let summaries = parser.list_conversations().expect("list claude");
    let detail = parser.get_conversation(session_id).expect("detail claude");

    assert_json_snapshot!("claude_list", summaries, {
        ".**.started_at" => "[ts]",
        ".**.ended_at" => "[ts]",
    });
    assert_json_snapshot!("claude_detail", detail, {
        ".**.started_at" => "[ts]",
        ".**.ended_at" => "[ts]",
        ".**.timestamp" => "[ts]",
        ".**.completed_at" => "[ts]",
    });
}

// ────────────────────────────────────────────────────────────────────────────
// Codex
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn codex_minimal_session_snapshot() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let base = temp.path().to_path_buf();
    let session_id = "codex-sess-001";
    // Codex walks `<base>/**/*.jsonl` and requires the filename to start with
    // `rollout-` (real Codex CLI naming convention) for both list and detail.
    let jsonl_path = base
        .join("2026")
        .join("03")
        .join(format!("rollout-{session_id}.jsonl"));
    let jsonl = format!(
        "{}\n{}\n{}\n{}\n",
        json!({
            "timestamp": "2026-03-01T10:00:00Z",
            "type": "session_meta",
            "payload": {
                "id": session_id,
                "cwd": "/tmp/demo",
                "cli_version": "0.1.0",
                "git": {"branch": "main"}
            }
        }),
        json!({
            "timestamp": "2026-03-01T10:00:00.500Z",
            "type": "turn_context",
            "payload": {"model": "gpt-5.1-codex"}
        }),
        json!({
            "timestamp": "2026-03-01T10:00:01Z",
            "type": "event_msg",
            "payload": {"type": "user_message", "message": "ping"}
        }),
        json!({
            "timestamp": "2026-03-01T10:00:02Z",
            "type": "event_msg",
            "payload": {"type": "agent_message", "message": "pong"}
        }),
    );
    write(&jsonl_path, &jsonl);

    let parser = CodexParser::with_base_dir(base);
    let summaries = parser.list_conversations().expect("list codex");
    let detail = parser.get_conversation(session_id).expect("detail codex");

    assert_json_snapshot!("codex_list", summaries, {
        ".**.started_at" => "[ts]",
        ".**.ended_at" => "[ts]",
    });
    assert_json_snapshot!("codex_detail", detail, {
        ".**.started_at" => "[ts]",
        ".**.ended_at" => "[ts]",
        ".**.timestamp" => "[ts]",
        ".**.completed_at" => "[ts]",
    });
}

// ────────────────────────────────────────────────────────────────────────────
// Gemini
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn gemini_minimal_session_snapshot() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let base = temp.path().to_path_buf();
    // Gemini layout: <base>/tmp/<project>/chats/session-*.json + .project_root
    let project_dir = base.join("tmp").join("codeg");
    let chats_dir = project_dir.join("chats");
    write(
        &project_dir.join(".project_root"),
        "/Users/test/workspace/demo",
    );
    let session_id = "gemini-sess-001";
    let content = serde_json::to_string_pretty(&json!({
        "sessionId": session_id,
        "projectHash": "abc",
        "startTime": "2026-03-02T04:30:00.000Z",
        "lastUpdated": "2026-03-02T04:30:02.000Z",
        "messages": [
            {
                "id": "u1",
                "timestamp": "2026-03-02T04:30:00.000Z",
                "type": "user",
                "content": [{"text": "ping"}]
            },
            {
                "id": "a1",
                "timestamp": "2026-03-02T04:30:02.000Z",
                "type": "gemini",
                "content": "pong",
                "tokens": {"input": 10, "output": 20, "cached": 3},
                "model": "gemini-2.5-pro"
            }
        ]
    }))
    .expect("serialize gemini fixture");
    write(
        &chats_dir.join(format!("session-2026-03-02T04-30-{session_id}.json")),
        &content,
    );

    let parser = GeminiParser::with_base_dir(base);
    let summaries = parser.list_conversations().expect("list gemini");
    let detail = parser.get_conversation(session_id).expect("detail gemini");

    assert_json_snapshot!("gemini_list", summaries, {
        ".**.started_at" => "[ts]",
        ".**.ended_at" => "[ts]",
    });
    assert_json_snapshot!("gemini_detail", detail, {
        ".**.started_at" => "[ts]",
        ".**.ended_at" => "[ts]",
        ".**.timestamp" => "[ts]",
        ".**.completed_at" => "[ts]",
    });
}

// ────────────────────────────────────────────────────────────────────────────
// OpenClaw
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn openclaw_minimal_session_snapshot() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let base = temp.path().to_path_buf();
    // Layout: <base>/<agent_id>/sessions/<session_id>.jsonl
    let agent_id = "test-agent";
    let session_id = "openclaw-sess-001";
    let conversation_id = format!("{agent_id}/{session_id}");
    let sessions_dir = base.join(agent_id).join("sessions");
    let jsonl = format!(
        "{}\n{}\n{}\n",
        json!({
            "type": "session",
            "version": 3,
            "id": session_id,
            "timestamp": "2026-03-17T01:00:00.000Z",
            "cwd": "/tmp/demo"
        }),
        json!({
            "type": "message",
            "id": "u1",
            "parentId": null,
            "timestamp": "2026-03-17T01:00:01.000Z",
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": "Hello"}]
            }
        }),
        json!({
            "type": "message",
            "id": "a1",
            "parentId": "u1",
            "timestamp": "2026-03-17T01:00:02.000Z",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "Hi"}],
                "model": "gpt-5.4",
                "usage": {"input": 100, "output": 50, "cacheRead": 200, "cacheWrite": 0, "totalTokens": 350}
            }
        }),
    );
    write(&sessions_dir.join(format!("{session_id}.jsonl")), &jsonl);

    let parser = OpenClawParser::with_base_dir(base);
    let summaries = parser.list_conversations().expect("list openclaw");
    let detail = parser
        .get_conversation(&conversation_id)
        .expect("detail openclaw");

    assert_json_snapshot!("openclaw_list", summaries, {
        ".**.started_at" => "[ts]",
        ".**.ended_at" => "[ts]",
    });
    assert_json_snapshot!("openclaw_detail", detail, {
        ".**.started_at" => "[ts]",
        ".**.ended_at" => "[ts]",
        ".**.timestamp" => "[ts]",
        ".**.completed_at" => "[ts]",
    });
}

// ────────────────────────────────────────────────────────────────────────────
// Cline
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn cline_minimal_session_snapshot() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let base = temp.path().to_path_buf();
    // Layout:
    //   <base>/state/taskHistory.json
    //   <base>/tasks/<id>/api_conversation_history.json
    //   <base>/tasks/<id>/task_metadata.json  (optional)
    //
    // Note: started_at is derived by parsing the entry id as a unix-ms
    // timestamp, so use a real timestamp string here.
    let task_id = "1740825600000"; // 2026-03-01T08:00:00Z in ms
    let history = json!([
        {
            "id": task_id,
            "ts": 1_740_825_602_000_i64,
            "task": "ping",
            "tokensIn": 10,
            "tokensOut": 20,
            "totalCost": 0.0,
            "cwdOnTaskInitialization": "/tmp/demo",
            "modelId": "claude-sonnet-4-6"
        }
    ]);
    write(
        &base.join("state").join("taskHistory.json"),
        &serde_json::to_string(&history).unwrap(),
    );

    let api_history = json!([
        {
            "role": "user",
            "content": [{"type": "text", "text": "ping"}],
            "ts": 1_740_825_600_500_i64
        },
        {
            "role": "assistant",
            "content": [{"type": "text", "text": "pong"}],
            "ts": 1_740_825_601_500_i64,
            "modelInfo": {"modelId": "claude-sonnet-4-6"},
            "metrics": {"tokens": {"prompt": 10, "completion": 20, "cached": 3}}
        }
    ]);
    write(
        &base
            .join("tasks")
            .join(task_id)
            .join("api_conversation_history.json"),
        &serde_json::to_string(&api_history).unwrap(),
    );

    let parser = ClineParser::with_base_dir(base);
    let summaries = parser.list_conversations().expect("list cline");
    let detail = parser.get_conversation(task_id).expect("detail cline");

    assert_json_snapshot!("cline_list", summaries, {
        ".**.started_at" => "[ts]",
        ".**.ended_at" => "[ts]",
    });
    assert_json_snapshot!("cline_detail", detail, {
        ".**.started_at" => "[ts]",
        ".**.ended_at" => "[ts]",
        ".**.timestamp" => "[ts]",
        ".**.completed_at" => "[ts]",
    });
}

// ────────────────────────────────────────────────────────────────────────────
// OpenCode
// ────────────────────────────────────────────────────────────────────────────

/// OpenCode parser reads from a SeaORM-managed SQLite file. It does NOT import
/// the OpenCode CLI's migrations — it issues raw SELECTs against three tables
/// (`session`, `message`, `part`). So the test fixture just creates those
/// tables with the columns the parser actually queries and inserts a minimal
/// conversation.
///
/// `OpenCodeParser` builds its own current-thread runtime via `block_on` on
/// every call, so it's safe to drive from either `#[test]` (sync) or a
/// `#[tokio::test]`. We use sync here and spin up a local runtime only for
/// the async DB setup.
#[test]
fn opencode_minimal_session_snapshot() {
    use sea_orm::{ConnectionTrait, Database, DatabaseBackend, Statement};

    let temp = tempfile::tempdir().expect("create tempdir");
    let base = temp.path().to_path_buf();
    let db_path = base.join("opencode.db");
    let session_id = "oc-sess-001";

    // 2026-03-01T10:00:00Z in milliseconds.
    let t0: i64 = 1_772_020_800_000;
    let t_user_created = t0 + 500;
    let t_asst_created = t0 + 2_000;
    let t_asst_completed = t0 + 3_000;
    let t_updated = t0 + 4_000;

    // Build the fixture DB inside a one-off current-thread runtime.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");
    rt.block_on(async {
        let conn = Database::connect(format!("sqlite:{}?mode=rwc", db_path.display()))
            .await
            .expect("open sqlite");

        for ddl in [
            "CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT, title TEXT, \
             time_created INTEGER, time_updated INTEGER)",
            "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, \
             time_created INTEGER, data TEXT)",
            "CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, \
             time_created INTEGER, data TEXT)",
        ] {
            conn.execute(Statement::from_string(DatabaseBackend::Sqlite, ddl))
                .await
                .expect("create table");
        }

        // Session row.
        conn.execute(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "INSERT INTO session (id, directory, title, time_created, time_updated) \
             VALUES (?, ?, ?, ?, ?)",
            [
                session_id.into(),
                "/tmp/demo".into(),
                "OpenCode demo session".into(),
                t0.into(),
                t_updated.into(),
            ],
        ))
        .await
        .expect("insert session");

        // User message.
        let user_data = json!({
            "role": "user",
            "time": { "created": t_user_created },
        })
        .to_string();
        conn.execute(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "INSERT INTO message (id, session_id, time_created, data) VALUES (?, ?, ?, ?)",
            [
                "m-user".into(),
                session_id.into(),
                t_user_created.into(),
                user_data.into(),
            ],
        ))
        .await
        .expect("insert user message");

        // Assistant message with usage + completion.
        let asst_data = json!({
            "role": "assistant",
            "modelID": "claude-sonnet-4-6",
            "time": { "created": t_asst_created, "completed": t_asst_completed },
            "tokens": {
                "input": 12,
                "output": 15,
                "cache": { "read": 0, "write": 0 },
            },
        })
        .to_string();
        conn.execute(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "INSERT INTO message (id, session_id, time_created, data) VALUES (?, ?, ?, ?)",
            [
                "m-asst".into(),
                session_id.into(),
                t_asst_created.into(),
                asst_data.into(),
            ],
        ))
        .await
        .expect("insert assistant message");

        // Text parts for each message.
        for (pid, mid, t, text) in [
            ("p-user-text", "m-user", t_user_created, "hello opencode"),
            ("p-asst-text", "m-asst", t_asst_created + 500, "world!"),
        ] {
            let data = json!({ "type": "text", "text": text }).to_string();
            conn.execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "INSERT INTO part (id, message_id, time_created, data) VALUES (?, ?, ?, ?)",
                [pid.into(), mid.into(), t.into(), data.into()],
            ))
            .await
            .expect("insert part");
        }
    });

    let parser = OpenCodeParser::with_base_dir(base);
    let conversations = parser.list_conversations().expect("list conversations");
    assert_json_snapshot!("opencode_list", conversations, {
        ".**.started_at" => "[ts]",
        ".**.ended_at" => "[ts]",
    });

    let detail = parser
        .get_conversation(session_id)
        .expect("get conversation");
    assert_json_snapshot!("opencode_detail", detail, {
        ".**.started_at" => "[ts]",
        ".**.ended_at" => "[ts]",
        ".**.timestamp" => "[ts]",
        ".**.completed_at" => "[ts]",
    });
}
