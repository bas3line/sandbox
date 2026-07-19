use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{
    DataSensitivity, IsolationPreference, NetworkMode, ResourceSpec, SandboxSpec, WorkloadSignals,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentProfile {
    pub name: String,
    pub display_name: String,
    pub executable: String,
    pub default_image: String,
    #[serde(default)]
    pub default_args: Vec<String>,
    #[serde(default)]
    pub env_passthrough: Vec<String>,
    pub homepage: String,
}

impl AgentProfile {
    #[must_use]
    pub fn sandbox_spec(&self, tenant: String, args: Vec<String>, ttl_seconds: u64) -> SandboxSpec {
        let mut command = vec![self.executable.clone()];
        command.extend(self.default_args.clone());
        command.extend(args);
        SandboxSpec {
            tenant,
            image: self.default_image.clone(),
            command,
            env: BTreeMap::new(),
            resources: ResourceSpec {
                cpu_millis: 2_000,
                memory_mib: 4_096,
                disk_mib: 30_720,
                pids: 512,
            },
            network: NetworkMode::RestrictedEgress,
            isolation: IsolationPreference::Auto,
            sensitivity: DataSensitivity::Confidential,
            signals: WorkloadSignals {
                untrusted_repository: true,
                executes_generated_code: true,
                needs_secrets: true,
                host_mounts: false,
                privileged: false,
            },
            ttl_seconds,
            labels: BTreeMap::from([("sandbox.dev/agent".into(), self.name.clone())]),
            placement: Default::default(),
            exposures: Vec::new(),
            agent: Some(self.name.clone()),
        }
    }
}

#[must_use]
pub fn built_in_agent_profiles() -> Vec<AgentProfile> {
    [
        (
            "codex",
            "OpenAI Codex",
            "codex",
            "sandbox-agent-codex:local",
            "https://github.com/openai/codex",
        ),
        (
            "claude",
            "Claude Code",
            "claude",
            "sandbox-agent-claude:local",
            "https://github.com/anthropics/claude-code",
        ),
        (
            "opencode",
            "OpenCode",
            "opencode",
            "sandbox-agent-opencode:local",
            "https://github.com/anomalyco/opencode",
        ),
        (
            "pi",
            "Pi",
            "pi",
            "sandbox-agent-pi:local",
            "https://github.com/badlogic/pi-mono",
        ),
        (
            "aider",
            "Aider",
            "aider",
            "paulgauthier/aider:latest",
            "https://github.com/Aider-AI/aider",
        ),
        (
            "goose",
            "Goose",
            "goose",
            "ghcr.io/block/goose:latest",
            "https://github.com/block/goose",
        ),
        (
            "commandcode",
            "CommandCode",
            "commandcode",
            "sandbox-agent-commandcode:local",
            "https://github.com/bas3line",
        ),
    ]
    .into_iter()
    .map(
        |(name, display_name, executable, image, homepage)| AgentProfile {
            name: name.into(),
            display_name: display_name.into(),
            executable: executable.into(),
            default_image: image.into(),
            default_args: Vec::new(),
            env_passthrough: Vec::new(),
            homepage: homepage.into(),
        },
    )
    .collect()
}

#[must_use]
pub fn find_agent_profile(name: &str) -> Option<AgentProfile> {
    built_in_agent_profiles()
        .into_iter()
        .find(|profile| profile.name == name)
}
