mod controller;
mod worker;

use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use sandbox_aegis::{AegisPolicy, AegisScheduler};
use sandbox_core::config::SandboxConfig;
use secrecy::ExposeSecret;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Role {
    Controller,
    Worker,
    All,
}

#[derive(Debug, Parser)]
#[command(
    name = "sandboxd",
    version,
    about = "Sandbox control-plane and worker daemon"
)]
struct Args {
    #[arg(long, env = "SANDBOX_CONFIG")]
    config: Option<PathBuf>,
    #[arg(long, env = "SANDBOX_ROLE", value_enum, default_value = "all")]
    role: Role,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let args = Args::parse();
    let config =
        Arc::new(SandboxConfig::load(args.config.as_deref()).context("load configuration")?);
    validate_auth(&config, args.role)?;
    let cancel = CancellationToken::new();
    let shutdown = cancel.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        shutdown.cancel();
    });

    match args.role {
        Role::Controller => run_controller(config, cancel).await,
        Role::Worker => worker::run(config, cancel).await,
        Role::All => run_all(config, cancel).await,
    }
}

async fn run_controller(config: Arc<SandboxConfig>, cancel: CancellationToken) -> Result<()> {
    let store = sandbox_storage::connect(&config.store)
        .await
        .context("connect state store")?;
    let bus = sandbox_events::connect(&config.bus)
        .await
        .context("connect event bus")?;
    let scheduler = AegisScheduler::new(AegisPolicy {
        microvm_risk_threshold: config.policy.microvm_risk_threshold,
        heartbeat_timeout_seconds: config.server.heartbeat_timeout_seconds,
        ..Default::default()
    });
    controller::serve(config, store, bus, scheduler, cancel).await
}

async fn run_all(config: Arc<SandboxConfig>, cancel: CancellationToken) -> Result<()> {
    let controller_config = config.clone();
    let controller_cancel = cancel.clone();
    let mut controller_task =
        tokio::spawn(async move { run_controller(controller_config, controller_cancel).await });
    let worker_config = config.clone();
    let worker_cancel = cancel.clone();
    let mut worker_task =
        tokio::spawn(async move { worker::run(worker_config, worker_cancel).await });

    tokio::select! {
        result = &mut controller_task => {
            cancel.cancel();
            worker_task.abort();
            result.context("controller task join")?
        }
        result = &mut worker_task => {
            cancel.cancel();
            controller_task.abort();
            result.context("worker task join")?
        }
        () = cancel.cancelled() => {
            controller_task.abort();
            worker_task.abort();
            info!("sandboxd shutdown complete");
            Ok(())
        }
    }
}

fn validate_auth(config: &SandboxConfig, role: Role) -> Result<()> {
    if config.server.allow_unauthenticated_dev {
        return Ok(());
    }
    if matches!(role, Role::Controller | Role::All) {
        let api_token = config
            .server
            .api_token
            .as_ref()
            .map(ExposeSecret::expose_secret);
        let node_token = config
            .server
            .node_token
            .as_ref()
            .map(ExposeSecret::expose_secret);
        if api_token.is_none_or(|token| token.len() < 32)
            || node_token.is_none_or(|token| token.len() < 32)
        {
            bail!(
                "controller requires distinct API and node tokens of at least 32 characters; development bypass must be explicit"
            )
        }
        if api_token == node_token {
            bail!("API and node tokens must be different");
        }
    }
    if matches!(role, Role::Worker | Role::All)
        && config
            .node
            .token
            .as_ref()
            .is_none_or(|token| token.expose_secret().len() < 32)
    {
        bail!("worker requires SANDBOX__NODE__TOKEN with at least 32 characters")
    }
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("sandboxd=info,tower_http=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .with_current_span(false)
        .init();
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
        match terminate {
            Ok(mut signal) => {
                tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = signal.recv() => {} }
            }
            Err(error) => {
                error!(%error, "failed to install SIGTERM handler");
                let _result = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _result = tokio::signal::ctrl_c().await;
    }
}
