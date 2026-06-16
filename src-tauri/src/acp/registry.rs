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
        /// The `uvx --from` package spec, e.g. "hermes-agent[acp,mcp]==0.16.0".
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
            name: "Claude Code",
            description: "ACP wrapper for Anthropic's Claude",
            distribution: AgentDistribution::Npx {
                version: "0.45.0",
                package: "@agentclientprotocol/claude-agent-acp@0.45.0",
                cmd: "claude-agent-acp",
                args: &[],
                env: &[],
                node_required: None,
            },
        },
        AgentType::Codex => AcpAgentMeta {
            agent_type,
            name: "Codex CLI",
            description: "ACP adapter for OpenAI's coding assistant",
            distribution: AgentDistribution::Binary {
                version: "0.16.0",
                cmd: "codex-acp",
                args: &[],
                env: &[],
                platforms: &[
                    PlatformBinary {
                        platform: "darwin-aarch64",
                        url: "https://github.com/zed-industries/codex-acp/releases/download/v0.16.0/codex-acp-0.16.0-aarch64-apple-darwin.tar.gz",
                    },
                    PlatformBinary {
                        platform: "darwin-x86_64",
                        url: "https://github.com/zed-industries/codex-acp/releases/download/v0.16.0/codex-acp-0.16.0-x86_64-apple-darwin.tar.gz",
                    },
                    PlatformBinary {
                        platform: "linux-aarch64",
                        url: "https://github.com/zed-industries/codex-acp/releases/download/v0.16.0/codex-acp-0.16.0-aarch64-unknown-linux-gnu.tar.gz",
                    },
                    PlatformBinary {
                        platform: "linux-x86_64",
                        url: "https://github.com/zed-industries/codex-acp/releases/download/v0.16.0/codex-acp-0.16.0-x86_64-unknown-linux-gnu.tar.gz",
                    },
                    PlatformBinary {
                        platform: "windows-aarch64",
                        url: "https://github.com/zed-industries/codex-acp/releases/download/v0.16.0/codex-acp-0.16.0-aarch64-pc-windows-msvc.zip",
                    },
                    PlatformBinary {
                        platform: "windows-x86_64",
                        url: "https://github.com/zed-industries/codex-acp/releases/download/v0.16.0/codex-acp-0.16.0-x86_64-pc-windows-msvc.zip",
                    },
                ],
            },
        },
        AgentType::Gemini => AcpAgentMeta {
            agent_type,
            name: "Gemini CLI",
            description: "Google's official CLI for Gemini",
            distribution: AgentDistribution::Npx {
                version: "0.46.0",
                package: "@google/gemini-cli@0.46.0",
                cmd: "gemini",
                args: &["--acp", "--skip-trust"],
                env: &[],
                node_required: Some("20.0.0"),
            },
        },
        AgentType::OpenClaw => AcpAgentMeta {
            agent_type,
            name: "OpenClaw",
            description: "OpenClaw is a personal AI assistant you run on your own devices.",
            distribution: AgentDistribution::Npx {
                version: "2026.6.1",
                package: "openclaw@2026.6.1",
                cmd: "openclaw",
                args: &["acp"],
                env: &[],
                node_required: Some("22.19.0"),
            },
        },
        AgentType::Cline => AcpAgentMeta {
            agent_type,
            name: "Cline",
            description: "Autonomous coding agent CLI",
            distribution: AgentDistribution::Npx {
                version: "3.0.9",
                package: "cline@3.0.9",
                cmd: "cline",
                args: &["--acp"],
                env: &[],
                node_required: None,
            },
        },
        AgentType::OpenCode => AcpAgentMeta {
            agent_type,
            name: "OpenCode",
            description: "The open source coding agent",
            distribution: AgentDistribution::Binary {
                version: "1.17.7",
                cmd: "opencode",
                args: &["acp"],
                env: &[],
                platforms: &[
                    PlatformBinary {
                        platform: "darwin-aarch64",
                        url: "https://github.com/anomalyco/opencode/releases/download/v1.17.7/opencode-darwin-arm64.zip",
                    },
                    PlatformBinary {
                        platform: "darwin-x86_64",
                        url: "https://github.com/anomalyco/opencode/releases/download/v1.17.7/opencode-darwin-x64.zip",
                    },
                    PlatformBinary {
                        platform: "linux-aarch64",
                        url: "https://github.com/anomalyco/opencode/releases/download/v1.17.7/opencode-linux-arm64.tar.gz",
                    },
                    PlatformBinary {
                        platform: "linux-x86_64",
                        url: "https://github.com/anomalyco/opencode/releases/download/v1.17.7/opencode-linux-x64.tar.gz",
                    },
                    PlatformBinary {
                        platform: "windows-aarch64",
                        url: "https://github.com/anomalyco/opencode/releases/download/v1.17.7/opencode-windows-arm64.zip",
                    },
                    PlatformBinary {
                        platform: "windows-x86_64",
                        url: "https://github.com/anomalyco/opencode/releases/download/v1.17.7/opencode-windows-x64.zip",
                    },
                ],
            },
        },
        AgentType::Hermes => AcpAgentMeta {
            agent_type,
            name: "Hermes Agent",
            description: "Nous Research's self-improving agent (ACP via uvx)",
            distribution: AgentDistribution::Uvx {
                version: "0.16.0",
                package: "hermes-agent[acp,mcp]==0.16.0",
                cmd: "hermes-acp",
                args: &[],
                env: &[],
                uv_required: Some("0.5.0"),
                // hermes-agent 0.16.0 is `requires-python = ">=3.11,<3.14"`, and
                // its win32 dep `pywinpty` (>=2.0.0,<3) has no Python 3.14 wheel
                // (the 2.0.15 source build fails against PyO3's 3.13 ceiling).
                // Without this pin uvx grabs the machine's default interpreter
                // (e.g. 3.14) and the install breaks; 3.13 is the newest version
                // Hermes supports.
                python: Some("3.13"),
                system_cmd: Some(("hermes", &["acp"])),
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
            "0.45.0",
            "@agentclientprotocol/claude-agent-acp@0.45.0",
            None,
        );
        assert_npx_version(
            AgentType::Gemini,
            "0.46.0",
            "@google/gemini-cli@0.46.0",
            Some("20.0.0"),
        );
        assert_npx_version(
            AgentType::OpenClaw,
            "2026.6.1",
            "openclaw@2026.6.1",
            Some("22.19.0"),
        );
        assert_npx_version(AgentType::Cline, "3.0.9", "cline@3.0.9", None);
        assert_binary_version(AgentType::Codex, "0.16.0", "/releases/download/v0.16.0/");
        assert_binary_version(AgentType::OpenCode, "1.17.7", "/releases/download/v1.17.7/");
        assert_uvx_version(
            AgentType::Hermes,
            "0.16.0",
            "hermes-agent[acp,mcp]==0.16.0",
            Some("0.5.0"),
            // hermes-agent 0.16.0 is requires-python `<3.14`; uvx must pin an
            // interpreter it (and its win32 `pywinpty` dep) supports.
            Some("3.13"),
        );
    }
}
