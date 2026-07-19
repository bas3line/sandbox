use std::{
    collections::BTreeMap,
    env,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use sandbox_client::SandboxClient;
use sandbox_core::{
    OperationId, SandboxId, TunnelId,
    agent::{built_in_agent_profiles, find_agent_profile},
    api::{CreateTunnelRequest, ExecSandboxRequest},
    model::{
        CommandSpec, DataSensitivity, ExposureProtocol, IsolationPreference, NetworkMode,
        Operation, OperationState, PortExposure, ResourceSpec, SandboxSpec, WorkloadSignals,
    },
};
use secrecy::SecretString;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    net::TcpStream,
    process::Command,
    sync::mpsc,
};
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
    /// Create, list, and remove public HTTP tunnels.
    Tunnel {
        #[command(subcommand)]
        command: TunnelCommands,
    },
    /// Share a local HTTP service at a temporary public URL.
    Http(HttpArgs),
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
    /// Publish PORT, optionally with a custom PORT=SUBDOMAIN mapping.
    #[arg(long = "expose", value_parser = parse_exposure)]
    exposures: Vec<PortExposure>,
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

#[derive(Debug, Args)]
struct HttpArgs {
    /// Local HTTP service port to share.
    #[arg(value_parser = parse_port)]
    port: u16,
    /// Tunnel provider. Auto prefers cloudflared and falls back to SSH via localhost.run.
    #[arg(
        long,
        env = "SANDBOX_HTTP_PROVIDER",
        value_enum,
        default_value = "auto"
    )]
    provider: HttpProvider,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum HttpProvider {
    Auto,
    Cloudflare,
    LocalhostRun,
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
                for tunnel in &response.sandbox.tunnels {
                    println!("tunnel  {}  {:?}", tunnel.public_url, tunnel.state);
                }
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
        Commands::Tunnel { command } => run_tunnel(command, &client, cli.json).await?,
        Commands::Http(args) => run_http(args, cli.json).await?,
        Commands::McpConfig => {
            print_json(
                &serde_json::json!({"mcpServers":{"sandbox":{"command":"sandbox-mcp","env":{"SANDBOX_URL":cli.server,"SANDBOX_TOKEN":"use-your-client-secret-store"}}}}),
            )?;
        }
    }
    Ok(())
}

async fn run_http(args: HttpArgs, json: bool) -> Result<()> {
    ensure_local_listener(args.port).await?;
    let (provider, executable) = resolve_http_provider(args.provider)?;
    let mut command = provider_command(provider, &executable, args.port);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = command.spawn().with_context(|| {
        format!(
            "start the {} public tunnel helper at {}",
            provider_name(provider),
            executable.display()
        )
    })?;
    let stdout = child
        .stdout
        .take()
        .context("capture tunnel helper stdout")?;
    let stderr = child
        .stderr
        .take()
        .context("capture tunnel helper stderr")?;
    let (url_tx, mut url_rx) = mpsc::unbounded_channel();
    let stdout_task = tokio::spawn(find_public_url(stdout, provider, url_tx.clone()));
    let stderr_task = tokio::spawn(find_public_url(stderr, provider, url_tx));

    let public_url = match tokio::time::timeout(Duration::from_secs(30), url_rx.recv()).await {
        Ok(Some(url)) => url,
        Ok(None) => {
            let status = child
                .wait()
                .await
                .context("wait for public tunnel helper")?;
            bail!(
                "{} exited before issuing a public URL ({status})",
                provider_name(provider)
            );
        }
        Err(_) => {
            let _ = child.kill().await;
            bail!(
                "{} did not issue a public URL within 30 seconds; check your internet connection or try --provider {}",
                provider_name(provider),
                alternative_provider(provider)
            );
        }
    };

    if json {
        print_json(&serde_json::json!({
            "local_url": format!("http://localhost:{}", args.port),
            "provider": provider_name(provider),
            "public_url": public_url,
        }))?;
    } else {
        println!("{public_url}");
        eprintln!("Public URL: anyone with this address can access your local service.");
        eprintln!("Press Ctrl-C to stop sharing.");
    }

    tokio::select! {
        status = child.wait() => {
            let status = status.context("wait for public tunnel helper")?;
            if !status.success() {
                bail!("{} tunnel exited unexpectedly ({status})", provider_name(provider));
            }
        }
        signal = tokio::signal::ctrl_c() => {
            signal.context("listen for Ctrl-C")?;
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }

    let _ = stdout_task.await;
    let _ = stderr_task.await;
    Ok(())
}

async fn ensure_local_listener(port: u16) -> Result<()> {
    let addresses: [std::net::SocketAddr; 2] = [
        ([127, 0, 0, 1], port).into(),
        ([0, 0, 0, 0, 0, 0, 0, 1], port).into(),
    ];
    for address in addresses {
        if matches!(
            tokio::time::timeout(Duration::from_millis(500), TcpStream::connect(address)).await,
            Ok(Ok(_))
        ) {
            return Ok(());
        }
    }
    bail!(
        "nothing is listening on localhost:{port}; start your app first, then run `sandbox http {port}`"
    )
}

fn resolve_http_provider(requested: HttpProvider) -> Result<(HttpProvider, PathBuf)> {
    match requested {
        HttpProvider::Auto => {
            if let Some(path) = find_executable("cloudflared") {
                return Ok((HttpProvider::Cloudflare, path));
            }
            find_ssh()
                .map(|path| (HttpProvider::LocalhostRun, path))
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "no public tunnel helper found; install cloudflared or OpenSSH, then retry"
                    )
                })
        }
        HttpProvider::Cloudflare => find_executable("cloudflared")
            .map(|path| (HttpProvider::Cloudflare, path))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "cloudflared is not installed; install it or use --provider localhost-run"
                )
            }),
        HttpProvider::LocalhostRun => find_ssh()
            .map(|path| (HttpProvider::LocalhostRun, path))
            .ok_or_else(|| {
                anyhow::anyhow!("OpenSSH is not installed; install it or use --provider cloudflare")
            }),
    }
}

fn find_ssh() -> Option<PathBuf> {
    find_executable("ssh").or_else(|| {
        [Path::new("/usr/bin/ssh"), Path::new("/bin/ssh")]
            .into_iter()
            .find(|path| path.is_file())
            .map(Path::to_path_buf)
    })
}

fn find_executable(name: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|path| {
        env::split_paths(&path)
            .map(|directory| directory.join(name))
            .find(|candidate| candidate.is_file())
    })
}

fn provider_command(provider: HttpProvider, executable: &Path, port: u16) -> Command {
    let mut command = Command::new(executable);
    match provider {
        HttpProvider::Cloudflare => {
            command.args(["tunnel", "--url", &format!("http://localhost:{port}")]);
        }
        HttpProvider::LocalhostRun => {
            command.args([
                "-T",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "StrictHostKeyChecking=accept-new",
                "-o",
                "ServerAliveInterval=30",
                "-o",
                "ServerAliveCountMax=3",
                "-o",
                "ExitOnForwardFailure=yes",
                "-R",
                &format!("80:localhost:{port}"),
                "nokey@localhost.run",
            ]);
        }
        HttpProvider::Auto => unreachable!("auto provider must be resolved before launch"),
    }
    command
}

async fn find_public_url<R>(reader: R, provider: HttpProvider, sender: mpsc::UnboundedSender<Url>)
where
    R: AsyncRead + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(url) = public_url_in_line(provider, &line) {
            let _ = sender.send(url);
        }
    }
}

fn public_url_in_line(provider: HttpProvider, line: &str) -> Option<Url> {
    line.split_whitespace().find_map(|token| {
        let token = &token[token.find("https://")?..];
        let token = token.trim_end_matches(|character: char| {
            !character.is_ascii_alphanumeric()
                && !matches!(
                    character,
                    ':' | '/' | '.' | '-' | '_' | '?' | '=' | '&' | '%'
                )
        });
        let url = Url::parse(token).ok()?;
        let host = url.host_str()?;
        let matches_provider = match provider {
            HttpProvider::Cloudflare => host.ends_with(".trycloudflare.com"),
            HttpProvider::LocalhostRun => host.ends_with(".lhr.life"),
            HttpProvider::Auto => false,
        };
        (url.scheme() == "https" && matches_provider).then_some(url)
    })
}

fn provider_name(provider: HttpProvider) -> &'static str {
    match provider {
        HttpProvider::Auto => "auto",
        HttpProvider::Cloudflare => "cloudflare",
        HttpProvider::LocalhostRun => "localhost.run",
    }
}

fn alternative_provider(provider: HttpProvider) -> &'static str {
    match provider {
        HttpProvider::Cloudflare => "localhost-run",
        HttpProvider::LocalhostRun | HttpProvider::Auto => "cloudflare",
    }
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
        exposures: args.exposures,
        agent: None,
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
    fn parses_http_shortcut() {
        let cli =
            Cli::try_parse_from(["sandbox", "http", "3000"]).expect("parse sandbox http shortcut");
        let Commands::Http(args) = cli.command else {
            panic!("expected HTTP command");
        };
        assert_eq!(args.port, 3000);
        assert_eq!(args.provider, HttpProvider::Auto);
    }

    #[test]
    fn http_shortcut_rejects_port_zero() {
        assert!(Cli::try_parse_from(["sandbox", "http", "0"]).is_err());
    }

    #[test]
    fn parses_explicit_http_provider() {
        let cli = Cli::try_parse_from(["sandbox", "http", "4321", "--provider", "localhost-run"])
            .expect("parse explicit provider");
        let Commands::Http(args) = cli.command else {
            panic!("expected HTTP command");
        };
        assert_eq!(args.provider, HttpProvider::LocalhostRun);
    }

    #[test]
    fn extracts_only_the_expected_provider_url() {
        assert_eq!(
            public_url_in_line(
                HttpProvider::LocalhostRun,
                "site tunneled with tls, https://abc123.lhr.life"
            )
            .expect("extract localhost.run URL")
            .as_str(),
            "https://abc123.lhr.life/"
        );
        assert!(
            public_url_in_line(
                HttpProvider::LocalhostRun,
                "documentation: https://localhost.run/docs/"
            )
            .is_none()
        );
        assert_eq!(
            public_url_in_line(
                HttpProvider::Cloudflare,
                "INF Your quick Tunnel has been created! url=https://demo.trycloudflare.com"
            )
            .expect("extract Cloudflare URL")
            .as_str(),
            "https://demo.trycloudflare.com/"
        );
    }

    #[tokio::test]
    async fn detects_a_local_listener() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind local listener");
        let port = listener.local_addr().expect("listener address").port();
        ensure_local_listener(port)
            .await
            .expect("detect local listener");
    }
}
