use std::{
    fs,
    io::{ErrorKind, Read, stderr},
    mem,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    time::Duration,
};

use anyhow::Context;
use clap::{ArgAction, CommandFactory, Parser, ValueEnum};
use ic_principal::Principal;
use notify::{Event, RecursiveMode, Watcher, recommended_watcher};
use pocket_ic::{
    PocketIcBuilder,
    common::rest::{AutoProgressConfig, IcpFeatures, IcpFeaturesConfig, InstanceHttpGatewayConfig},
};
use reqwest::Client;
use semver::{Version, VersionReq};
use serde::Serialize;
use sysinfo::{ProcessesToUpdate, Signal, System};
use tempfile::{NamedTempFile, TempDir};
use tokio::select;
use tokio::{process::Command, signal::unix::SignalKind};

/// CLI launcher for the pocket-ic server, primarily for use with icp-cli.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// The expected version of the CLI interface. Only used for automated setups.
    #[arg(long, env = "ICP_CLI_NETWORK_LAUNCHER_INTERFACE_VERSION")]
    interface_version: Option<Version>,
    /// Port for the HTTP gateway for the ICP API to listen on.
    #[arg(long)]
    gateway_port: Option<u16>,
    /// Port for the PocketIC admin interface to listen on.
    #[arg(long)]
    config_port: Option<u16>,
    /// Network interface to bind the PocketIC server on.
    #[arg(long)]
    bind: Option<IpAddr>,
    /// Directory to store the PocketIC state.
    #[arg(long)]
    state_dir: Option<PathBuf>,
    /// Artificial delay for execution, in milliseconds.
    #[arg(long)]
    artificial_delay_ms: Option<u64>,
    /// List of subnets to create. `--subnet=nns` is always implied. Defaults to `--subnet=application`.
    #[arg(long, value_enum, action = ArgAction::Append)]
    subnet: Vec<SubnetKind>,
    /// Addresses of bitcoind nodes to connect to. Implies `--subnet=bitcoin`.
    #[arg(long, action = ArgAction::Append)]
    bitcoind_addr: Vec<SocketAddr>,
    /// Addresses of dogecoind nodes to connect to. Implies `--subnet=bitcoin`.
    #[arg(long, action = ArgAction::Append)]
    dogecoind_addr: Vec<SocketAddr>,
    /// Installs the Internet Identity canister.
    #[arg(long)]
    ii: bool,
    /// Installs the NNS and SNS. Implies `--ii` and `--subnet=sns`.
    #[arg(long)]
    nns: bool,
    /// Path to the pocket-ic server binary. By default, looks for `pocket-ic` next to the launcher.
    /// The launcher is unlikely to be usable with a different version than it shipped with.
    #[arg(long)]
    pocketic_server_path: Option<PathBuf>,
    /// File to redirect pocket-ic stdout to.
    #[arg(long)]
    stdout_file: Option<PathBuf>,
    /// File to redirect pocket-ic stderr to.
    #[arg(long)]
    stderr_file: Option<PathBuf>,
    /// Directory to write status signal files to. Used by automated setups.
    #[arg(long)]
    status_dir: Option<PathBuf>,
    /// Enables verbose logging from pocket-ic. By default only errors are printed.
    #[arg(long)]
    verbose: bool,
    #[arg(trailing_var_arg = true, hide = true, allow_hyphen_values = true)]
    unknown_args: Vec<String>,
}

#[derive(ValueEnum, Clone)]
enum SubnetKind {
    Application,
    System,
    VerifiedApplication,
    Bitcoin,
    Fiduciary,
    Nns,
    Sns,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let Cli {
        gateway_port,
        config_port,
        bind,
        state_dir,
        artificial_delay_ms,
        subnet,
        bitcoind_addr,
        dogecoind_addr,
        ii,
        nns,
        pocketic_server_path,
        stdout_file,
        stderr_file,
        status_dir,
        verbose,
        interface_version: _,
        unknown_args: _,
    } = get_errorchecked_args();
    // pocket-ic is expected to be installed next to the launcher (see package.sh)
    let pocketic_server_path = if let Some(path) = pocketic_server_path {
        path
    } else {
        let assumed = std::env::current_exe()
            .context("Failed to get current exe path")?
            .parent()
            .expect("exe path should always have parent")
            .join("pocket-ic");
        if !assumed.exists() {
            eprintln!(
                "Error: --pocketic-server-path not provided and could not find pocket-ic next to the launcher"
            );
            std::process::exit(1);
        }
        assumed
    };

    // pocket-ic produces a lot of output so we're going to mute stderr for a moment
    let (pic, mut child, topology, config_port) = try_with_maybe_muted_stderr(verbose, async {
        // We learn the port by pocket-ic writing it to a file
        let tmpdir = TempDir::new().context("failed to create temporary directory")?;
        let port_file = tmpdir.path().join("pocketic.port");
        let (tx, mut rx) = tokio::sync::mpsc::channel(10);
        let mut watcher = recommended_watcher({
            let port_file = port_file.clone();
            move |event: Result<Event, notify::Error>| {
                if let Err(e) = event {
                    _ = tx.blocking_send(Err(e).context("failed to watch directory for port file"));
                    return;
                }
                match fs::read_to_string(&port_file) {
                    Ok(contents) => {
                        if contents.ends_with('\n') {
                            match contents.trim().parse::<u16>() {
                                Ok(port) => _ = tx.blocking_send(Ok(port)),
                                Err(e) => {
                                    _ = tx.blocking_send(
                                        Err(e).context("failed to parse port from port file"),
                                    )
                                }
                            }
                        }
                    }
                    Err(e) if e.kind() == ErrorKind::NotFound => {}
                    Err(e) => panic!("Failed to read port file: {}", e),
                };
            }
        })
        .context("failed to create file watcher")?;
        watcher
            .watch(tmpdir.path(), RecursiveMode::Recursive)
            .context("failed to watch temporary directory")?;
        // pocket-ic CLI setup begins here
        let mut cmd = Command::new(&pocketic_server_path);
        // the default TTL is 1m - increase to 30 days. We manually shut the network down instead of relying on idle timeout.
        cmd.args(["--ttl", "2592000"]);
        cmd.arg("--port-file").arg(&port_file);
        if let Some(config_port) = config_port {
            cmd.args(["--port", &config_port.to_string()]);
        }
        if let Some(bind) = bind {
            cmd.arg("--ip-addr").arg(bind.to_string());
        }
        if let Some(stdout_file) = stdout_file {
            let file =
                std::fs::File::create(stdout_file).context("failed to create stdout file")?;
            cmd.stdout(file);
        }
        if let Some(stderr_file) = stderr_file {
            let file =
                std::fs::File::create(stderr_file).context("failed to create stderr file")?;
            cmd.stderr(file);
        }
        if !verbose {
            cmd.args(["--log-levels", "error"]);
        }
        #[cfg(unix)]
        {
            cmd.process_group(0);
        }
        let child = cmd
            .spawn()
            .context("failed to spawn pocket-ic server process")?;
        let config_port = rx
            .recv()
            .await
            .expect("failed to receive port from watcher")?;
        drop(watcher);
        // pocket-ic CLI setup ends here
        // initial HTTP setup
        let mut pic = PocketIcBuilder::new()
            .with_server_url(
                format!("http://127.0.0.1:{config_port}/")
                    .parse()
                    .expect("valid url"),
            )
            .with_http_gateway(InstanceHttpGatewayConfig {
                ip_addr: bind.map(|ip| ip.to_string()),
                port: gateway_port,
                domains: Some(vec!["localhost".to_string()]),
                https_config: None,
            });
        if let Some(dir) = state_dir {
            pic = pic.with_state_dir(dir);
        }
        if subnet.is_empty() {
            pic = pic.with_application_subnet();
        } else {
            for subnet in subnet {
                match subnet {
                    SubnetKind::Application => pic = pic.with_application_subnet(),
                    SubnetKind::System => pic = pic.with_system_subnet(),
                    SubnetKind::VerifiedApplication => pic = pic.with_verified_application_subnet(),
                    SubnetKind::Bitcoin => pic = pic.with_bitcoin_subnet(),
                    SubnetKind::Fiduciary => pic = pic.with_fiduciary_subnet(),
                    SubnetKind::Nns => pic = pic.with_nns_subnet(),
                    SubnetKind::Sns => pic = pic.with_sns_subnet(),
                }
            }
        }
        pic = pic.with_nns_subnet();
        let mut features = IcpFeatures {
            cycles_minting: Some(IcpFeaturesConfig::DefaultConfig),
            icp_token: Some(IcpFeaturesConfig::DefaultConfig),
            cycles_token: Some(IcpFeaturesConfig::DefaultConfig),
            registry: Some(IcpFeaturesConfig::DefaultConfig),
            ..<_>::default()
        };
        if nns || ii {
            pic = pic.with_ii_subnet();
            features.ii = Some(IcpFeaturesConfig::DefaultConfig);
        }
        if nns {
            pic = pic.with_sns_subnet();
            features.nns_governance = Some(IcpFeaturesConfig::DefaultConfig);
            features.nns_ui = Some(IcpFeaturesConfig::DefaultConfig);
            features.sns = Some(IcpFeaturesConfig::DefaultConfig);
        }
        pic = pic.with_icp_features(features);
        if !bitcoind_addr.is_empty() {
            pic = pic.with_bitcoind_addrs(bitcoind_addr);
        }
        if !dogecoind_addr.is_empty() {
            pic = pic.with_dogecoind_addrs(dogecoind_addr);
        }
        let pic = pic.build_async().await;
        // pocket-ic crate doesn't currently support setting artificial delay via builder
        let client = Client::new();
        let progress_url = pic
            .get_server_url()
            .join(&format!("/instances/{}/auto_progress", pic.instance_id))
            .expect("valid url");
        client
            .post(progress_url)
            .json(&AutoProgressConfig {
                artificial_delay_ms,
            })
            .send()
            .await
            .context("failed to send auto progress config to pocket-ic")?
            .error_for_status()
            .context("failed to configure pocket-ic for auto-progress")?;
        let topology = pic.topology().await;
        Ok((pic, child, topology, config_port))
    })
    .await?;
    let default_ecid = Principal::from_slice(&topology.default_effective_canister_id.canister_id);
    let gateway_url = pic.url().expect("gateway url set in builder");
    let gateway_port = gateway_url
        .port_or_known_default()
        .expect("gateway urls should have a known port");
    // write everything to the status file
    if let Some(status_dir) = status_dir {
        fs::create_dir_all(&status_dir).context("failed to create status directory")?;
        let status_file = status_dir.join("status.json");
        let status = Status {
            v: "1".to_string(),
            instance_id: pic.instance_id,
            config_port,
            gateway_port,
            root_key: hex::encode(
                pic.root_key()
                    .await
                    .expect("root key should be available if there is a root subnet"),
            ),
            default_effective_canister_id: default_ecid,
        };
        let mut contents = serde_json::to_string(&status).expect("infallible serialization");
        contents.push('\n');
        fs::write(status_file, contents).context("failed to write status file")?;
    }
    eprintln!("pocket-ic instance running with gateway port {gateway_port}");
    let ctrlc = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(SignalKind::terminate())
            .context("failed to install SIGTERM handler")?;
        select! {
            res = ctrlc => res.context("failed to listen for ctrl-c")?,
            _ = sigterm.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        ctrlc.await.context("failed to listen for ctrl-c")?;
    }
    pic.drop().await;
    let pid = child.id().expect("child process should have an id") as usize;
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&[pid.into()]), true);
    if let Some(process) = sys.process(pid.into()) {
        process.kill_with(Signal::Interrupt);
    }
    select! {
        _ = child.wait() => {},
        _ = tokio::time::sleep(Duration::from_secs(5)) => {
            let _ = child.kill().await;
        }
    }
    Ok(())
}

fn get_errorchecked_args() -> Cli {
    let mut cli = Cli::parse();
    let mut command = Cli::command();
    // If no interface version is provided, normal behavior.
    let Some(interface_version) = &cli.interface_version else {
        if !cli.unknown_args.is_empty() {
            unknown_arg(&mut command, &cli.unknown_args[0]);
        }
        return cli;
    };
    let our_version = Version::parse("1.0.0").expect("valid version");
    // Backwards compatibility: if at all possible, the requirement should be kept at ^1.0.0 while retaining semver.
    let requirement = VersionReq::parse("^1.0.0").expect("valid version req");
    if !requirement.matches(interface_version) {
        eprintln!(
            "Error: Unsupported interface version {interface_version}. Supported versions: {requirement}",
        );
        std::process::exit(1);
    }
    // Forwards compatibility: unknown arguments for a newer version should be ignored rather than erroring.
    if !cli.unknown_args.is_empty() {
        if *interface_version == our_version {
            // If this is the exact same version, unknown args are bad args.
            unknown_arg(&mut command, &cli.unknown_args[0]);
        } else {
            // If this is a future version, unknown args are possibly correct.
            // It is a lot more likely to be misinput if the user is writing them (vs automation),
            // which is why the behavior is disabled without an explicit interface version,
            // since manual usage likely will not involve this flag.
            let mut unknown_args = vec![];
            while !cli.unknown_args.is_empty() {
                let mut prev_unknown_args = mem::take(&mut cli.unknown_args);
                unknown_args.push(prev_unknown_args.remove(0));
                cli.update_from(&prev_unknown_args);
            }
            eprintln!("Warning: Unknown launcher parameters: {unknown_args:?}");
        }
    }
    cli
}

fn unknown_arg(cmd: &mut clap::Command, arg: &str) -> ! {
    let mut err = clap::Error::new(clap::error::ErrorKind::UnknownArgument);
    err.insert(
        clap::error::ContextKind::InvalidArg,
        clap::error::ContextValue::String(arg.to_string()),
    );
    let err = err.format(cmd);
    err.exit();
}

#[cfg(unix)]
async fn try_with_maybe_muted_stderr<R>(
    verbose: bool,
    f: impl Future<Output = anyhow::Result<R>>,
) -> anyhow::Result<R> {
    use std::io::{Seek, SeekFrom};
    use std::sync::Arc;
    if verbose {
        f.await
    } else {
        let stderr = stderr().lock();
        let stderr_fd = nix::unistd::dup(&stderr).context("failed to dup stderr")?;
        let stderr_fd = Arc::new(stderr_fd);
        let logfile = NamedTempFile::new().context("failed to create temporary logfile")?;
        nix::unistd::dup2_stderr(logfile.as_file()).context("failed to mute stderr")?;
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new({
            let stderr_fd = Arc::clone(&stderr_fd);
            move |panic_info| {
                let _ = nix::unistd::dup2_stderr(&stderr_fd);
                hook(panic_info);
            }
        }));
        let result = f.await;
        _ = std::panic::take_hook();
        nix::unistd::dup2_stderr(&stderr_fd).context("failed to restore stderr")?;
        if result.is_err() {
            let mut log_contents = String::new();
            let logfile_read_result = logfile
                .as_file()
                .seek(SeekFrom::Start(0))
                .and_then(|_| logfile.as_file().read_to_string(&mut log_contents));
            match logfile_read_result {
                Ok(_) => {
                    if !log_contents.trim().is_empty() {
                        eprintln!(
                            "error occurred while stderr output was muted, reprinting:\n{}",
                            log_contents
                        );
                    }
                }
                Err(e) => {
                    eprintln!(
                        "error reprinting muted stderr output: failed to read temporary logfile: {}",
                        e
                    );
                    // still return original error
                }
            }
        }
        result
    }
}

#[cfg(not(unix))]
async fn try_with_maybe_muted_stderr<R>(
    verbose: bool,
    f: impl Future<Output = anyhow::Result<R>>,
) -> anyhow::Result<R> {
    f.await
}

#[derive(Serialize)]
struct Status {
    v: String,
    instance_id: usize,
    config_port: u16,
    gateway_port: u16,
    root_key: String,
    default_effective_canister_id: Principal,
}
