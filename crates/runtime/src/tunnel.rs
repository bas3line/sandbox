use std::{fs, io::Write, path::PathBuf, process::Stdio};

use sandbox_core::{
    SandboxId, TunnelId,
    config::TunnelConfig,
    model::{ExposureProtocol, Tunnel},
};
use tempfile::NamedTempFile;
use tokio::process::Command;

use crate::RuntimeError;

#[derive(Clone, Debug)]
pub(crate) struct DockerTunnelManager {
    config: TunnelConfig,
}

impl DockerTunnelManager {
    pub(crate) fn new(config: TunnelConfig) -> Option<Self> {
        config.enabled.then_some(Self { config })
    }

    pub(crate) async fn expose(
        &self,
        sandbox_id: SandboxId,
        tunnel: &Tunnel,
    ) -> Result<(), RuntimeError> {
        if tunnel.protocol != ExposureProtocol::Http {
            return Err(RuntimeError::Tunnel(
                "the Docker tunnel edge currently supports HTTP and WebSocket services only".into(),
            ));
        }
        self.require_edge().await?;
        self.ensure_network(sandbox_id).await?;
        let network = self.network_name(sandbox_id);
        let sandbox = sandbox_name(sandbox_id);
        if self.uses_none_mode(&sandbox).await? {
            self.disconnect("none", &sandbox).await?;
        }
        if let Err(error) = self
            .connect(&network, &sandbox, Some(sandbox.as_str()))
            .await
        {
            let _cleanup = self.teardown_network(sandbox_id).await;
            return Err(error);
        }
        if let Err(error) = self
            .connect(&network, &self.config.edge_container, None)
            .await
        {
            let _cleanup = self.teardown_network(sandbox_id).await;
            return Err(error);
        }
        if let Err(error) = self.write_route(sandbox_id, tunnel) {
            let _cleanup = self.unexpose(sandbox_id, tunnel).await;
            return Err(error);
        }
        Ok(())
    }

    pub(crate) async fn unexpose(
        &self,
        sandbox_id: SandboxId,
        tunnel: &Tunnel,
    ) -> Result<(), RuntimeError> {
        remove_file_if_present(self.route_path(sandbox_id, tunnel.id))?;
        if !self.has_routes(sandbox_id)? {
            self.teardown_network(sandbox_id).await?;
        }
        Ok(())
    }

    pub(crate) async fn cleanup(&self, sandbox_id: SandboxId) -> Result<(), RuntimeError> {
        let directory = PathBuf::from(&self.config.config_dir);
        if let Ok(entries) = fs::read_dir(&directory) {
            let prefix = route_prefix(sandbox_id);
            for entry in entries {
                let entry = entry.map_err(tunnel_io)?;
                if entry.file_name().to_string_lossy().starts_with(&prefix) {
                    remove_file_if_present(entry.path())?;
                }
            }
        }
        self.teardown_network(sandbox_id).await
    }

    fn write_route(&self, sandbox_id: SandboxId, tunnel: &Tunnel) -> Result<(), RuntimeError> {
        let directory = PathBuf::from(&self.config.config_dir);
        fs::create_dir_all(&directory).map_err(tunnel_io)?;
        let route_name = format!("tunnel-{}", compact_id(tunnel.id));
        let tls = if self.config.edge_tls {
            self.config.edge_cert_resolver.as_ref().map_or_else(
                || "      tls: {}\n".to_owned(),
                |resolver| format!("      tls:\n        certResolver: {resolver}\n"),
            )
        } else {
            String::new()
        };
        let body = format!(
            "http:\n  routers:\n    {route_name}:\n      rule: \"Host(`{hostname}`)\"\n      entryPoints:\n        - {entrypoint}\n      service: {route_name}\n{tls}  services:\n    {route_name}:\n      loadBalancer:\n        passHostHeader: true\n        servers:\n          - url: \"http://{sandbox}:{port}\"\n",
            hostname = tunnel.hostname,
            entrypoint = self.config.edge_entrypoint,
            sandbox = sandbox_name(sandbox_id),
            port = tunnel.container_port,
        );
        let mut temporary = NamedTempFile::new_in(&directory).map_err(tunnel_io)?;
        temporary.write_all(body.as_bytes()).map_err(tunnel_io)?;
        temporary.flush().map_err(tunnel_io)?;
        temporary
            .persist(self.route_path(sandbox_id, tunnel.id))
            .map_err(|error| tunnel_io(error.error))?;
        Ok(())
    }

    fn has_routes(&self, sandbox_id: SandboxId) -> Result<bool, RuntimeError> {
        let directory = PathBuf::from(&self.config.config_dir);
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(tunnel_io(error)),
        };
        let prefix = route_prefix(sandbox_id);
        for entry in entries {
            let entry = entry.map_err(tunnel_io)?;
            if entry.file_name().to_string_lossy().starts_with(&prefix) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn require_edge(&self) -> Result<(), RuntimeError> {
        self.checked(
            &["inspect".into(), self.config.edge_container.clone()],
            "inspect tunnel edge",
        )
        .await?;
        Ok(())
    }

    async fn ensure_network(&self, sandbox_id: SandboxId) -> Result<(), RuntimeError> {
        let network = self.network_name(sandbox_id);
        let inspect = self
            .output(&[
                "network".into(),
                "inspect".into(),
                "--format".into(),
                "{{ index .Labels \"sandbox.dev/id\" }}".into(),
                network.clone(),
            ])
            .await?;
        if inspect.status.success() {
            let owner = String::from_utf8_lossy(&inspect.stdout);
            if owner.trim() == sandbox_id.to_string() {
                return Ok(());
            }
            return Err(RuntimeError::Tunnel(format!(
                "Docker network {network} is owned by a different sandbox"
            )));
        }
        if !is_missing(&inspect.stderr) {
            return Err(command_error("network inspect", &inspect.stderr));
        }
        self.checked(
            &[
                "network".into(),
                "create".into(),
                "--internal".into(),
                "--label".into(),
                format!("sandbox.dev/id={sandbox_id}"),
                network,
            ],
            "network create",
        )
        .await?;
        Ok(())
    }

    async fn connect(
        &self,
        network: &str,
        container: &str,
        alias: Option<&str>,
    ) -> Result<(), RuntimeError> {
        let mut args = vec!["network".into(), "connect".into()];
        if let Some(alias) = alias {
            args.extend(["--alias".into(), alias.into()]);
        }
        args.extend([network.into(), container.into()]);
        let output = self.output(&args).await?;
        if output.status.success() || contains(&output.stderr, "already exists") {
            Ok(())
        } else {
            Err(command_error("network connect", &output.stderr))
        }
    }

    async fn teardown_network(&self, sandbox_id: SandboxId) -> Result<(), RuntimeError> {
        let network = self.network_name(sandbox_id);
        self.disconnect(&network, &self.config.edge_container)
            .await?;
        self.disconnect(&network, &sandbox_name(sandbox_id)).await?;
        let output = self
            .output(&["network".into(), "rm".into(), network])
            .await?;
        if output.status.success() || is_missing(&output.stderr) {
            if self.uses_none_mode(&sandbox_name(sandbox_id)).await? {
                self.connect("none", &sandbox_name(sandbox_id), None)
                    .await?;
            }
            Ok(())
        } else {
            Err(command_error("network rm", &output.stderr))
        }
    }

    async fn uses_none_mode(&self, container: &str) -> Result<bool, RuntimeError> {
        let output = self
            .output(&[
                "inspect".into(),
                "--format".into(),
                "{{.HostConfig.NetworkMode}}".into(),
                container.into(),
            ])
            .await?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).trim() == "none");
        }
        if is_missing(&output.stderr) {
            return Ok(false);
        }
        Err(command_error("inspect network mode", &output.stderr))
    }

    async fn disconnect(&self, network: &str, container: &str) -> Result<(), RuntimeError> {
        let output = self
            .output(&[
                "network".into(),
                "disconnect".into(),
                "--force".into(),
                network.into(),
                container.into(),
            ])
            .await?;
        if output.status.success()
            || is_missing(&output.stderr)
            || contains(&output.stderr, "is not connected")
        {
            Ok(())
        } else {
            Err(command_error("network disconnect", &output.stderr))
        }
    }

    async fn checked(
        &self,
        args: &[String],
        operation: &str,
    ) -> Result<std::process::Output, RuntimeError> {
        let output = self.output(args).await?;
        if output.status.success() {
            Ok(output)
        } else {
            Err(command_error(operation, &output.stderr))
        }
    }

    async fn output(&self, args: &[String]) -> Result<std::process::Output, RuntimeError> {
        Command::new("docker")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(Into::into)
    }

    fn network_name(&self, sandbox_id: SandboxId) -> String {
        format!("{}-{sandbox_id}", self.config.docker_network_prefix)
    }

    fn route_path(&self, sandbox_id: SandboxId, tunnel_id: TunnelId) -> PathBuf {
        PathBuf::from(&self.config.config_dir).join(format!(
            "{}{}.yml",
            route_prefix(sandbox_id),
            compact_id(tunnel_id)
        ))
    }
}

fn route_prefix(sandbox_id: SandboxId) -> String {
    format!("sandbox-{}-", compact_uuid(sandbox_id.to_string()))
}

fn compact_id(id: TunnelId) -> String {
    compact_uuid(id.to_string())
}

fn compact_uuid(value: String) -> String {
    value.replace('-', "")
}

fn sandbox_name(id: SandboxId) -> String {
    format!("sandbox-{id}")
}

fn remove_file_if_present(path: PathBuf) -> Result<(), RuntimeError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(tunnel_io(error)),
    }
}

fn is_missing(stderr: &[u8]) -> bool {
    contains(stderr, "no such") || contains(stderr, "not found")
}

fn contains(stderr: &[u8], needle: &str) -> bool {
    String::from_utf8_lossy(stderr)
        .to_ascii_lowercase()
        .contains(needle)
}

fn command_error(operation: &str, stderr: &[u8]) -> RuntimeError {
    RuntimeError::Tunnel(format!(
        "docker {operation}: {}",
        String::from_utf8_lossy(stderr).trim()
    ))
}

fn tunnel_io(error: std::io::Error) -> RuntimeError {
    RuntimeError::Tunnel(error.to_string())
}

#[cfg(test)]
mod tests {
    use sandbox_core::{
        SandboxId, TunnelId,
        config::TunnelConfig,
        model::{ExposureProtocol, Tunnel, TunnelState},
    };

    use super::DockerTunnelManager;

    #[test]
    fn route_is_host_exact_and_uses_configured_backend_port() {
        let root = tempfile::tempdir().expect("temporary tunnel directory");
        let config = TunnelConfig {
            enabled: true,
            base_domain: Some("tunnel.example.com".into()),
            config_dir: root.path().to_string_lossy().into_owned(),
            edge_tls: false,
            ..TunnelConfig::default()
        };
        let manager = DockerTunnelManager::new(config).expect("enabled manager");
        let sandbox_id = SandboxId::new();
        let tunnel = Tunnel {
            id: TunnelId::new(),
            container_port: 8_000,
            protocol: ExposureProtocol::Http,
            subdomain: "demo".into(),
            hostname: "demo.tunnel.example.com".into(),
            public_url: "https://demo.tunnel.example.com".into(),
            authenticated: false,
            state: TunnelState::Pending,
            failure: None,
        };
        manager
            .write_route(sandbox_id, &tunnel)
            .expect("write route");
        manager
            .write_route(sandbox_id, &tunnel)
            .expect("replace route atomically");
        let route =
            std::fs::read_to_string(manager.route_path(sandbox_id, tunnel.id)).expect("read route");
        assert!(route.contains("Host(`demo.tunnel.example.com`)"));
        assert!(route.contains(&format!("http://sandbox-{sandbox_id}:8000")));
        assert!(!route.contains("certResolver"));
    }

    #[test]
    fn route_can_enable_the_configured_tls_resolver() {
        let root = tempfile::tempdir().expect("temporary tunnel directory");
        let config = TunnelConfig {
            enabled: true,
            base_domain: Some("tunnel.example.com".into()),
            config_dir: root.path().to_string_lossy().into_owned(),
            edge_tls: true,
            edge_cert_resolver: Some("production".into()),
            ..TunnelConfig::default()
        };
        let manager = DockerTunnelManager::new(config).expect("enabled manager");
        let sandbox_id = SandboxId::new();
        let tunnel = Tunnel {
            id: TunnelId::new(),
            container_port: 3_000,
            protocol: ExposureProtocol::Http,
            subdomain: "review".into(),
            hostname: "review.tunnel.example.com".into(),
            public_url: "https://review.tunnel.example.com".into(),
            authenticated: false,
            state: TunnelState::Pending,
            failure: None,
        };
        manager
            .write_route(sandbox_id, &tunnel)
            .expect("write TLS route");
        let route =
            std::fs::read_to_string(manager.route_path(sandbox_id, tunnel.id)).expect("read route");
        assert!(route.contains("certResolver: production"));
        assert!(route.contains("- websecure"));
    }
}
