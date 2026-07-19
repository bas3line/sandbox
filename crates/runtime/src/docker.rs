use std::{collections::BTreeMap, io::Write, process::Stdio, time::Duration};

use async_trait::async_trait;
use sandbox_core::{
    SandboxId,
    config::TunnelConfig,
    model::{CommandOutput, CommandSpec, IsolationTier, NetworkMode, SandboxSpec, Tunnel},
};
use tempfile::NamedTempFile;
use tokio::{process::Command, time::timeout};

use crate::{RuntimeCapabilities, RuntimeError, SandboxRuntime, tunnel::DockerTunnelManager};

#[derive(Clone, Debug)]
pub struct DockerRuntime {
    restricted_network: String,
    tunnels: Option<DockerTunnelManager>,
    output_limit: usize,
}

impl DockerRuntime {
    #[must_use]
    pub fn new(
        restricted_network: String,
        tunnel_config: TunnelConfig,
        output_limit: usize,
    ) -> Self {
        Self {
            restricted_network,
            tunnels: DockerTunnelManager::new(tunnel_config),
            output_limit,
        }
    }

    fn name(id: SandboxId) -> String {
        format!("sandbox-{id}")
    }

    fn workspace_volume_name(id: SandboxId) -> String {
        format!("sandbox-{id}-workspace")
    }

    async fn owned_exists(&self, id: SandboxId) -> Result<bool, RuntimeError> {
        let name = Self::name(id);
        let output = Command::new("docker")
            .args([
                "inspect",
                "--format",
                "{{ index .Config.Labels \"sandbox.dev/id\" }}",
                &name,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;
        if output.status.success() {
            let owner = String::from_utf8_lossy(&output.stdout);
            if owner.trim() == id.to_string() {
                return Ok(true);
            }
            return Err(RuntimeError::Command(format!(
                "container name {} is owned by a different workload",
                name
            )));
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let normalized = stderr.to_ascii_lowercase();
        if normalized.contains("no such object") || normalized.contains("no such container") {
            Ok(false)
        } else {
            Err(RuntimeError::Command(format!(
                "docker inspect: {}",
                stderr.trim()
            )))
        }
    }

    async fn checked(
        &self,
        args: &[String],
        operation: &str,
    ) -> Result<std::process::Output, RuntimeError> {
        let output = Command::new("docker")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;
        if output.status.success() {
            Ok(output)
        } else {
            Err(RuntimeError::Command(format!(
                "docker {operation}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        }
    }

    async fn owned_workspace_volume_exists(&self, id: SandboxId) -> Result<bool, RuntimeError> {
        let name = Self::workspace_volume_name(id);
        let output = Command::new("docker")
            .args([
                "volume",
                "inspect",
                "--format",
                "{{ index .Labels \"sandbox.dev/id\" }}",
                &name,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;
        if output.status.success() {
            let owner = String::from_utf8_lossy(&output.stdout);
            if owner.trim() == id.to_string() {
                return Ok(true);
            }
            return Err(RuntimeError::Command(format!(
                "workspace volume {name} is owned by a different workload"
            )));
        }
        let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
        if stderr.contains("no such volume") {
            Ok(false)
        } else {
            Err(RuntimeError::Command(format!(
                "docker volume inspect: {}",
                stderr.trim()
            )))
        }
    }

    async fn ensure_workspace_volume(&self, id: SandboxId) -> Result<String, RuntimeError> {
        let name = Self::workspace_volume_name(id);
        if self.owned_workspace_volume_exists(id).await? {
            return Ok(name);
        }
        self.checked(
            &[
                "volume".into(),
                "create".into(),
                "--label".into(),
                format!("sandbox.dev/id={id}"),
                name.clone(),
            ],
            "volume create",
        )
        .await?;
        Ok(name)
    }

    async fn delete_workspace_volume(&self, id: SandboxId) -> Result<(), RuntimeError> {
        if !self.owned_workspace_volume_exists(id).await? {
            return Ok(());
        }
        self.checked(
            &[
                "volume".into(),
                "rm".into(),
                Self::workspace_volume_name(id),
            ],
            "volume rm",
        )
        .await?;
        Ok(())
    }

    async fn prepare_workspace(&self, id: SandboxId) -> Result<(), RuntimeError> {
        // A named volume is root-owned when an image does not already contain
        // /workspace. Keep the workload's configured non-root user, but make
        // the isolated per-sandbox workspace writable after the container is
        // started. This root exec has no added capabilities and the container
        // still has a read-only root filesystem.
        let writable_check = [
            "exec".into(),
            Self::name(id),
            "/bin/sh".into(),
            "-c".into(),
            "test -w /workspace".into(),
        ];
        let output = Command::new("docker")
            .args(&writable_check)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .await?;
        if output.status.success() {
            return Ok(());
        }
        self.checked(
            &[
                "exec".into(),
                "--user".into(),
                "0".into(),
                Self::name(id),
                "chmod".into(),
                "0777".into(),
                "/workspace".into(),
            ],
            "prepare workspace",
        )
        .await?;
        self.checked(&writable_check, "verify writable workspace")
            .await?;
        Ok(())
    }

    fn env_file(env: &BTreeMap<String, String>) -> Result<Option<NamedTempFile>, RuntimeError> {
        if env.is_empty() {
            return Ok(None);
        }
        let mut file = NamedTempFile::new()?;
        for (key, value) in env {
            if value.contains(['\n', '\r', '\0']) {
                return Err(RuntimeError::Command(format!(
                    "environment value for {key} contains a forbidden byte"
                )));
            }
            writeln!(file, "{key}={value}")?;
        }
        file.flush()?;
        Ok(Some(file))
    }

    async fn start_command(&self, id: SandboxId, argv: &[String]) -> Result<(), RuntimeError> {
        if argv.is_empty() {
            return Ok(());
        }
        let mut args = vec!["exec".into(), "--detach".into(), Self::name(id)];
        args.extend(argv.iter().cloned());
        self.checked(&args, "exec --detach").await?;
        Ok(())
    }

    async fn ensure_local_image(&self, image: &str) -> Result<(), RuntimeError> {
        if !image.ends_with(":local") {
            return Ok(());
        }
        let output = Command::new("docker")
            .args(["image", "inspect", image])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .await?;
        if output.status.success() {
            return Ok(());
        }
        let hint = image
            .strip_prefix("sandbox-agent-")
            .and_then(|name| name.strip_suffix(":local"))
            .map_or_else(
                || format!("build {image} on the selected worker"),
                |agent| {
                    format!("run `./scripts/build-agent-image.sh {agent}` on the selected worker")
                },
            );
        Err(RuntimeError::Command(format!(
            "local image {image} is missing on this worker; {hint}, or pass an immutable registry image override"
        )))
    }
}

#[async_trait]
impl SandboxRuntime for DockerRuntime {
    async fn probe(&self) -> Result<RuntimeCapabilities, RuntimeError> {
        let args = vec![
            "version".into(),
            "--format".into(),
            "{{.Server.Version}}".into(),
        ];
        let output = self.checked(&args, "version").await?;
        Ok(RuntimeCapabilities {
            name: "docker".into(),
            version: String::from_utf8_lossy(&output.stdout).trim().into(),
            tiers: vec![IsolationTier::Container],
            supports_http_tunnels: self.tunnels.is_some(),
        })
    }

    async fn create(
        &self,
        id: SandboxId,
        spec: &SandboxSpec,
        isolation: IsolationTier,
    ) -> Result<(), RuntimeError> {
        if isolation != IsolationTier::Container {
            return Err(RuntimeError::UnsupportedTier(isolation));
        }
        if self.owned_exists(id).await? {
            return Ok(());
        }
        self.ensure_local_image(&spec.image).await?;
        let env_file = Self::env_file(&spec.env)?;
        let workspace_volume = self.ensure_workspace_volume(id).await?;
        let network = match spec.network {
            NetworkMode::Deny => "none",
            NetworkMode::RestrictedEgress => &self.restricted_network,
            NetworkMode::OpenEgress => "bridge",
        };
        let mut args = vec![
            "create".into(),
            "--name".into(),
            Self::name(id),
            "--label".into(),
            format!("sandbox.dev/id={id}"),
            "--label".into(),
            format!("sandbox.dev/tenant={}", spec.tenant),
            "--cpus".into(),
            format!("{:.3}", f64::from(spec.resources.cpu_millis) / 1_000.0),
            "--memory".into(),
            format!("{}m", spec.resources.memory_mib),
            "--pids-limit".into(),
            spec.resources.pids.to_string(),
            "--network".into(),
            network.into(),
            "--read-only".into(),
            "--cap-drop".into(),
            "ALL".into(),
            "--security-opt".into(),
            "no-new-privileges:true".into(),
            "--entrypoint".into(),
            "/bin/sh".into(),
            "--workdir".into(),
            "/workspace".into(),
            "--mount".into(),
            format!("type=volume,src={workspace_volume},dst=/workspace"),
            "--tmpfs".into(),
            "/tmp:rw,noexec,nosuid,nodev,size=64m".into(),
            "--tmpfs".into(),
            "/run:rw,noexec,nosuid,nodev,size=16m".into(),
        ];
        if let Some(file) = &env_file {
            args.extend([
                "--env-file".into(),
                file.path().to_string_lossy().into_owned(),
            ]);
        }
        args.extend([
            spec.image.clone(),
            "-c".into(),
            "trap 'exit 0' TERM INT; while :; do sleep 3600 & wait $!; done".into(),
        ]);
        if let Err(error) = self.checked(&args, "create").await {
            let _cleanup = self.delete_workspace_volume(id).await;
            return Err(error);
        }
        if let Err(error) = self
            .checked(&["start".into(), Self::name(id)], "start")
            .await
        {
            let _cleanup = self.delete(id).await;
            return Err(error);
        }
        if let Err(error) = self.prepare_workspace(id).await {
            let _cleanup = self.delete(id).await;
            return Err(error);
        }
        if let Err(error) = self.start_command(id, &spec.command).await {
            let _cleanup = self.delete(id).await;
            return Err(error);
        }
        Ok(())
    }

    async fn exec(
        &self,
        id: SandboxId,
        command: &CommandSpec,
    ) -> Result<CommandOutput, RuntimeError> {
        let mut args = vec!["exec".into()];
        if let Some(cwd) = &command.cwd {
            args.extend(["--workdir".into(), cwd.clone()]);
        }
        for (key, value) in &command.env {
            args.extend(["--env".into(), format!("{key}={value}")]);
        }
        args.push(Self::name(id));
        args.extend(command.argv.iter().cloned());

        let future = Command::new("docker")
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();
        let output = timeout(Duration::from_secs(command.timeout_seconds), future)
            .await
            .map_err(|_| RuntimeError::Timeout(command.timeout_seconds))??;
        let (stdout, stdout_cut) = truncate(&output.stdout, self.output_limit);
        let (stderr, stderr_cut) = truncate(&output.stderr, self.output_limit);
        Ok(CommandOutput {
            exit_code: output.status.code().unwrap_or(128),
            stdout,
            stderr,
            truncated: stdout_cut || stderr_cut,
        })
    }

    async fn expose(&self, id: SandboxId, tunnel: &Tunnel) -> Result<(), RuntimeError> {
        self.tunnels
            .as_ref()
            .ok_or(RuntimeError::TunnelsDisabled)?
            .expose(id, tunnel)
            .await
    }

    async fn unexpose(&self, id: SandboxId, tunnel: &Tunnel) -> Result<(), RuntimeError> {
        self.tunnels
            .as_ref()
            .ok_or(RuntimeError::TunnelsDisabled)?
            .unexpose(id, tunnel)
            .await
    }

    async fn delete(&self, id: SandboxId) -> Result<(), RuntimeError> {
        if let Some(tunnels) = &self.tunnels {
            tunnels.cleanup(id).await?;
        }
        if self.owned_exists(id).await? {
            self.checked(
                &["rm".into(), "--force".into(), Self::name(id)],
                "rm --force",
            )
            .await?;
        }
        self.delete_workspace_volume(id).await?;
        Ok(())
    }
}

fn truncate(bytes: &[u8], limit: usize) -> (String, bool) {
    let truncated = bytes.len() > limit;
    let bytes = &bytes[..bytes.len().min(limit)];
    (String::from_utf8_lossy(bytes).into_owned(), truncated)
}

#[cfg(test)]
mod tests {
    use super::{DockerRuntime, truncate};
    use sandbox_core::SandboxId;

    #[test]
    fn workspace_volumes_are_scoped_to_the_sandbox() {
        let id = SandboxId::new();
        assert_eq!(
            DockerRuntime::workspace_volume_name(id),
            format!("sandbox-{id}-workspace")
        );
    }

    #[test]
    fn output_truncation_reports_when_data_was_cut() {
        assert_eq!(truncate(b"abcdef", 3), ("abc".into(), true));
        assert_eq!(truncate(b"abc", 3), ("abc".into(), false));
    }
}
