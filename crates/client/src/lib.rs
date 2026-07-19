//! Typed client shared by the CLI, worker daemon, and MCP server.

use reqwest::{Method, StatusCode};
use sandbox_core::{
    NodeId, OperationId, SandboxId,
    api::{
        ApiErrorBody, CompleteAssignmentRequest, CreateSandboxRequest, CreateSandboxResponse,
        ExecSandboxRequest, HealthResponse, HeartbeatRequest, LeaseAssignmentsResponse,
        ListSandboxesResponse, OperationResponse, RegisterNodeRequest, RegisterNodeResponse,
    },
    model::{NodeRecord, Operation, Sandbox, SandboxSpec},
};
use secrecy::{ExposeSecret, SecretString};
use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("HTTP client error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("invalid URL: {0}")]
    Url(#[from] url::ParseError),
    #[error("Sandbox API returned {status}: {code}: {message}")]
    Api {
        status: StatusCode,
        code: String,
        message: String,
    },
    #[error("Sandbox API returned {0} with an unreadable error body")]
    Unexpected(StatusCode),
}

#[derive(Clone)]
pub struct SandboxClient {
    base_url: Url,
    token: Option<SecretString>,
    http: reqwest::Client,
}

impl SandboxClient {
    pub fn new(mut base_url: Url, token: Option<SecretString>) -> Result<Self, ClientError> {
        if !base_url.path().ends_with('/') {
            let path = format!("{}/", base_url.path());
            base_url.set_path(&path);
        }
        let http = reqwest::Client::builder()
            .user_agent(concat!("sandbox-client/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            base_url,
            token,
            http,
        })
    }

    pub async fn health(&self) -> Result<HealthResponse, ClientError> {
        self.request::<(), _>(Method::GET, "healthz", None).await
    }

    pub async fn create_sandbox(
        &self,
        spec: SandboxSpec,
    ) -> Result<CreateSandboxResponse, ClientError> {
        self.request(
            Method::POST,
            "v1/sandboxes",
            Some(&CreateSandboxRequest { spec }),
        )
        .await
    }

    pub async fn list_sandboxes(&self, tenant: Option<&str>) -> Result<Vec<Sandbox>, ClientError> {
        let path = tenant.map_or_else(
            || "v1/sandboxes".into(),
            |tenant| {
                format!(
                    "v1/sandboxes?tenant={}",
                    url::form_urlencoded::byte_serialize(tenant.as_bytes()).collect::<String>()
                )
            },
        );
        let response: ListSandboxesResponse =
            self.request::<(), _>(Method::GET, &path, None).await?;
        Ok(response.sandboxes)
    }

    pub async fn get_sandbox(&self, id: SandboxId) -> Result<Sandbox, ClientError> {
        self.request::<(), _>(Method::GET, &format!("v1/sandboxes/{id}"), None)
            .await
    }

    pub async fn exec(
        &self,
        id: SandboxId,
        request: ExecSandboxRequest,
    ) -> Result<Operation, ClientError> {
        let response: OperationResponse = self
            .request(
                Method::POST,
                &format!("v1/sandboxes/{id}/exec"),
                Some(&request),
            )
            .await?;
        Ok(response.operation)
    }

    pub async fn delete(&self, id: SandboxId) -> Result<Operation, ClientError> {
        let response: OperationResponse = self
            .request::<(), _>(Method::DELETE, &format!("v1/sandboxes/{id}"), None)
            .await?;
        Ok(response.operation)
    }

    pub async fn operation(&self, id: OperationId) -> Result<Operation, ClientError> {
        let response: OperationResponse = self
            .request::<(), _>(Method::GET, &format!("v1/operations/{id}"), None)
            .await?;
        Ok(response.operation)
    }

    pub async fn register_node(
        &self,
        node: NodeRecord,
    ) -> Result<RegisterNodeResponse, ClientError> {
        self.request(
            Method::POST,
            "v1/nodes/register",
            Some(&RegisterNodeRequest { node }),
        )
        .await
    }

    pub async fn heartbeat(
        &self,
        id: NodeId,
        heartbeat: &HeartbeatRequest,
    ) -> Result<(), ClientError> {
        self.request::<_, serde_json::Value>(
            Method::POST,
            &format!("v1/nodes/{id}/heartbeat"),
            Some(heartbeat),
        )
        .await?;
        Ok(())
    }

    pub async fn lease_assignments(
        &self,
        id: NodeId,
        limit: usize,
    ) -> Result<LeaseAssignmentsResponse, ClientError> {
        self.request::<(), _>(
            Method::GET,
            &format!("v1/nodes/{id}/assignments?limit={limit}"),
            None,
        )
        .await
    }

    pub async fn complete_assignment(
        &self,
        request: &CompleteAssignmentRequest,
    ) -> Result<(), ClientError> {
        self.request::<_, serde_json::Value>(
            Method::POST,
            "v1/assignments/complete",
            Some(request),
        )
        .await?;
        Ok(())
    }

    async fn request<B, R>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<R, ClientError>
    where
        B: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let url = self.base_url.join(path)?;
        let mut request = self.http.request(method, url);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token.expose_secret());
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await?;
        let status = response.status();
        if status.is_success() {
            return response.json().await.map_err(Into::into);
        }
        match response.json::<ApiErrorBody>().await {
            Ok(error) => Err(ClientError::Api {
                status,
                code: error.code,
                message: error.message,
            }),
            Err(_) => Err(ClientError::Unexpected(status)),
        }
    }
}
