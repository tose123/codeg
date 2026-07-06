use crate::models::agent::AgentType;

#[derive(Debug, Clone)]
pub enum AgentDistribution {
    Npx {
        version: &'static str,
        package: &'static str,
        /// The command name provided by this npx package (e.g. "gemini", "openclaw").
        cmd: &'static str,
        args: &'static [&'static str],
        env: &'static [(&'static str, &'static str)],
        /// Minimum Node.js version required, e.g. "22.12.0". None means no specific requirement.
        node_required: Option<&'static str>,
    },
    Binary {
        version: &'static str,
        cmd: &'static str,
        args: &'static [&'static str],
        env: &'static [(&'static str, &'static str)],
        platforms: &'static [PlatformBinary],
    },
    /// Python agents launched through `uvx` (the `uv` tool runner), which
    /// fetches + caches the pinned package on first use — analogous to npx.
    /// Used for ACP agents distributed as Python packages (e.g. Hermes).
    Uvx {
        version: &'static str,
        /// The `uvx --from` package spec, e.g. "hermes-agent[acp,mcp]==0.18.0".
        package: &'static str,
        /// The console-script entry point to run, e.g. "hermes-acp".
        cmd: &'static str,
        args: &'static [&'static str],
        env: &'static [(&'static str, &'static str)],
        /// Minimum `uv` version required, e.g. "0.5.0". None means no specific requirement.
        uv_required: Option<&'static str>,
        /// Interpreter to pin via `uvx --python <ver>`, e.g. `Some("3.13")`.
        /// `None` lets uvx pick its default interpreter. Set this when the
        /// package (or a transitive dep) does not support the machine's default
        /// Python — uv auto-downloads a managed build of the pinned version.
        python: Option<&'static str>,
        /// Fallback command resolvable on PATH when `uvx` is unavailable, e.g.
        /// `Some(("hermes", &["acp"]))` — lets users who installed the agent via
        /// its official installer launch it without `uv`.
        system_cmd: Option<(&'static str, &'static [&'static str])>,
    },
}

#[derive(Debug, Clone)]
pub struct PlatformBinary {
    pub platform: &'static str,
    pub url: &'static str,
}

#[derive(Debug, Clone)]
pub struct AcpAgentMeta {
    pub agent_type: AgentType,
    /// 是否经 ACP 线缆（session/new 的 `mcpServers` 字段）向该 agent 转发 MCP
    /// 服务器——既包括用户配置的服务器，也包括内置 codeg-mcp 伴生进程。
    /// OpenClaw 拒绝 `mcpServers` 中的任何服务器条目（会使 session/new 失败），
    /// 故置 false。注意空列表 `[]` 仍会按 ACP schema 序列化、OpenClaw 可接受——
    /// 闸门只是保证该列表对 OpenClaw 恒为空（不含任何条目）。
    pub supports_mcp: bool,
    pub name: &'static str,
    pub description: &'static str,
    pub distribution: AgentDistribution,
}

impl AcpAgentMeta {
    pub fn registry_version(&self) -> Option<&'static str> {
        match &self.distribution {
            AgentDistribution::Npx { version, .. }
            | AgentDistribution::Binary { version, .. }
            | AgentDistribution::Uvx { version, .. } => Some(*version),
        }
    }
}

pub fn current_platform() -> &'static str {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "darwin-aarch64"
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "darwin-x86_64"
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        "linux-aarch64"
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "linux-x86_64"
    }
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        "windows-aarch64"
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        "windows-x86_64"
    }
}

pub fn all_acp_agents() -> Vec<AgentType> {
    vec![
        AgentType::ClaudeCode,
        AgentType::Codex,
        AgentType::Gemini,
        AgentType::OpenClaw,
        AgentType::OpenCode,
        AgentType::Cline,
        AgentType::Hermes,
        AgentType::CodeBuddy,
        AgentType::KimiCode,
        AgentType::Pi,
    ]
}

pub fn registry_id_for(agent_type: AgentType) -> &'static str {
    match agent_type {
        AgentType::ClaudeCode => "claude-acp",
        AgentType::Codex => "codex-acp",
        AgentType::Gemini => "gemini",
        AgentType::OpenClaw => "openclaw-acp",
        AgentType::OpenCode => "opencode",
        AgentType::Cline => "cline",
        AgentType::Hermes => "hermes",
        AgentType::CodeBuddy => "codebuddy-code",
        AgentType::KimiCode => "kimi-code",
        AgentType::Pi => "pi-acp",
    }
}

pub fn from_registry_id(id: &str) -> Option<AgentType> {
    match id {
        "claude-acp" => Some(AgentType::ClaudeCode),
        "codex-acp" => Some(AgentType::Codex),
        "gemini" => Some(AgentType::Gemini),
        "openclaw-acp" => Some(AgentType::OpenClaw),
        "opencode" => Some(AgentType::OpenCode),
        "cline" => Some(AgentType::Cline),
        "hermes" => Some(AgentType::Hermes),
        "codebuddy-code" => Some(AgentType::CodeBuddy),
        "kimi-code" => Some(AgentType::KimiCode),
        "pi-acp" => Some(AgentType::Pi),
        _ => None,
    }
}

pub fn get_agent_meta(agent_type: AgentType) -> AcpAgentMeta {
    debug_assert_eq!(
        from_registry_id(registry_id_for(agent_type)),
        Some(agent_type)
    );
    match agent_type {
        AgentType::ClaudeCode => AcpAgentMeta {
            agent_type,
            supports_mcp: true,
            name: "Claude Code",
            description: "ACP wrapper for Anthropic's Claude",
            distribution: AgentDistribution::Npx {
                version: "0.55.0",
                package: "@agentclientprotocol/claude-agent-acp@0.55.0",
                cmd: "claude-agent-acp",
                args: &[],
                env: &[],
                node_required: None,
            },
        },
        AgentType::Codex => AcpAgentMeta {
            agent_type,
            supports_mcp: true,
            name: "Codex CLI",
            description: "ACP adapter for OpenAI's coding assistant",
            // codex-acp moved from zed-industries (Rust binary) to the
            // agentclientprotocol org (TypeScript rewrite, npx-distributed).
            // 1.1.0 bundles `@openai/codex` 0.142.5 and drives `codex
            // app-server`; since 1.0.1 it also resolves the resumed
            // `model_provider` from `~/.codex/config.toml` (#224), so codeg no
            // longer injects `MODEL_PROVIDER` to keep resumed sessions on the
            // custom provider. 1.1.0 (#263) also reports `/goal` transitions as a
            // structured `session_info_update` (`_meta.codex.goal`) rather than
            // live agent text — see `crate::acp::codex_goal`.
            distribution: AgentDistribution::Npx {
                version: "1.1.0",
                package: "@agentclientprotocol/codex-acp@1.1.0",
                cmd: "codex-acp",
                args: &[],
                env: &[],
                node_required: None,
            },
        },
        AgentType::Gemini => AcpAgentMeta {
            agent_type,
            supports_mcp: true,
            name: "Gemini CLI",
            description: "Google's official CLI for Gemini",
            distribution: AgentDistribution::Npx {
                version: "0.47.0",
                package: "@google/gemini-cli@0.47.0",
                cmd: "gemini",
                args: &["--acp", "--skip-trust"],
                env: &[],
                node_required: Some("20.0.0"),
            },
        },
        AgentType::OpenClaw => AcpAgentMeta {
            agent_type,
            // OpenClaw 拒绝 `mcpServers` 中的任何服务器条目（会使 session/new 失败），
            // 故不向其转发任何 MCP 条目（含 codeg-mcp 伴生进程）。详见 supports_mcp 字段注释。
            supports_mcp: false,
            name: "OpenClaw",
            description: "OpenClaw is a personal AI assistant you run on your own devices.",
            distribution: AgentDistribution::Npx {
                version: "2026.6.11",
                package: "openclaw@2026.6.11",
                cmd: "openclaw",
                args: &["acp"],
                env: &[],
                node_required: Some("22.19.0"),
            },
        },
        AgentType::Cline => AcpAgentMeta {
            agent_type,
            supports_mcp: true,
            name: "Cline",
            description: "Autonomous coding agent CLI",
            distribution: AgentDistribution::Npx {
                version: "3.0.34",
                package: "cline@3.0.34",
                cmd: "cline",
                args: &["--acp"],
                env: &[],
                node_required: None,
            },
        },
        AgentType::OpenCode => AcpAgentMeta {
            agent_type,
            supports_mcp: true,
            name: "OpenCode",
            description: "The open source coding agent",
            distribution: AgentDistribution::Binary {
                version: "1.17.13",
                cmd: "opencode",
                args: &["acp"],
                env: &[],
                platforms: &[
                    PlatformBinary {
                        platform: "darwin-aarch64",
                        url: "https://github.com/anomalyco/opencode/releases/download/v1.17.13/opencode-darwin-arm64.zip",
                    },
                    PlatformBinary {
                        platform: "darwin-x86_64",
                        url: "https://github.com/anomalyco/opencode/releases/download/v1.17.13/opencode-darwin-x64.zip",
                    },
                    PlatformBinary {
                        platform: "linux-aarch64",
                        url: "https://github.com/anomalyco/opencode/releases/download/v1.17.13/opencode-linux-arm64.tar.gz",
                    },
                    PlatformBinary {
                        platform: "linux-x86_64",
                        url: "https://github.com/anomalyco/opencode/releases/download/v1.17.13/opencode-linux-x64.tar.gz",
                    },
                    PlatformBinary {
                        platform: "windows-aarch64",
                        url: "https://github.com/anomalyco/opencode/releases/download/v1.17.13/opencode-windows-arm64.zip",
                    },
                    PlatformBinary {
                        platform: "windows-x86_64",
                        url: "https://github.com/anomalyco/opencode/releases/download/v1.17.13/opencode-windows-x64.zip",
                    },
                ],
            },
        },
        AgentType::Hermes => AcpAgentMeta {
            agent_type,
            supports_mcp: true,
            name: "Hermes Agent",
            description: "Nous Research's self-improving agent (ACP via uvx)",
            distribution: AgentDistribution::Uvx {
                version: "0.18.0",
                package: "hermes-agent[acp,mcp]==0.18.0",
                cmd: "hermes-acp",
                args: &[],
                env: &[],
                uv_required: Some("0.5.0"),
                // hermes-agent 0.18.0 is `requires-python = ">=3.11,<3.14"`, and
                // its win32 dep `pywinpty` (>=2.0.0,<3) has no Python 3.14 wheel
                // (the 2.0.15 source build fails against PyO3's 3.13 ceiling).
                // Without this pin uvx grabs the machine's default interpreter
                // (e.g. 3.14) and the install breaks; 3.13 is the newest version
                // Hermes supports.
                python: Some("3.13"),
                system_cmd: Some(("hermes", &["acp"])),
            },
        },
        AgentType::CodeBuddy => AcpAgentMeta {
            agent_type,
            supports_mcp: true,
            name: "CodeBuddy",
            description: "Tencent Cloud's official AI coding assistant (ACP)",
            distribution: AgentDistribution::Npx {
                version: "2.117.0",
                package: "@tencent-ai/codebuddy-code@2.117.0",
                cmd: "codebuddy",
                args: &["--acp"],
                env: &[],
                node_required: Some("22.0.0"),
            },
        },
        AgentType::KimiCode => AcpAgentMeta {
            agent_type,
            supports_mcp: true,
            name: "Kimi Code",
            description: "Moonshot AI's official CLI coding assistant (ACP)",
            distribution: AgentDistribution::Npx {
                version: "0.22.3",
                package: "@moonshot-ai/kimi-code@0.22.3",
                cmd: "kimi",
                args: &["acp"],
                env: &[],
                node_required: Some("22.19.0"),
            },
        },
        AgentType::Pi => AcpAgentMeta {
            agent_type,
            // pi-acp accepts ACP-wire `mcpServers` but drops them (does not
            // forward to pi), and pi has no native MCP. supports_mcp stays
            // `true` only to satisfy the `only_openclaw_opts_out_of_mcp`
            // invariant — actual wire forwarding is short-circuited in
            // `connection.rs` (see the skip-list), so neither user servers nor
            // the codeg-mcp companion are futilely forwarded.
            supports_mcp: true,
            name: "Pi",
            description: "Self-extensible coding agent (ACP via pi-acp)",
            // pi-acp 0.0.31 spawns `pi --mode rpc` as a child, so `pi` (npm
            // `@earendil-works/pi-coding-agent`) must be resolvable on PATH —
            // or pointed at a custom build via the `PI_ACP_PI_COMMAND` env
            // (see BYO-pi). Args are empty: the ACP server is the default mode
            // (`npx -y pi-acp`, no subcommand). `node_required` follows pi's
            // 22+ requirement (pi-acp's own engines say >=20). The embedded
            // context env lets pi-acp advertise `promptCapabilities.embeddedContext`.
            distribution: AgentDistribution::Npx {
                version: "0.0.31",
                package: "pi-acp@0.0.31",
                cmd: "pi-acp",
                args: &[],
                env: &[("PI_ACP_ENABLE_EMBEDDED_CONTEXT", "true")],
                node_required: Some("22.0.0"),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_npx_version(
        agent_type: AgentType,
        expected_version: &str,
        expected_package: &str,
        expected_node_required: Option<&str>,
    ) {
        let meta = get_agent_meta(agent_type);
        match meta.distribution {
            AgentDistribution::Npx {
                version,
                package,
                node_required,
                ..
            } => {
                assert_eq!(version, expected_version);
                assert_eq!(package, expected_package);
                assert_eq!(node_required, expected_node_required);
                assert_eq!(meta.registry_version(), Some(expected_version));
            }
            other => {
                panic!("expected npx distribution for {agent_type:?}, got {other:?}");
            }
        }
    }

    fn assert_uvx_version(
        agent_type: AgentType,
        expected_version: &str,
        expected_package: &str,
        expected_uv_required: Option<&str>,
        expected_python: Option<&str>,
    ) {
        let meta = get_agent_meta(agent_type);
        match meta.distribution {
            AgentDistribution::Uvx {
                version,
                package,
                uv_required,
                python,
                ..
            } => {
                assert_eq!(version, expected_version);
                assert_eq!(package, expected_package);
                assert_eq!(uv_required, expected_uv_required);
                assert_eq!(python, expected_python);
                assert_eq!(meta.registry_version(), Some(expected_version));
            }
            other => {
                panic!("expected uvx distribution for {agent_type:?}, got {other:?}");
            }
        }
    }

    fn assert_binary_version(
        agent_type: AgentType,
        expected_version: &str,
        expected_release_path: &str,
    ) {
        let meta = get_agent_meta(agent_type);
        match meta.distribution {
            AgentDistribution::Binary {
                version, platforms, ..
            } => {
                assert_eq!(version, expected_version);
                assert_eq!(meta.registry_version(), Some(expected_version));
                for platform in platforms {
                    assert!(
                        platform.url.contains(expected_release_path),
                        "{} URL did not use {expected_release_path}: {}",
                        platform.platform,
                        platform.url
                    );
                }
            }
            other => {
                panic!("expected binary distribution for {agent_type:?}, got {other:?}");
            }
        }
    }

    #[test]
    fn registry_pins_current_acp_agent_versions() {
        assert_npx_version(
            AgentType::ClaudeCode,
            "0.55.0",
            "@agentclientprotocol/claude-agent-acp@0.55.0",
            None,
        );
        assert_npx_version(
            AgentType::Gemini,
            "0.47.0",
            "@google/gemini-cli@0.47.0",
            Some("20.0.0"),
        );
        assert_npx_version(
            AgentType::OpenClaw,
            "2026.6.11",
            "openclaw@2026.6.11",
            Some("22.19.0"),
        );
        assert_npx_version(AgentType::Cline, "3.0.34", "cline@3.0.34", None);
        assert_npx_version(
            AgentType::CodeBuddy,
            "2.117.0",
            "@tencent-ai/codebuddy-code@2.117.0",
            Some("22.0.0"),
        );
        assert_npx_version(
            AgentType::KimiCode,
            "0.22.3",
            "@moonshot-ai/kimi-code@0.22.3",
            Some("22.19.0"),
        );
        assert_npx_version(
            AgentType::Codex,
            "1.1.0",
            "@agentclientprotocol/codex-acp@1.1.0",
            None,
        );
        assert_npx_version(AgentType::Pi, "0.0.31", "pi-acp@0.0.31", Some("22.0.0"));
        assert_binary_version(AgentType::OpenCode, "1.17.13", "/releases/download/v1.17.13/");
        assert_uvx_version(
            AgentType::Hermes,
            "0.18.0",
            "hermes-agent[acp,mcp]==0.18.0",
            Some("0.5.0"),
            // hermes-agent 0.18.0 is requires-python `<3.14`; uvx must pin an
            // interpreter it (and its win32 `pywinpty` dep) supports.
            Some("3.13"),
        );
    }

    // OpenClaw rejects MCP server entries inside `mcpServers` (the empty `[]`
    // field is still serialized and tolerated) and fails session/new on any
    // entry, so it must be the only agent with `supports_mcp == false`. Every
    // other agent (current and future) keeps it `true`. Iterating the full
    // registry means a newly-added agent that wrongly opts out — or a
    // regression flipping OpenClaw back on — trips this assert.
    #[test]
    fn only_openclaw_opts_out_of_mcp() {
        for agent_type in all_acp_agents() {
            let meta = get_agent_meta(agent_type);
            assert_eq!(
                meta.supports_mcp,
                agent_type != AgentType::OpenClaw,
                "unexpected supports_mcp for {agent_type:?}"
            );
        }
    }
}
