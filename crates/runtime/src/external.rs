use std::{path::PathBuf, process::Stdio, time::Duration};

use async_trait::async_trait;
use sandbox_core::{
    SandboxId,
    model::{CommandOutput, CommandSpec, IsolationTier, SandboxSpec},
};
use serde::{Deserialize, Serialize};
use tokio::{io::AsyncWriteExt, process::Command, time::timeout};

use crate::{RuntimeCapabilities, RuntimeError, SandboxRuntime};

#[derive(Clone, Debug)]
pub struct ExternalRuntime {
    driver: PathBuf,
    output_limit: usize,
}

#[derive(Serialize)]
struct DriverRequest<'a, T> {
    protocol_version: &'static str,
    sandbox_id: SandboxId,
    payload: &'a T,
}

#[derive(Deserialize)]
struct DriverResponse<T> {
    success: bool,
    result: Option<T>,
    error: Option<String>,
}

impl ExternalRuntime {
    #[must_use]
    pub fn new(driver: impl Into<PathBuf>, output_limit: usize) -> Self {
        Self {
            driver: driver.into(),
            output_limit,
        }
    }

    async fn call<I: Serialize, O: for<'de> Deserialize<'de>>(
        &self,
        action: &str,
        id: SandboxId,
        input: &I,
        timeout_seconds: u64,
    ) -> Result<O, RuntimeError> {
        let request = DriverRequest {
            protocol_version: "sandbox.runtime.v1",
            sandbox_id: id,
            payload: input,
        };
        let body = serde_json::to_vec(&request)?;
        let mut child = Command::new(&self.driver)
            .arg(action)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| RuntimeError::Command("driver stdin was unavailable".into()))?;
        stdin.write_all(&body).await?;
        drop(stdin);
        let output = timeout(
            Duration::from_secs(timeout_seconds),
            child.wait_with_output(),
        )
        .await
        .map_err(|_| RuntimeError::Timeout(timeout_seconds))??;
        if output.stdout.len() > self.output_limit {
            return Err(RuntimeError::Command(
                "external driver response exceeded output limit".into(),
            ));
        }
        if !output.status.success() {
            return Err(RuntimeError::Command(
                String::from_utf8_lossy(&output.stderr).trim().into(),
            ));
        }
        let response: DriverResponse<O> = serde_json::from_slice(&output.stdout)?;
        if response.success {
            response
                .result
                .ok_or_else(|| RuntimeError::Command("driver omitted result".into()))
        } else {
            Err(RuntimeError::Command(
                response
                    .error
                    .unwrap_or_else(|| "driver failed without an error".into()),
            ))
        }
    }
}

#[derive(Serialize)]
struct CreatePayload<'a> {
    spec: &'a SandboxSpec,
    isolation: IsolationTier,
}

#[derive(Serialize)]
struct EmptyPayload;

#[derive(Deserialize)]
struct EmptyResult {}

#[async_trait]
impl SandboxRuntime for ExternalRuntime {
    async fn probe(&self) -> Result<RuntimeCapabilities, RuntimeError> {
        self.call("probe", SandboxId::new(), &EmptyPayload, 10)
            .await
    }

    async fn create(
        &self,
        id: SandboxId,
        spec: &SandboxSpec,
        isolation: IsolationTier,
    ) -> Result<(), RuntimeError> {
        let _result: EmptyResult = self
            .call("create", id, &CreatePayload { spec, isolation }, 120)
            .await?;
        Ok(())
    }

    async fn exec(
        &self,
        id: SandboxId,
        command: &CommandSpec,
    ) -> Result<CommandOutput, RuntimeError> {
        self.call(
            "exec",
            id,
            command,
            command.timeout_seconds.saturating_add(5),
        )
        .await
    }

    async fn delete(&self, id: SandboxId) -> Result<(), RuntimeError> {
        let _result: EmptyResult = self.call("delete", id, &EmptyPayload, 60).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf};

    use sandbox_core::model::{
        CommandSpec, DataSensitivity, IsolationPreference, IsolationTier, NetworkMode,
        ResourceSpec, SandboxSpec, WorkloadSignals,
    };

    use super::{ExternalRuntime, SandboxRuntime};

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/fake-runtime-driver.sh")
    }

    fn spec() -> SandboxSpec {
        SandboxSpec {
            tenant: "test".into(),
            image: "fixture:latest".into(),
            command: Vec::new(),
            env: BTreeMap::new(),
            resources: ResourceSpec::default(),
            network: NetworkMode::Deny,
            isolation: IsolationPreference::Microvm,
            sensitivity: DataSensitivity::Internal,
            signals: WorkloadSignals::default(),
            ttl_seconds: 60,
            labels: BTreeMap::new(),
            placement: Default::default(),
            exposures: Vec::new(),
            agent: None,
        }
    }

    #[tokio::test]
    async fn external_protocol_handles_unit_and_output_results() {
        let runtime = ExternalRuntime::new(fixture(), 4_096);
        let capabilities = runtime.probe().await;
        assert!(matches!(capabilities, Ok(value) if value.tiers.contains(&IsolationTier::Microvm)));
        let id = sandbox_core::SandboxId::new();
        assert!(
            runtime
                .create(id, &spec(), IsolationTier::Microvm)
                .await
                .is_ok()
        );
        let output = runtime
            .exec(
                id,
                &CommandSpec {
                    argv: vec!["true".into()],
                    cwd: None,
                    env: BTreeMap::new(),
                    timeout_seconds: 5,
                },
            )
            .await;
        assert!(
            matches!(output, Ok(value) if value.exit_code == 0 && value.stdout.contains("fake runtime ok"))
        );
        assert!(runtime.delete(id).await.is_ok());
    }
}
