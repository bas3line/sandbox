use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use sandbox_client::SandboxClient;
use sandbox_core::{
    OperationId, SandboxId,
    agent::{built_in_agent_profiles, find_agent_profile},
    api::ExecSandboxRequest,
    model::{
        CommandSpec, DataSensitivity, IsolationPreference, NetworkMode, Operation, OperationState,
        ResourceSpec, SandboxSpec, WorkloadSignals,
    },
};
use secrecy::SecretString;
use url::Url;

#[derive(Debug, Parser)]
#[command(name = "sandbox", version, about = "Create and control remote coding sandboxes", long_about = None)]
struct Cli {
    #[arg(long, env = "SANDBOX_URL", default_value = "http://127.0.0.1:8080")]
    server: Url,
    #[arg(long, env = "SANDBOX_TOKEN", hide_env_values = true)]
    token: Option<String>,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Check API reachability and version.
    Doctor,
    /// Create a sandbox.
    Create(CreateArgs),
    /// List sandboxes.
    List {
        #[arg(long, env = "SANDBOX_TENANT")]
        tenant: Option<String>,
    },
    /// Show one sandbox.
    Inspect { id: SandboxId },
    /// Execute an argv-safe command in a running sandbox.
    Exec(ExecArgs),
    /// Destroy a sandbox and its ephemeral storage.
    Delete {
        id: SandboxId,
        #[arg(long)]
        wait: bool,
    },
    /// Wait for an asynchronous operation.
    Wait {
        id: OperationId,
        #[arg(long, default_value_t = 300)]
        timeout: u64,
    },
    /// Launch and manage coding-agent profiles.
    Agent {
        #[command(subcommand)]
        command: AgentCommands,
    },
    /// Print MCP client configuration for sandbox-mcp.
    McpConfig,
}

#[derive(Debug, Args)]
struct CreateArgs {
    #[arg(long, env = "SANDBOX_TENANT", default_value = "default")]
    tenant: String,
    #[arg(long)]
    image: String,
    #[arg(long, default_value_t = 1_000)]
    cpu_millis: u32,
    #[arg(long, default_value_t = 1_024)]
    memory_mib: u32,
    #[arg(long, default_value_t = 10_240)]
    disk_mib: u32,
    #[arg(long, default_value_t = 256)]
    pids: u32,
    #[arg(long, default_value_t = 3_600)]
    ttl: u64,
    #[arg(long, value_enum, default_value = "deny")]
    network: NetworkArg,
    #[arg(long, value_enum, default_value = "auto")]
    isolation: IsolationArg,
    #[arg(long, value_enum, default_value = "internal")]
    sensitivity: SensitivityArg,
    #[arg(long)]
    untrusted_repo: bool,
    #[arg(long)]
    generated_code: bool,
    #[arg(long)]
    needs_secrets: bool,
    #[arg(long = "label", value_parser = parse_key_value)]
    labels: Vec<(String, String)>,
    #[arg(last = true)]
    command: Vec<String>,
}

#[derive(Debug, Args)]
struct ExecArgs {
    id: SandboxId,
    #[arg(long)]
    cwd: Option<String>,
    #[arg(long, default_value_t = 300)]
    timeout: u64,
    #[arg(long = "env", value_parser = parse_key_value)]
    env: Vec<(String, String)>,
    #[arg(long)]
    no_wait: bool,
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    argv: Vec<String>,
}

#[derive(Debug, Subcommand)]
enum AgentCommands {
    /// List built-in agent profiles.
    List,
    /// Create a hardened sandbox using an agent profile.
    Run {
        name: String,
        #[arg(long, env = "SANDBOX_TENANT", default_value = "default")]
        tenant: String,
        #[arg(long)]
        image: Option<String>,
        #[arg(long, default_value_t = 3_600)]
        ttl: u64,
        #[arg(last = true)]
        args: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum NetworkArg {
    Deny,
    Restricted,
    Open,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
enum IsolationArg {
    Auto,
    Container,
    Microvm,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
enum SensitivityArg {
    Public,
    Internal,
    Confidential,
    Restricted,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = SandboxClient::new(
        cli.server.clone(),
        cli.token.clone().map(SecretString::from),
    )?;
    match cli.command {
        Commands::Doctor => {
            let health = client.health().await.context("contact sandbox server")?;
            if cli.json {
                print_json(&health)?;
            } else {
                println!(
                    "ok  sandboxd {}  store={}  {}",
                    health.version, health.store, cli.server
                );
            }
        }
        Commands::Create(args) => {
            let spec = create_spec(args);
            let response = client.create_sandbox(spec).await?;
            if cli.json {
                print_json(&response)?;
            } else {
                println!(
                    "{}  {:?}  node={}  isolation={:?}  operation={}",
                    response.sandbox.id,
                    response.sandbox.state,
                    response.sandbox.node_id,
                    response.sandbox.isolation,
                    response.operation.id
                );
            }
        }
        Commands::List { tenant } => {
            let sandboxes = client.list_sandboxes(tenant.as_deref()).await?;
            if cli.json {
                print_json(&sandboxes)?;
            } else {
                println!("ID                                    STATE       ISOLATION  IMAGE");
                for sandbox in sandboxes {
                    println!(
                        "{:<36}  {:<10}  {:<9}  {}",
                        sandbox.id,
                        format!("{:?}", sandbox.state).to_lowercase(),
                        format!("{:?}", sandbox.isolation).to_lowercase(),
                        sandbox.spec.image
                    );
                }
            }
        }
        Commands::Inspect { id } => {
            let sandbox = client.get_sandbox(id).await?;
            print_json(&sandbox)?;
        }
        Commands::Exec(args) => {
            let operation = client
                .exec(
                    args.id,
                    ExecSandboxRequest {
                        command: CommandSpec {
                            argv: args.argv,
                            cwd: args.cwd,
                            env: args.env.into_iter().collect(),
                            timeout_seconds: args.timeout,
                        },
                    },
                )
                .await?;
            if args.no_wait {
                if cli.json {
                    print_json(&operation)?;
                } else {
                    println!("{}", operation.id);
                }
            } else {
                let completed =
                    wait_for(&client, operation.id, args.timeout.saturating_add(30)).await?;
                render_operation(&completed, cli.json)?;
                if let Some(output) = completed.output
                    && output.exit_code != 0
                {
                    std::process::exit(output.exit_code.clamp(1, 255));
                }
            }
        }
        Commands::Delete { id, wait } => {
            let operation = client.delete(id).await?;
            if wait {
                let completed = wait_for(&client, operation.id, 120).await?;
                render_operation(&completed, cli.json)?;
            } else if cli.json {
                print_json(&operation)?;
            } else {
                println!("{}", operation.id);
            }
        }
        Commands::Wait { id, timeout } => {
            let operation = wait_for(&client, id, timeout).await?;
            render_operation(&operation, cli.json)?;
        }
        Commands::Agent { command } => run_agent(command, &client, cli.json).await?,
        Commands::McpConfig => {
            print_json(
                &serde_json::json!({"mcpServers":{"sandbox":{"command":"sandbox-mcp","env":{"SANDBOX_URL":cli.server,"SANDBOX_TOKEN":"use-your-client-secret-store"}}}}),
            )?;
        }
    }
    Ok(())
}

fn create_spec(args: CreateArgs) -> SandboxSpec {
    SandboxSpec {
        tenant: args.tenant,
        image: args.image,
        command: args.command,
        env: BTreeMap::new(),
        resources: ResourceSpec {
            cpu_millis: args.cpu_millis,
            memory_mib: args.memory_mib,
            disk_mib: args.disk_mib,
            pids: args.pids,
        },
        network: match args.network {
            NetworkArg::Deny => NetworkMode::Deny,
            NetworkArg::Restricted => NetworkMode::RestrictedEgress,
            NetworkArg::Open => NetworkMode::OpenEgress,
        },
        isolation: match args.isolation {
            IsolationArg::Auto => IsolationPreference::Auto,
            IsolationArg::Container => IsolationPreference::Container,
            IsolationArg::Microvm => IsolationPreference::Microvm,
        },
        sensitivity: match args.sensitivity {
            SensitivityArg::Public => DataSensitivity::Public,
            SensitivityArg::Internal => DataSensitivity::Internal,
            SensitivityArg::Confidential => DataSensitivity::Confidential,
            SensitivityArg::Restricted => DataSensitivity::Restricted,
        },
        signals: WorkloadSignals {
            untrusted_repository: args.untrusted_repo,
            executes_generated_code: args.generated_code,
            needs_secrets: args.needs_secrets,
            host_mounts: false,
            privileged: false,
        },
        ttl_seconds: args.ttl,
        labels: args.labels.into_iter().collect(),
        placement: Default::default(),
        exposures: Vec::new(),
        agent: None,
    }
}

async fn run_agent(command: AgentCommands, client: &SandboxClient, json: bool) -> Result<()> {
    match command {
        AgentCommands::List => {
            let profiles = built_in_agent_profiles();
            if json {
                print_json(&profiles)?;
            } else {
                for profile in profiles {
                    println!("{:<14} {}", profile.name, profile.display_name);
                }
            }
        }
        AgentCommands::Run {
            name,
            tenant,
            image,
            ttl,
            args,
        } => {
            let profile = find_agent_profile(&name).ok_or_else(|| {
                anyhow::anyhow!("unknown agent profile {name}; use `sandbox agent list`")
            })?;
            let mut spec = profile.sandbox_spec(tenant, args, ttl);
            if let Some(image) = image {
                spec.image = image;
            }
            let response = client.create_sandbox(spec).await?;
            if json {
                print_json(&response)?;
            } else {
                println!(
                    "{}  agent={}  operation={}",
                    response.sandbox.id, name, response.operation.id
                );
            }
        }
    }
    Ok(())
}

async fn wait_for(
    client: &SandboxClient,
    id: OperationId,
    timeout_seconds: u64,
) -> Result<Operation> {
    let started = Instant::now();
    loop {
        let operation = client.operation(id).await?;
        if matches!(
            operation.state,
            OperationState::Succeeded | OperationState::Failed
        ) {
            return Ok(operation);
        }
        if started.elapsed() >= Duration::from_secs(timeout_seconds) {
            bail!("operation {id} did not complete within {timeout_seconds} seconds");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn render_operation(operation: &Operation, json: bool) -> Result<()> {
    if json {
        print_json(operation)?;
    } else {
        if let Some(output) = &operation.output {
            print!("{}", output.stdout);
            eprint!("{}", output.stderr);
        }
        if let Some(error) = &operation.error {
            eprintln!("sandbox: {error}");
        }
    }
    if operation.state == OperationState::Failed {
        bail!("operation {} failed", operation.id);
    }
    Ok(())
}

fn print_json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn parse_key_value(value: &str) -> Result<(String, String), String> {
    let (key, value) = value
        .split_once('=')
        .ok_or_else(|| "expected KEY=VALUE".to_owned())?;
    if key.is_empty() {
        return Err("key cannot be empty".into());
    }
    Ok((key.into(), value.into()))
}
