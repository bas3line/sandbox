mod local_http;

use std::{
    collections::BTreeMap,
    io::Write as _,
    path::{Path, PathBuf},
    process::ExitCode,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use sandbox_client::{ClientError, SandboxClient};
use sandbox_core::{
    OperationId, SandboxId, TunnelId,
    agent::{built_in_agent_profiles, find_agent_profile},
    api::{CreateTunnelRequest, ExecSandboxRequest},
    model::{
        CommandSpec, DataSensitivity, ExposureProtocol, IsolationPreference, NetworkMode,
        Operation, OperationState, PortExposure, ResourceSpec, Sandbox, SandboxSpec, SandboxState,
        WorkloadSignals,
    },
};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use url::Url;

const DEFAULT_SERVER: &str = "http://127.0.0.1:8080";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct ClientConfig {
    server: Option<Url>,
}

#[derive(Debug, Parser)]
#[command(name = "sandbox", version, about = "Create and control remote coding sandboxes", long_about = None)]
struct Cli {
    #[arg(long, env = "SANDBOX_URL", global = true)]
    server: Option<Url>,
    #[arg(long, env = "SANDBOX_TOKEN", hide_env_values = true, global = true)]
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
    List(ListArgs),
    /// Show one sandbox.
    Inspect { id: SandboxId },
    /// Execute an argv-safe command in a running sandbox.
    Exec(ExecArgs),
    /// Destroy a sandbox and its ephemeral storage.
    Delete {
        id: SandboxId,
        /// Return immediately with the delete operation ID.
        #[arg(long)]
        no_wait: bool,
        /// Deprecated compatibility flag; deletion now waits by default.
        #[arg(long, hide = true, conflicts_with = "no_wait")]
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
    /// Create, list, and remove public HTTP tunnels.
    Tunnel {
        #[command(subcommand)]
        command: TunnelCommands,
    },
    /// Share a local HTTP service at a temporary public URL.
    Http(HttpArgs),
    /// Print MCP client configuration for sandbox-mcp.
    McpConfig,
    /// Store or inspect non-secret CLI connection settings.
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
}

#[derive(Debug, Subcommand)]
enum ConfigCommands {
    /// Save the controller URL for future commands.
    SetServer { url: Url },
    /// Show the effective controller and configuration path.
    Show,
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
    /// Publish PORT, optionally with a custom PORT=SUBDOMAIN mapping.
    #[arg(long = "expose", value_parser = parse_exposure)]
    exposures: Vec<PortExposure>,
    /// Return immediately with the create operation ID.
    #[arg(long)]
    no_wait: bool,
    /// Maximum time to wait for creation.
    #[arg(long, default_value_t = 300, value_name = "SECONDS")]
    wait_timeout: u64,
    #[arg(last = true)]
    command: Vec<String>,
}

#[derive(Debug, Args)]
struct ListArgs {
    #[arg(long, env = "SANDBOX_TENANT")]
    tenant: Option<String>,
    /// Include stopped and failed audit records.
    #[arg(long)]
    all: bool,
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

#[derive(Debug, Args)]
struct HttpArgs {
    /// Local HTTP service port to share.
    #[arg(value_parser = parse_port)]
    port: u16,
    /// Hosted Sandbox relay. Override this for a self-hosted deployment.
    #[arg(
        long,
        env = "SANDBOX_HTTP_RELAY",
        default_value = "https://relay.tunnel.yshubham.com"
    )]
    relay: Url,
    /// Optional temporary hostname label below the relay's wildcard domain.
    #[arg(long)]
    subdomain: Option<String>,
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
        /// Agent network mode. Use open only when the agent must reach an external model API.
        #[arg(long, value_enum, default_value = "restricted")]
        network: NetworkArg,
        /// Return immediately after scheduling the agent sandbox.
        #[arg(long)]
        no_wait: bool,
        /// Maximum time to wait for sandbox creation.
        #[arg(long, default_value_t = 300, value_name = "SECONDS")]
        wait_timeout: u64,
        /// Maximum time for the optional agent command.
        #[arg(long, default_value_t = 900, value_name = "SECONDS")]
        command_timeout: u64,
        #[arg(last = true)]
        args: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
enum TunnelCommands {
    /// Publish one HTTP/WebSocket port from a running sandbox.
    Create {
        id: SandboxId,
        #[arg(long)]
        port: u16,
        #[arg(long)]
        subdomain: Option<String>,
        #[arg(long)]
        no_wait: bool,
    },
    /// List the public URLs allocated to a sandbox.
    List { id: SandboxId },
    /// Remove a public tunnel and its isolated edge network when no routes remain.
    Delete {
        id: SandboxId,
        tunnel_id: TunnelId,
        #[arg(long)]
        no_wait: bool,
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
async fn main() -> ExitCode {
    // Reqwest and the WebSocket relay share rustls. Select one process-wide
    // provider explicitly so release builds never depend on feature inference.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            if let Some(client_error) = error.downcast_ref::<ClientError>() {
                if client_error.is_connect() {
                    eprintln!(
                        "hint: run `sandbox config set-server <URL>` (or set SANDBOX_URL), then run `sandbox doctor`"
                    );
                } else if matches!(
                    client_error,
                    ClientError::Api { status, .. }
                        if *status == reqwest::StatusCode::UNAUTHORIZED
                            || *status == reqwest::StatusCode::FORBIDDEN
                ) {
                    eprintln!("hint: provide SANDBOX_TOKEN from your deployment's secret store");
                }
            }
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    // Local sharing and the static agent catalog must remain usable even when
    // no controller URL is configured or the saved controller config is bad.
    if let Commands::Http(args) = &cli.command {
        local_http::run(
            args.port,
            args.relay.clone(),
            args.subdomain.clone(),
            cli.token.as_deref(),
            cli.json,
        )
        .await?;
        return Ok(());
    }
    if matches!(
        &cli.command,
        Commands::Agent {
            command: AgentCommands::List
        }
    ) {
        render_agent_profiles(cli.json)?;
        return Ok(());
    }

    if let Commands::Config { command } = &cli.command {
        let config_path = client_config_path()?;
        match command {
            ConfigCommands::SetServer { url } => {
                validate_server_url(url)?;
                save_client_config(
                    &config_path,
                    &ClientConfig {
                        server: Some(url.clone()),
                    },
                )?;
                if cli.json {
                    print_json(&serde_json::json!({
                        "config_path": config_path,
                        "server": url,
                    }))?;
                } else {
                    println!("server  {url}");
                    println!("saved   {}", config_path.display());
                    println!("next    sandbox doctor");
                }
            }
            ConfigCommands::Show => {
                let server = resolve_server(cli.server.clone(), &config_path)?;
                if cli.json {
                    print_json(&serde_json::json!({
                        "config_path": config_path,
                        "server": server,
                        "token_configured": cli.token.is_some(),
                    }))?;
                } else {
                    println!("server  {server}");
                    println!("config  {}", config_path.display());
                    println!(
                        "token   {}",
                        if cli.token.is_some() {
                            "configured"
                        } else {
                            "not configured"
                        }
                    );
                }
            }
        }
        return Ok(());
    }

    let server = match cli.server.clone() {
        Some(server) => {
            validate_server_url(&server)?;
            server
        }
        None => resolve_server(None, &client_config_path()?)?,
    };
    let client = SandboxClient::new(server.clone(), cli.token.clone().map(SecretString::from))?;
    match cli.command {
        Commands::Doctor => {
            let health = client.health().await?;
            if cli.json {
                print_json(&health)?;
            } else {
                println!(
                    "ok  sandboxd {}  store={}  {}",
                    health.version, health.store, server
                );
                if health.tunnels.enabled {
                    println!(
                        "tunnels  {}://*.{}",
                        health.tunnels.public_scheme.as_deref().unwrap_or("https"),
                        health
                            .tunnels
                            .base_domain
                            .as_deref()
                            .unwrap_or("unconfigured")
                    );
                } else {
                    println!("tunnels  disabled");
                }
            }
        }
        Commands::Create(args) => {
            let no_wait = args.no_wait;
            let wait_timeout = args.wait_timeout;
            let spec = create_spec(args);
            let response = client.create_sandbox(spec).await?;
            if no_wait {
                if cli.json {
                    print_json(&response)?;
                } else {
                    println!(
                        "{}  scheduled  operation={}",
                        response.sandbox.id, response.operation.id
                    );
                }
            } else {
                let operation = wait_for(&client, response.operation.id, wait_timeout).await?;
                if operation.state == OperationState::Failed {
                    render_operation(&operation, cli.json)?;
                }
                let sandbox = client.get_sandbox(response.sandbox.id).await?;
                if cli.json {
                    print_json(&serde_json::json!({
                        "sandbox": sandbox,
                        "operation": operation,
                    }))?;
                } else {
                    render_sandbox(&sandbox);
                    println!("operation  {}  succeeded", operation.id);
                }
            }
        }
        Commands::List(args) => {
            let mut sandboxes = client.list_sandboxes(args.tenant.as_deref()).await?;
            if !args.all {
                sandboxes.retain(|sandbox| {
                    !matches!(sandbox.state, SandboxState::Stopped | SandboxState::Failed)
                });
            }
            if cli.json {
                print_json(&sandboxes)?;
            } else if sandboxes.is_empty() {
                if args.all {
                    println!("No sandboxes.");
                } else {
                    println!("No active sandboxes. Use `sandbox list --all` for audit records.");
                }
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
            if cli.json {
                print_json(&sandbox)?;
            } else {
                render_sandbox(&sandbox);
            }
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
        Commands::Delete {
            id,
            no_wait,
            wait: _,
        } => {
            let operation = client.delete(id).await?;
            if no_wait {
                if cli.json {
                    print_json(&operation)?;
                } else {
                    println!("{}", operation.id);
                }
            } else {
                let completed = wait_for(&client, operation.id, 120).await?;
                render_operation(&completed, cli.json)?;
            }
        }
        Commands::Wait { id, timeout } => {
            let operation = wait_for(&client, id, timeout).await?;
            render_operation(&operation, cli.json)?;
            if let Some(output) = operation.output
                && output.exit_code != 0
            {
                std::process::exit(output.exit_code.clamp(1, 255));
            }
        }
        Commands::Agent { command } => run_agent(command, &client, cli.json).await?,
        Commands::Tunnel { command } => run_tunnel(command, &client, cli.json).await?,
        Commands::Http(_) => unreachable!("HTTP commands return before client creation"),
        Commands::McpConfig => {
            print_json(
                &serde_json::json!({"mcpServers":{"sandbox":{"command":"sandbox-mcp","env":{"SANDBOX_URL":server,"SANDBOX_TOKEN":"use-your-client-secret-store"}}}}),
            )?;
        }
        Commands::Config { .. } => unreachable!("config commands return before client creation"),
    }
    Ok(())
}

fn client_config_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("SANDBOX_CONFIG").filter(|value| !value.is_empty()) {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            bail!("SANDBOX_CONFIG must be an absolute path");
        }
        return Ok(path);
    }
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path).join("sandbox/config.json"));
    }
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("HOME is unset; set SANDBOX_CONFIG to an absolute path"))?;
    Ok(PathBuf::from(home).join(".config/sandbox/config.json"))
}

fn load_client_config(path: &Path) -> Result<ClientConfig> {
    if !path.exists() {
        return Ok(ClientConfig::default());
    }
    let bytes = std::fs::read(path)
        .with_context(|| format!("read Sandbox config at {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("parse Sandbox config at {}", path.display()))
}

fn resolve_server(explicit: Option<Url>, config_path: &Path) -> Result<Url> {
    let server = match explicit {
        Some(server) => server,
        None => load_client_config(config_path)?
            .server
            .unwrap_or(Url::parse(DEFAULT_SERVER)?),
    };
    validate_server_url(&server)?;
    Ok(server)
}

fn validate_server_url(server: &Url) -> Result<()> {
    if !matches!(server.scheme(), "http" | "https") {
        bail!("Sandbox server URL must use HTTP or HTTPS");
    }
    if server.host_str().is_none() {
        bail!("Sandbox server URL must include a host");
    }
    if !server.username().is_empty() || server.password().is_some() {
        bail!("Sandbox server URL must not contain credentials");
    }
    if server.query().is_some() || server.fragment().is_some() {
        bail!("Sandbox server URL must not contain a query or fragment");
    }
    Ok(())
}

fn save_client_config(path: &Path, config: &ClientConfig) -> Result<()> {
    if let Some(server) = &config.server {
        validate_server_url(server)?;
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Sandbox config path has no parent directory"))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create Sandbox config directory at {}", parent.display()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("create temporary config in {}", parent.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    let mut bytes = serde_json::to_vec_pretty(config)?;
    bytes.push(b'\n');
    temporary.write_all(&bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("save Sandbox config at {}", path.display()))?;
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
        network: network_mode(args.network),
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
        exposures: args.exposures,
        agent: None,
    }
}

const fn network_mode(network: NetworkArg) -> NetworkMode {
    match network {
        NetworkArg::Deny => NetworkMode::Deny,
        NetworkArg::Restricted => NetworkMode::RestrictedEgress,
        NetworkArg::Open => NetworkMode::OpenEgress,
    }
}

async fn run_tunnel(command: TunnelCommands, client: &SandboxClient, json: bool) -> Result<()> {
    match command {
        TunnelCommands::Create {
            id,
            port,
            subdomain,
            no_wait,
        } => {
            let response = client
                .create_tunnel(
                    id,
                    CreateTunnelRequest {
                        container_port: port,
                        protocol: ExposureProtocol::Http,
                        subdomain,
                        authenticated: false,
                    },
                )
                .await?;
            if no_wait {
                if json {
                    print_json(&response)?;
                } else {
                    println!(
                        "{}  operation={}",
                        response.tunnel.public_url, response.operation.id
                    );
                }
                return Ok(());
            }
            let operation = wait_for(client, response.operation.id, 120).await?;
            if operation.state == OperationState::Failed {
                render_operation(&operation, json)?;
            }
            let sandbox = client.get_sandbox(id).await?;
            let tunnel = sandbox
                .tunnels
                .into_iter()
                .find(|tunnel| tunnel.id == response.tunnel.id)
                .context("activated tunnel was absent from sandbox state")?;
            if json {
                print_json(&serde_json::json!({"tunnel": tunnel, "operation": operation}))?;
            } else {
                println!("{}", tunnel.public_url);
            }
        }
        TunnelCommands::List { id } => {
            let tunnels = client.get_sandbox(id).await?.tunnels;
            if json {
                print_json(&tunnels)?;
            } else if tunnels.is_empty() {
                println!("No tunnels.");
            } else {
                println!("ID                                    STATE       PORT   URL");
                for tunnel in tunnels {
                    println!(
                        "{:<36}  {:<10}  {:<5}  {}",
                        tunnel.id,
                        format!("{:?}", tunnel.state).to_lowercase(),
                        tunnel.container_port,
                        tunnel.public_url
                    );
                }
            }
        }
        TunnelCommands::Delete {
            id,
            tunnel_id,
            no_wait,
        } => {
            let response = client.delete_tunnel(id, tunnel_id).await?;
            if no_wait {
                if json {
                    print_json(&response)?;
                } else {
                    println!("{}", response.operation.id);
                }
            } else {
                let operation = wait_for(client, response.operation.id, 120).await?;
                render_operation(&operation, json)?;
            }
        }
    }
    Ok(())
}

async fn run_agent(command: AgentCommands, client: &SandboxClient, json: bool) -> Result<()> {
    match command {
        AgentCommands::List => unreachable!("agent list returns before client creation"),
        AgentCommands::Run {
            name,
            tenant,
            image,
            ttl,
            network,
            no_wait,
            wait_timeout,
            command_timeout,
            args,
        } => {
            let profile = find_agent_profile(&name).ok_or_else(|| {
                anyhow::anyhow!("unknown agent profile {name}; use `sandbox agent list`")
            })?;
            let execute_agent = !args.is_empty();
            if no_wait && execute_agent {
                bail!(
                    "--no-wait cannot be combined with agent arguments; provision first, then use `sandbox exec`"
                );
            }
            let image = image.or_else(|| profile.default_image.clone()).ok_or_else(|| {
                anyhow::anyhow!(
                    "agent {name} has no bundled image; pass an approved immutable image with `--image`"
                )
            })?;
            let agent_argv = profile.command_argv(args);
            let mut spec = profile.sandbox_spec(tenant, image, ttl);
            spec.network = network_mode(network);
            let response = client.create_sandbox(spec).await?;
            if no_wait {
                if json {
                    print_json(&response)?;
                } else {
                    println!(
                        "{}  agent={}  scheduled  operation={}",
                        response.sandbox.id, name, response.operation.id
                    );
                }
                return Ok(());
            }

            let create_operation = wait_for(client, response.operation.id, wait_timeout).await?;
            if create_operation.state == OperationState::Failed {
                render_operation(&create_operation, json)?;
            }
            let sandbox = client.get_sandbox(response.sandbox.id).await?;

            // No extra arguments means "provision an agent-ready sandbox". An
            // interactive process would require PTY streaming, so do not start
            // one detached with its output discarded.
            if !execute_agent {
                if json {
                    print_json(&serde_json::json!({
                        "sandbox": sandbox,
                        "operation": create_operation,
                        "agent_command": null,
                    }))?;
                } else {
                    render_sandbox(&sandbox);
                    println!("agent      {}  provisioned", name);
                    println!(
                        "next       sandbox exec {} -- {} <non-interactive args>",
                        sandbox.id, profile.executable
                    );
                    println!("note       interactive PTY attachment is not available yet");
                }
                return Ok(());
            }

            let operation = client
                .exec(
                    sandbox.id,
                    ExecSandboxRequest {
                        command: CommandSpec {
                            argv: agent_argv,
                            cwd: Some("/workspace".into()),
                            env: BTreeMap::new(),
                            timeout_seconds: command_timeout,
                        },
                    },
                )
                .await?;
            let agent_operation =
                wait_for(client, operation.id, command_timeout.saturating_add(30)).await?;
            let exit_code = agent_operation
                .output
                .as_ref()
                .map_or(0, |output| output.exit_code);
            if json {
                print_json(&serde_json::json!({
                    "sandbox": sandbox,
                    "create_operation": create_operation,
                    "agent_operation": agent_operation,
                }))?;
            } else {
                render_operation(&agent_operation, false)?;
                println!("sandbox    {}  retained until deletion or TTL", sandbox.id);
            }
            if exit_code != 0 {
                std::process::exit(exit_code.clamp(1, 255));
            }
        }
    }
    Ok(())
}

fn render_agent_profiles(json: bool) -> Result<()> {
    let profiles = built_in_agent_profiles();
    if json {
        print_json(&profiles)?;
    } else {
        println!("NAME           DISPLAY NAME         DEFAULT IMAGE");
        for profile in profiles {
            println!(
                "{:<14} {:<20} {}",
                profile.name,
                profile.display_name,
                profile
                    .default_image
                    .as_deref()
                    .unwrap_or("<required: pass --image>")
            );
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
        let exit = operation
            .output
            .as_ref()
            .map(|output| format!("  exit={}", output.exit_code))
            .unwrap_or_default();
        eprintln!(
            "operation  {}  {}{}",
            operation.id,
            format!("{:?}", operation.state).to_lowercase(),
            exit
        );
    }
    // Command operations use Failed for a non-zero process status but still
    // include structured output. Let exec/wait propagate that exact exit code;
    // lifecycle failures have no command output and remain regular CLI errors.
    if operation.state == OperationState::Failed && operation.output.is_none() {
        bail!("operation {} failed", operation.id);
    }
    Ok(())
}

fn render_sandbox(sandbox: &Sandbox) {
    println!("sandbox    {}", sandbox.id);
    println!(
        "state      {}",
        format!("{:?}", sandbox.state).to_lowercase()
    );
    println!("image      {}", sandbox.spec.image);
    println!("node       {}", sandbox.node_id);
    println!(
        "isolation  {}  risk={}",
        format!("{:?}", sandbox.isolation).to_lowercase(),
        sandbox.risk_score
    );
    println!("expires    {}", sandbox.expires_at);
    if let Some(agent) = &sandbox.spec.agent {
        println!("agent      {agent}");
    }
    if let Some(failure) = &sandbox.failure {
        println!("failure    {failure}");
    }
    for tunnel in &sandbox.tunnels {
        println!(
            "tunnel     {}  {}",
            format!("{:?}", tunnel.state).to_lowercase(),
            tunnel.public_url
        );
    }
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

fn parse_exposure(value: &str) -> Result<PortExposure, String> {
    let (port, subdomain) = value
        .split_once('=')
        .map_or((value, None), |(port, subdomain)| (port, Some(subdomain)));
    let container_port = port
        .parse::<u16>()
        .map_err(|_| "exposure must be PORT or PORT=SUBDOMAIN".to_owned())?;
    if container_port == 0 || subdomain.is_some_and(str::is_empty) {
        return Err("exposure must be PORT or PORT=SUBDOMAIN".into());
    }
    let exposure = PortExposure {
        container_port,
        protocol: ExposureProtocol::Http,
        subdomain: subdomain.map(str::to_owned),
        authenticated: false,
    };
    exposure.validate().map_err(|error| error.to_string())?;
    Ok(exposure)
}

fn parse_port(value: &str) -> Result<u16, String> {
    let port = value
        .parse::<u16>()
        .map_err(|_| "port must be between 1 and 65535".to_owned())?;
    if port == 0 {
        return Err("port must be between 1 and 65535".into());
    }
    Ok(port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_connection_flags_work_before_or_after_subcommands() {
        for args in [
            [
                "sandbox",
                "--server",
                "https://sandbox.example.com",
                "doctor",
            ],
            [
                "sandbox",
                "doctor",
                "--server",
                "https://sandbox.example.com",
            ],
        ] {
            let cli = Cli::try_parse_from(args).expect("parse global server flag");
            assert_eq!(
                cli.server.as_ref().and_then(Url::host_str),
                Some("sandbox.example.com")
            );
        }
    }

    #[test]
    fn config_round_trip_keeps_only_the_server() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("nested/config.json");
        let expected = Url::parse("https://sandbox.example.com/control/").expect("valid URL");
        save_client_config(
            &path,
            &ClientConfig {
                server: Some(expected.clone()),
            },
        )
        .expect("save config");

        let loaded = load_client_config(&path).expect("load config");
        assert_eq!(loaded.server, Some(expected));
        let raw = std::fs::read_to_string(path).expect("read config");
        assert!(!raw.to_ascii_lowercase().contains("token"));
    }

    #[test]
    fn explicit_server_ignores_a_broken_saved_config() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("config.json");
        std::fs::write(&path, b"not json").expect("write broken config");
        let explicit = Url::parse("https://sandbox.example.com").expect("valid URL");

        assert_eq!(
            resolve_server(Some(explicit.clone()), &path).expect("resolve explicit server"),
            explicit
        );
    }

    #[test]
    fn server_urls_reject_credentials_and_non_http_schemes() {
        for value in [
            "file:///tmp/controller.sock",
            "https://user:password@sandbox.example.com",
            "https://sandbox.example.com?token=secret",
        ] {
            let url = Url::parse(value).expect("syntactically valid URL");
            assert!(validate_server_url(&url).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn create_and_delete_wait_by_default() {
        let cli = Cli::try_parse_from(["sandbox", "create", "--image", "ubuntu:24.04"])
            .expect("parse create");
        let Commands::Create(create) = cli.command else {
            panic!("expected create command");
        };
        assert!(!create.no_wait);

        let id = SandboxId::new();
        let cli =
            Cli::try_parse_from(["sandbox", "delete", &id.to_string()]).expect("parse delete");
        let Commands::Delete { no_wait, .. } = cli.command else {
            panic!("expected delete command");
        };
        assert!(!no_wait);
    }

    #[test]
    fn agent_arguments_use_the_explicit_delimiter() {
        let cli = Cli::try_parse_from([
            "sandbox",
            "agent",
            "run",
            "opencode",
            "--tenant",
            "test",
            "--",
            "--version",
        ])
        .expect("parse agent command");
        let Commands::Agent {
            command: AgentCommands::Run { args, .. },
        } = cli.command
        else {
            panic!("expected agent run command");
        };
        assert_eq!(args, ["--version"]);
    }

    #[test]
    fn parses_http_shortcut() {
        let cli =
            Cli::try_parse_from(["sandbox", "http", "3000"]).expect("parse sandbox http shortcut");
        let Commands::Http(args) = cli.command else {
            panic!("expected HTTP command");
        };
        assert_eq!(args.port, 3000);
        assert_eq!(args.relay.as_str(), "https://relay.tunnel.yshubham.com/");
        assert!(args.subdomain.is_none());
    }

    #[test]
    fn http_shortcut_rejects_port_zero() {
        assert!(Cli::try_parse_from(["sandbox", "http", "0"]).is_err());
    }

    #[test]
    fn parses_custom_http_relay_and_subdomain() {
        let cli = Cli::try_parse_from([
            "sandbox",
            "http",
            "4321",
            "--relay",
            "https://relay.sandbox.example",
            "--subdomain",
            "demo",
        ])
        .expect("parse custom relay");
        let Commands::Http(args) = cli.command else {
            panic!("expected HTTP command");
        };
        assert_eq!(args.relay.as_str(), "https://relay.sandbox.example/");
        assert_eq!(args.subdomain.as_deref(), Some("demo"));
    }
}
