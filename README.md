# Codeg

[![Release](https://img.shields.io/github/v/release/xintaofei/codeg)](https://github.com/xintaofei/codeg/releases)
[![License](https://img.shields.io/github/license/xintaofei/codeg)](./LICENSE)
[![Tauri](https://img.shields.io/badge/Tauri-2.x-24C8DB)](https://tauri.app/)
[![Next.js](https://img.shields.io/badge/Next.js-16-black)](https://nextjs.org/)
[![Docker](https://img.shields.io/badge/Docker-ready-2496ED)](./Dockerfile)

<p>
  <strong>English</strong> |
  <a href="./docs/readme/README.zh-CN.md">简体中文</a> |
  <a href="./docs/readme/README.zh-TW.md">繁體中文</a> |
  <a href="./docs/readme/README.ja.md">日本語</a> |
  <a href="./docs/readme/README.ko.md">한국어</a> |
  <a href="./docs/readme/README.es.md">Español</a> |
  <a href="./docs/readme/README.de.md">Deutsch</a> |
  <a href="./docs/readme/README.fr.md">Français</a> |
  <a href="./docs/readme/README.pt.md">Português</a> |
  <a href="./docs/readme/README.ar.md">العربية</a>
</p>

Codeg (Code Generation) is an enterprise-grade multi-agent coding workspace.
It unifies local AI coding agents (Claude Code, Codex CLI, OpenCode, Gemini CLI,
OpenClaw, Cline, etc.) in a desktop app, standalone server, or Docker container — enabling
remote development from any browser — with conversation aggregation, parallel `git worktree`
development, MCP/Skills management, chat channel interactions (Telegram, Lark, iLink, etc.),
and integrated Git/file/terminal workflows.

![gallery](./docs/images/gallery.svg)

## Main Interface
![Codeg Light](./docs/images/main-light.png#gh-light-mode-only)
![Codeg Dark](./docs/images/main-dark.png#gh-dark-mode-only)

## Settings
| Agents | MCP | Skills | Version Control | Web Service |
| :---: | :---: | :---: | :---: | :---: |
| ![Agents](./docs/images/1-light.png#gh-light-mode-only) ![Agents](./docs/images/1-dark.png#gh-dark-mode-only) | ![MCP](./docs/images/2-light.png#gh-light-mode-only) ![MCP](./docs/images/2-dark.png#gh-dark-mode-only) | ![Skills](./docs/images/3-light.png#gh-light-mode-only) ![Skills](./docs/images/3-dark.png#gh-dark-mode-only) | ![Version Control](./docs/images/4-light.png#gh-light-mode-only) ![Version Control](./docs/images/4-dark.png#gh-dark-mode-only) | ![Web Service](./docs/images/5-light.png#gh-light-mode-only) ![Web Service](./docs/images/5-dark.png#gh-dark-mode-only) |

## Highlights

- Unified multi-agent workspace in the same project
- Local conversation ingestion with structured rendering
- Parallel development with built-in `git worktree` flows
- **Project Boot** — visually scaffold new projects with live preview
- **Chat Channels** — connect Telegram, Lark (Feishu), iLink (Weixin) and more to your coding agents for real-time notifications, full session interaction, and remote task control
- MCP management (local scan + registry search/install)
- Skills management (global and project scope)
- Git remote account management (GitHub and other Git servers)
- Web service mode — access Codeg from any browser for remote work
- **Standalone server deployment** — run `codeg-server` on any Linux/macOS server, access via browser
- **Docker support** — `docker compose up` or `docker run`, with custom token, port, and volume mounts for data persistence and project directories
- Integrated engineering loop (file tree, diff, git changes, commit, terminal)

## Project Boot

Create new projects visually with a split-pane interface: configure on the left, preview in real time on the right.

![Project Boot Light](./docs/images/project-boot-light.png#gh-light-mode-only)
![Project Boot Dark](./docs/images/project-boot-dark.png#gh-dark-mode-only)

### What it does

- **Visual Configuration** — pick style, color theme, icon library, font, border radius, and more from dropdowns; the preview iframe updates instantly
- **Live Preview** — see your chosen look & feel rendered in real time before creating anything
- **One-Click Scaffolding** — hit "Create Project" and the launcher runs `shadcn init` with your preset, framework template (Next.js / Vite / React Router / Astro / Laravel), and package manager of choice (pnpm / npm / yarn / bun)
- **Package Manager Detection** — automatically checks which package managers are installed and shows their versions
- **Seamless Integration** — the newly created project opens in Codeg's workspace right away

Currently supports **shadcn/ui** project scaffolding, with a tab-based design ready for more project types in the future.

## Chat Channels

Connect your favorite messaging apps — Telegram, Lark (Feishu), iLink (Weixin), and more — to your AI coding agents. Create tasks, send follow-up messages, approve permissions, resume sessions, and monitor activity — all from your chat app. Receive real-time agent responses with tool-call details, permission prompts, and completion summaries without ever opening a browser.

### Supported Channels

| Channel | Protocol | Status |
| --- | --- | --- |
| Telegram | Bot API (HTTP long-polling) | Built-in |
| Lark (Feishu) | WebSocket + REST API | Built-in |
| iLink (Weixin) | WebSocket + REST API | Built-in |

> More channels (Discord, Slack, DingTalk, etc.) are planned for future releases.

### Key Features

- **Session Interaction** — run full agent sessions from your chat app: `/folder` to pick a project, `/agent` to choose an agent, `/task <description>` to start a task, then send follow-up messages as plain text. `/resume` to continue a previous session, `/cancel` to abort, `/sessions` to list active sessions
- **Permission Control** — agents can request tool-execution permissions in-chat; respond with `/approve` (or `/approve always` for auto-approve) and `/deny` without switching context
- **Event Notifications** — agent turn completions, tool calls, and errors are pushed to all enabled channels in real time with rich formatting
- **Query Commands** — `/search <keyword>`, `/today`, `/status`, `/help` for quick lookups; configurable command prefix
- **Daily Reports** — automated daily summary at a scheduled time, including conversation counts, agent-type breakdown, and project activity
- **Multi-Language** — message templates in 10 languages (English, Simplified/Traditional Chinese, Japanese, Korean, Spanish, German, French, Portuguese, Arabic)
- **Secure Credentials** — tokens stored in the OS keyring, never exposed in config files or logs
- **Rich Messages** — Markdown formatting for Telegram, card-based layout for Lark; plain-text fallback for all platforms

### Setup

1. Create a channel in **Settings → Chat Channels** (choose Telegram, Lark, or iLink)
2. Enter your bot token (Telegram), app credentials (Lark), or scan QR code to log in (iLink) — stored securely in the OS keyring
3. Configure event filters and optional daily report schedule
4. Connect — messages start flowing as agents emit events

## Supported Agents

| Agent | Environment Variable Path | macOS / Linux Default | Windows Default |
| --- | --- | --- | --- |
| Claude Code | `$CLAUDE_CONFIG_DIR/projects` | `~/.claude/projects` | `%USERPROFILE%\\.claude\\projects` |
| Codex CLI | `$CODEX_HOME/sessions` | `~/.codex/sessions` | `%USERPROFILE%\\.codex\\sessions` |
| OpenCode | `$XDG_DATA_HOME/opencode/opencode.db` | `~/.local/share/opencode/opencode.db` | `%USERPROFILE%\\.local\\share\\opencode\\opencode.db` |
| Gemini CLI | `$GEMINI_CLI_HOME/.gemini` | `~/.gemini` | `%USERPROFILE%\\.gemini` |
| OpenClaw | — | `~/.openclaw/agents` | `%USERPROFILE%\\.openclaw\\agents` |
| Cline | `$CLINE_DIR` | `~/.cline/data/tasks` | `%USERPROFILE%\\.cline\\data\\tasks` |

> Note: environment variables take precedence over fallback paths.

## Quick Start

### Requirements

- Node.js `>=22` (recommended)
- pnpm `>=10`
- Rust stable (2021 edition)
- Tauri 2 build dependencies (desktop mode only)

Linux (Debian/Ubuntu) example:

```bash
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf
```

### Development

```bash
pnpm install

# Frontend static export to out/
pnpm build

# Full desktop app (Tauri + Next.js)
pnpm tauri dev

# Frontend only
pnpm dev

# Desktop build
pnpm tauri build

# Standalone server (no Tauri/GUI required)
pnpm server:dev

# Build server release binary
pnpm server:build

# Lint
pnpm eslint .

# Rust checks (run in src-tauri/)
cargo check
cargo clippy
cargo build
```

### Server Deployment

Codeg can run as a standalone web server without a desktop environment.

#### Option 1: One-line install (Linux / macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/xintaofei/codeg/main/install.sh | bash
```

Install a specific version or to a custom directory:

```bash
curl -fsSL https://raw.githubusercontent.com/xintaofei/codeg/main/install.sh | bash -s -- --version v0.5.2 --dir ~/.local/bin
```

Then run:

```bash
codeg-server
```

#### Option 2: One-line install (Windows PowerShell)

```powershell
irm https://raw.githubusercontent.com/xintaofei/codeg/main/install.ps1 | iex
```

Or install a specific version:

```powershell
.\install.ps1 -Version v0.5.2
```

#### Option 3: Download from GitHub Releases

Pre-built binaries (with bundled web assets) are available on the [Releases](https://github.com/xintaofei/codeg/releases) page:

| Platform | File |
| --- | --- |
| Linux x64 | `codeg-server-linux-x64.tar.gz` |
| Linux arm64 | `codeg-server-linux-arm64.tar.gz` |
| macOS x64 | `codeg-server-darwin-x64.tar.gz` |
| macOS arm64 | `codeg-server-darwin-arm64.tar.gz` |
| Windows x64 | `codeg-server-windows-x64.zip` |

```bash
# Example: download, extract, and run
tar xzf codeg-server-linux-x64.tar.gz
cd codeg-server-linux-x64
CODEG_STATIC_DIR=./web ./codeg-server
```

#### Option 4: Docker

```bash
# Using Docker Compose (recommended)
docker compose up -d

# Or run directly with Docker
docker run -d -p 3080:3080 -v codeg-data:/data ghcr.io/xintaofei/codeg:latest

# With custom token and project directory mounted
docker run -d -p 3080:3080 \
  -v codeg-data:/data \
  -v /path/to/projects:/projects \
  -e CODEG_TOKEN=your-secret-token \
  ghcr.io/xintaofei/codeg:latest
```

The Docker image uses a multi-stage build (Node.js + Rust → slim Debian runtime) and includes `git` and `ssh` for repository operations. Data is persisted in the `/data` volume. You can optionally mount project directories to access local repos from within the container.

#### Option 5: Build from source

```bash
pnpm install && pnpm build          # build frontend
cd src-tauri
cargo build --release --bin codeg-server --no-default-features
CODEG_STATIC_DIR=../out ./target/release/codeg-server
```

#### Configuration

Environment variables:

| Variable | Default | Description |
| --- | --- | --- |
| `CODEG_PORT` | `3080` | HTTP port |
| `CODEG_HOST` | `0.0.0.0` | Bind address |
| `CODEG_TOKEN` | *(random)* | Auth token (printed to stderr on start) |
| `CODEG_DATA_DIR` | `~/.local/share/codeg` | SQLite database directory (also roots `uploads/`, `pets/`) |
| `CODEG_STATIC_DIR` | `./web` or `./out` | Next.js static export directory |
| `CODEG_UPLOAD_MAX_TOTAL_BYTES` | *(unset)* | Hard cap on total bytes resident under `<data dir>/uploads/`. Plain decimal byte count (e.g. `10737418240` for 10 GiB). Unset, `0`, or an unparseable value disables the cap and prints a startup line so the posture is visible. The cap is enforced within a single `codeg-server` process — horizontally-scaled deployments sharing one `uploads/` volume need external coordination (file lock, Redis, reverse-proxy quota). |
| `CODEG_UPLOAD_QUOTA_STRICT` | *(unset)* | When truthy (`1` / `true` / `yes` / `on`), abort startup with exit code 2 if `CODEG_UPLOAD_MAX_TOTAL_BYTES` is set to an unparseable value, instead of fail-open with a WARN. Use this when your security policy requires "configured quota must be effective". |

## Architecture

```text
Next.js 16 (Static Export) + React 19
        |
        | invoke() (desktop) / fetch() + WebSocket (web)
        v
  ┌─────────────────────────┐
  │   Transport Abstraction  │
  │  (Tauri IPC or HTTP/WS) │
  └─────────────────────────┘
        |
        v
┌─── Tauri Desktop ───┐    ┌─── codeg-server ───┐
│  Tauri 2 Commands    │    │  Axum HTTP + WS    │
│  (window management) │    │  (standalone mode)  │
└──────────┬───────────┘    └──────────┬──────────┘
           └──────────┬───────────────┘
                      v
            Shared Rust Core
              |- AppState
              |- ACP Manager
              |- Parsers (conversation ingestion)
              |- Chat Channels
              |- Git / File Tree / Terminal
              |- MCP marketplace + config
              |- SeaORM + SQLite
                      |
              ┌───────┼───────┐
              v       v       v
  Local Filesystem  Git   Chat Channels
    / Git Repos    Repos  (Telegram, Lark, iLink)
```

## Constraints

- Frontend uses static export (`output: "export"`)
- No Next.js dynamic routes (`[param]`); use query params instead
- Tauri command params: frontend `camelCase`, Rust `snake_case`
- TypeScript strict mode

## Privacy & Security

- Local-first by default for parsing, storage, and project operations
- Network access happens only on user-triggered actions
- System proxy support for enterprise environments
- Web service mode uses token-based authentication

## Acknowledgments

- [LinuxDO](https://linux.do) — the community where it all began
- [ACP](https://agentclientprotocol.com) — the Agent Client Protocol (ACP) is the foundation that enables Codeg to connect with multiple agents

## License

Apache-2.0. See `LICENSE`.
