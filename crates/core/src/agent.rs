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
    pub default_image: Option<String>,
    #[serde(default)]
    pub default_args: Vec<String>,
    #[serde(default)]
    pub env_passthrough: Vec<String>,
    pub homepage: String,
}

impl AgentProfile {
    #[must_use]
    pub fn command_argv(&self, args: Vec<String>) -> Vec<String> {
        let mut command = vec![self.executable.clone()];
        command.extend(self.default_args.clone());
        command.extend(args);
        command
    }

    #[must_use]
    pub fn sandbox_spec(&self, tenant: String, image: String, ttl_seconds: u64) -> SandboxSpec {
        SandboxSpec {
            tenant,
            image,
            // Agent commands run through the normal exec operation so their
            // output, exit code, timeout, and truncation state stay observable.
            command: Vec::new(),
            env: BTreeMap::from([
                ("HOME".into(), "/workspace/.home".into()),
                ("XDG_CACHE_HOME".into(), "/workspace/.cache".into()),
                ("XDG_CONFIG_HOME".into(), "/workspace/.config".into()),
                ("XDG_DATA_HOME".into(), "/workspace/.local/share".into()),
            ]),
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
            Some("sandbox-agent-codex:local"),
            "https://github.com/openai/codex",
        ),
        (
            "claude",
            "Claude Code",
            "claude",
            Some("sandbox-agent-claude:local"),
            "https://github.com/anthropics/claude-code",
        ),
        (
            "opencode",
            "OpenCode",
            "opencode",
            Some("sandbox-agent-opencode:local"),
            "https://github.com/anomalyco/opencode",
        ),
        (
            "pi",
            "Pi",
            "pi",
            Some("sandbox-agent-pi:local"),
            "https://github.com/badlogic/pi-mono",
        ),
        (
            "aider",
            "Aider",
            "aider",
            Some("paulgauthier/aider:latest"),
            "https://github.com/Aider-AI/aider",
        ),
        (
            "goose",
            "Goose",
            "goose",
            Some("ghcr.io/block/goose:latest"),
            "https://github.com/block/goose",
        ),
        (
            "commandcode",
            "CommandCode",
            "commandcode",
            None,
            "https://github.com/bas3line",
        ),
    ]
    .into_iter()
    .map(
        |(name, display_name, executable, image, homepage)| AgentProfile {
            name: name.into(),
            display_name: display_name.into(),
            executable: executable.into(),
            default_image: image.map(str::to_owned),
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

#[cfg(test)]
mod tests {
    use super::{built_in_agent_profiles, find_agent_profile};

    #[test]
    fn commandcode_requires_an_operator_image() {
        let profile = find_agent_profile("commandcode").expect("commandcode profile");
        assert!(profile.default_image.is_none());
    }

    #[test]
    fn agent_commands_are_observable_execs_not_detached_startup_commands() {
        let profile = find_agent_profile("opencode").expect("opencode profile");
        let spec = profile.sandbox_spec(
            "test".into(),
            profile.default_image.clone().expect("default image"),
            300,
        );
        assert!(spec.command.is_empty());
        assert_eq!(
            spec.env.get("HOME").map(String::as_str),
            Some("/workspace/.home")
        );
        assert_eq!(
            profile.command_argv(vec!["--version".into()]),
            ["opencode", "--version"]
        );
        assert_eq!(built_in_agent_profiles().len(), 7);
    }
}
