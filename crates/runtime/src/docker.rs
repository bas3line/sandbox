use std::{collections::BTreeMap, io::Write, process::Stdio, time::Duration};

use async_trait::async_trait;
use sandbox_core::{
    SandboxId,
    model::{CommandOutput, CommandSpec, IsolationTier, NetworkMode, SandboxSpec},
};
use tempfile::NamedTempFile;
use tokio::{process::Command, time::timeout};

use crate::{RuntimeCapabilities, RuntimeError, SandboxRuntime};

#[derive(Clone, Debug)]
pub struct DockerRuntime {
    restricted_network: String,
    output_limit: usize,
}

impl DockerRuntime {
    #[must_use]
    pub fn new(restricted_network: String, output_limit: usize) -> Self {
        Self {
            restricted_network,
            output_limit,
        }
    }

    fn name(id: SandboxId) -> String {
        format!("sandbox-{id}")
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
        let env_file = Self::env_file(&spec.env)?;
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
        self.checked(&args, "create").await?;
        if let Err(error) = self
            .checked(&["start".into(), Self::name(id)], "start")
            .await
        {
            let _cleanup = self.delete(id).await;
            return Err(error);
        }
        self.start_command(id, &spec.command).await
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

    async fn delete(&self, id: SandboxId) -> Result<(), RuntimeError> {
        if !self.owned_exists(id).await? {
            return Ok(());
        }
        self.checked(
            &["rm".into(), "--force".into(), Self::name(id)],
            "rm --force",
        )
        .await?;
        Ok(())
    }
}

fn truncate(bytes: &[u8], limit: usize) -> (String, bool) {
    let truncated = bytes.len() > limit;
    let bytes = &bytes[..bytes.len().min(limit)];
    (String::from_utf8_lossy(bytes).into_owned(), truncated)
}
