#![cfg_attr(not(feature = "cloud-engine"), allow(unused))]

use std::{
    collections::HashSet,
    fs,
    io::{ErrorKind, Read, stderr},
    mem,
    net::{IpAddr, SocketAddr},
    time::{Duration, Instant},
};

use anyhow::Context;
use camino::{Utf8Path, Utf8PathBuf};
use clap::{ArgAction, CommandFactory, Parser, ValueEnum};
use ic_principal::Principal;
use notify::{Event, RecursiveMode, Watcher, recommended_watcher};
use pocket_ic::{
    PocketIcBuilder,
    common::rest::{
        AutoProgressConfig, ExtendedSubnetConfigSet, IcpFeatures, IcpFeaturesConfig,
        InstanceHttpGatewayConfig, SubnetSpec,
    },
};
use reqwest::Client;
use semver::{Version, VersionReq};
use serde::Serialize;
use tempfile::{NamedTempFile, TempDir};
use tokio::process::Child;
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
    #[arg(long)]
    pocketic_config_bind: Option<IpAddr>,
    /// Directory to store the PocketIC state.
    #[arg(long)]
    state_dir: Option<Utf8PathBuf>,
    /// Artificial delay for execution, in milliseconds.
    #[arg(long)]
    artificial_delay_ms: Option<u64>,
    /// List of workload subnets to create. Defaults to `--subnet=application` when none are specified. The NNS, fiduciary, and test-threshold-keys subnets are always created regardless of this flag.
    #[arg(long, value_enum, action = ArgAction::Append)]
    subnet: Vec<SubnetKind>,
    /// Addresses of bitcoind nodes to connect to (e.g. 127.0.0.1:18444 or bitcoind:18444).
    /// Implies `--subnet=bitcoin`.
    #[arg(long, action = ArgAction::Append)]
    bitcoind_addr: Vec<String>,
    /// Addresses of dogecoind nodes to connect to (e.g. 127.0.0.1:22556 or dogecoind:22556).
    /// Implies `--subnet=bitcoin`.
    #[arg(long, action = ArgAction::Append)]
    dogecoind_addr: Vec<String>,
    /// Domain names for the HTTP gateway. "localhost" is always included.
    #[arg(long, action = ArgAction::Append)]
    domain: Vec<String>,
    /// Path to a file containing custom domain mappings for the HTTP gateway.
    /// Defaults to <status_dir>/custom-domains.txt if --status-dir is provided.
    #[arg(long)]
    custom_domains_file: Option<Utf8PathBuf>,
    /// Installs the Internet Identity canister.
    #[arg(long)]
    ii: bool,
    /// Installs the NNS and SNS. Implies `--ii` and `--subnet=sns`.
    #[arg(long)]
    nns: bool,
    /// Path to the pocket-ic server binary. By default, looks for `pocket-ic` next to the launcher.
    /// The launcher is unlikely to be usable with a different version than it shipped with.
    #[arg(long, env = "ICP_CLI_NETWORK_LAUNCHER_POCKETIC_SERVER_PATH")]
    pocketic_server_path: Option<Utf8PathBuf>,
    /// File to redirect pocket-ic stdout to.
    #[arg(long)]
    stdout_file: Option<Utf8PathBuf>,
    /// File to redirect pocket-ic stderr to.
    #[arg(long)]
    stderr_file: Option<Utf8PathBuf>,
    /// Directory to write status signal files to. Used by automated setups.
    #[arg(long)]
    status_dir: Option<Utf8PathBuf>,
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
    Sns,
    /// Accepted for backward compatibility but ignored: the NNS subnet is always created.
    Nns,
    /// Accepted for backward compatibility but ignored: the fiduciary subnet is always created.
    Fiduciary,
    #[cfg(feature = "cloud-engine")]
    CloudEngine,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let Cli {
        gateway_port,
        config_port,
        bind,
        pocketic_config_bind,
        state_dir,
        artificial_delay_ms,
        subnet,
        bitcoind_addr,
        dogecoind_addr,
        domain,
        custom_domains_file,
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
        assumed.try_into()?
    };

    // pocket-ic produces a lot of output so we're going to mute stderr for a moment
    let (pic, pocketic, topology, config_port) = try_with_maybe_muted_stderr(verbose, async {
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
        if let Some(ip_addr) = pocketic_config_bind {
            cmd.arg("--ip-addr").arg(ip_addr.to_string());
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
        // Take ownership of the process before anything else can fail: from here on
        // every `?` (and every panic) has to take pocket-ic down with it.
        let pocketic = PocketIcProcess {
            child: cmd
                .spawn()
                .context("failed to spawn pocket-ic server process")?,
        };
        let config_port = rx
            .recv()
            .await
            .expect("failed to receive port from watcher")?;
        drop(watcher);
        // pocket-ic CLI setup ends here
        // initial HTTP setup
        let mut base_subnets = ExtendedSubnetConfigSet::default();
        #[cfg(feature = "cloud-engine")]
        for _ in 0..subnet
            .iter()
            .filter(|s| matches!(s, SubnetKind::CloudEngine))
            .count()
        {
            use pocket_ic::common::rest::CanisterCyclesCostSchedule;

            base_subnets.cloud_engine.push(
                SubnetSpec::default()
                    .with_subnet_admins(vec![Principal::anonymous()])
                    .with_cost_schedule(CanisterCyclesCostSchedule::Free),
            );
        }
        let mut pic = PocketIcBuilder::new_with_config(base_subnets)
            .with_server_url(
                format!("http://127.0.0.1:{config_port}/")
                    .parse()
                    .expect("valid url"),
            )
            .with_http_gateway(InstanceHttpGatewayConfig {
                ip_addr: bind.map(|ip| ip.to_string()),
                port: gateway_port,
                domains: Some({
                    let mut domains: HashSet<String> = domain.into_iter().collect();
                    domains.insert("localhost".to_string());
                    domains.into_iter().collect()
                }),
                https_config: None,
                domain_custom_provider_local_file: custom_domains_file
                    .or_else(|| {
                        status_dir
                            .as_ref()
                            .map(|dir| dir.join("custom-domains.txt"))
                    })
                    .map(|pth| pth.into_string()),
            });
        if let Some(dir) = state_dir {
            pic = pic.with_state_dir(dir.into());
        }
        // Always-on base topology: mirrors the mainnet subnet layout and provides
        // infrastructure. Created unconditionally, independent of --subnet.
        pic = pic.with_nns_subnet();
        pic = pic.with_fiduciary_subnet();
        // TestThresholdKeys holds test_key_1 and dfx_test_key for all threshold algorithms
        // (ECDSA, Schnorr, VetKd). As of pocket-ic 14.0.0 these keys are no longer held by
        // the II or fiduciary subnets.
        pic = pic.with_test_threshold_keys_subnet();
        // Workload subnets selected via --subnet. With no --subnet, a single application
        // subnet is created.
        if subnet.is_empty() {
            pic = pic.with_application_subnet();
        } else {
            for subnet in subnet {
                match subnet {
                    SubnetKind::Application => pic = pic.with_application_subnet(),
                    SubnetKind::System => pic = pic.with_system_subnet(),
                    SubnetKind::VerifiedApplication => pic = pic.with_verified_application_subnet(),
                    SubnetKind::Bitcoin => pic = pic.with_bitcoin_subnet(),
                    SubnetKind::Sns => pic = pic.with_sns_subnet(),
                    // Part of the always-on base topology above; accepted for backward
                    // compatibility but ignored here.
                    SubnetKind::Nns | SubnetKind::Fiduciary => {}
                    #[cfg(feature = "cloud-engine")]
                    SubnetKind::CloudEngine => {} // handled above
                }
            }
        }
        // --bitcoind-addr and --dogecoind-addr imply --subnet=bitcoin
        if !bitcoind_addr.is_empty() || !dogecoind_addr.is_empty() {
            pic = pic.with_bitcoin_subnet();
        }
        let mut features = IcpFeatures {
            cycles_minting: Some(IcpFeaturesConfig::DefaultConfig),
            icp_token: Some(IcpFeaturesConfig::DefaultConfig),
            cycles_token: Some(IcpFeaturesConfig::DefaultConfig),
            registry: Some(IcpFeaturesConfig::DefaultConfig),
            ..<_>::default()
        };
        // II subnet and canister are needed for NNS/SNS governance and Internet Identity.
        // Threshold signature keys (tECDSA/tSchnorr/VetKd) are provided by the TestThresholdKeys
        // subnet, which is always enabled — Bitcoin/Dogecoin signing does not require II.
        if nns || ii {
            pic = pic.with_ii_subnet();
            features.ii = Some(IcpFeaturesConfig::DefaultConfig);
        }
        if nns {
            pic = pic.with_sns_subnet();
            features.nns_governance = Some(IcpFeaturesConfig::DefaultConfig);
            features.nns_ui = Some(IcpFeaturesConfig::DefaultConfig);
            features.sns = Some(IcpFeaturesConfig::DefaultConfig);
            features.canister_migration = Some(IcpFeaturesConfig::DefaultConfig);
        }
        if !bitcoind_addr.is_empty() {
            features.bitcoin = Some(IcpFeaturesConfig::DefaultConfig);
        }
        if !dogecoind_addr.is_empty() {
            features.dogecoin = Some(IcpFeaturesConfig::DefaultConfig);
        }
        pic = pic.with_icp_features(features);
        if !bitcoind_addr.is_empty() {
            let addrs = resolve_addrs(&bitcoind_addr)
                .await
                .context("failed to resolve --bitcoind-addr")?;
            pic = pic.with_bitcoind_addrs(addrs);
        }
        if !dogecoind_addr.is_empty() {
            let addrs = resolve_addrs(&dogecoind_addr)
                .await
                .context("failed to resolve --dogecoind-addr")?;
            pic = pic.with_dogecoind_addrs(addrs);
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
        Ok((pic, pocketic, topology, config_port))
    })
    .await?;
    let default_ecid = Principal::from_slice(&topology.default_effective_canister_id.canister_id);
    let gateway_url = pic.url().expect("gateway url set in builder");
    let gateway_port = gateway_url
        .port_or_known_default()
        .expect("gateway urls should have a known port");
    // write everything to the status file
    if let Some(status_dir) = &status_dir {
        fs::create_dir_all(status_dir).context("failed to create status directory")?;
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
            supported_features: vec!["custom-domains".to_string()],
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
    // Explicit: the network has to be gone before the status directory is, since
    // its absence is how automated setups learn the network stopped.
    drop(pocketic);
    if let Some(status_dir) = &status_dir {
        remove_status_dir(status_dir)?;
    }
    Ok(())
}

/// Empties the status directory, and removes the directory itself if it can.
///
/// The contents are the part that has to go: automated setups learn the network
/// stopped from `status.json` disappearing. The directory itself frequently
/// cannot be removed at all — in container mode it is a bind mount from the host
/// (`--status-dir=/app/status` in the published images), and `rmdir` on a mount
/// point always fails with `EBUSY`. Insisting on it made every containerized
/// shutdown report "failed to remove status directory" and exit nonzero, for a
/// directory that was already empty.
///
/// Removing it only when the launcher created it would not work either: the
/// native path passes `--status-dir` too, and hands over a temp dir that nothing
/// but the launcher will ever clean up. Best-effort keeps that contract intact
/// while tolerating a mount point.
fn remove_status_dir(status_dir: &Utf8Path) -> anyhow::Result<()> {
    let entries = match fs::read_dir(status_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).context("failed to read status directory"),
    };
    for entry in entries {
        let entry = entry.context("failed to read status directory")?;
        let path = entry.path();
        // status.json and the caller's custom domains file are the expected
        // contents, but nothing rules out a subdirectory.
        let is_dir = entry
            .file_type()
            .with_context(|| format!("failed to stat {}", path.display()))?
            .is_dir();
        let removed = if is_dir {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        match removed {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => {
                return Err(e).with_context(|| {
                    format!(
                        "failed to remove {} from the status directory",
                        path.display()
                    )
                });
            }
        }
    }
    match fs::remove_dir(status_dir) {
        Ok(()) => {}
        // `ResourceBusy` is the bind mount above; `DirectoryNotEmpty` means
        // something wrote to the directory while it was being emptied. Neither is
        // the launcher's to fix, and either way the contents are gone.
        Err(e)
            if matches!(
                e.kind(),
                ErrorKind::NotFound | ErrorKind::ResourceBusy | ErrorKind::DirectoryNotEmpty
            ) => {}
        // Anything else leaks an empty directory — worth reporting, not worth
        // failing a shutdown that otherwise went fine.
        Err(e) => eprintln!("Warning: failed to remove status directory {status_dir}: {e}"),
    }
    Ok(())
}

/// How long pocket-ic gets to shut down on its own before it is killed.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Owns the spawned pocket-ic server and shuts it down when dropped.
///
/// The launcher takes explicit ownership of pocket-ic's lifecycle: pocket-ic is
/// spawned into its own process group and runs with a 30-day `--ttl`, so nothing
/// else will ever clean it up. Doing the shutdown in `Drop` is what makes that
/// ownership airtight — one code path covers a normal ctrl-c shutdown, an error
/// returned from startup, and a panic unwind alike. While only the happy path
/// signalled pocket-ic, any failure after the spawn (e.g. the HTTP gateway port
/// already being in use) left the server running orphaned for those 30 days.
struct PocketIcProcess {
    child: Child,
}

impl Drop for PocketIcProcess {
    fn drop(&mut self) {
        // Blocking rather than async: `Drop` cannot await, and this only ever runs
        // on the launcher's way out.
        #[cfg(unix)]
        {
            use nix::{
                sys::signal::{Signal, kill, killpg},
                unistd::Pid,
            };

            // `id()` is `None` only once the process has been reaped, which nothing
            // else here does.
            let Some(pid) = self.child.id().map(|id| Pid::from_raw(id as i32)) else {
                return;
            };
            // SIGINT goes to pocket-ic alone, deliberately not to its process group:
            // its own SIGINT handler tears the canister sandboxes down, and killing
            // them out from under it makes an internal panic — and so a *failed*
            // graceful shutdown — more likely.
            warn_unless_gone("SIGINT to pocket-ic", kill(pid, Signal::SIGINT));
            self.wait_for_grace_period();
            // Whatever is still alive after the grace period gets SIGKILLed, and
            // this time group-wide: pocket-ic leads its own process group, so this
            // also reaps sandboxes it never got around to. Once pocket-ic has shut
            // down gracefully the group is empty and this is a no-op.
            warn_unless_gone(
                "SIGKILL to the pocket-ic process group",
                killpg(pid, Signal::SIGKILL),
            );
        }
        #[cfg(not(unix))]
        {
            // Deliberately unimplemented rather than approximated. pocket-ic expects
            // CTRL_C_EVENT on Windows for the graceful shutdown that tears its
            // sandboxes down, which `start_kill` (TerminateProcess) does not deliver,
            // and there are no process groups to fall back on. This crate does not
            // build for Windows anyway — see the unconditional `tokio::signal::unix`
            // import — so fail loudly if that ever changes instead of shipping a
            // shutdown that looks right and isn't.
            compile_error!("PocketIcProcess has no Windows shutdown path; see the comment above");
        }
        // Reap the server so it doesn't linger as a zombie on the happy path,
        // where the launcher stays alive long enough afterwards to matter.
        let _ = self.child.try_wait();
    }
}

impl PocketIcProcess {
    /// Blocks until the server exits, giving up after [`SHUTDOWN_GRACE`].
    fn wait_for_grace_period(&mut self) {
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        loop {
            // A wait that errors won't start succeeding, so treat it like an exit.
            if !matches!(self.child.try_wait(), Ok(None)) {
                return;
            }
            if Instant::now() >= deadline {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Reports a failed signal, except for an already-gone target — which is the
/// outcome the signal was after anyway.
#[cfg(unix)]
fn warn_unless_gone(what: &str, result: nix::Result<()>) {
    match result {
        Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
        Err(e) => eprintln!("Warning: failed to send {what}: {e}"),
    }
}

/// Resolves a list of address strings (hostname:port or ip:port) to socket addresses.
async fn resolve_addrs(addrs: &[String]) -> anyhow::Result<Vec<SocketAddr>> {
    let mut resolved = Vec::with_capacity(addrs.len());
    for addr in addrs {
        let socket_addr = tokio::net::lookup_host(addr)
            .await
            .with_context(|| format!("failed to resolve address '{addr}'"))?
            .next()
            .with_context(|| format!("no addresses found for '{addr}'"))?;
        resolved.push(socket_addr);
    }
    Ok(resolved)
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
    let our_version = Version::parse("1.1.0").expect("valid version");
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
    supported_features: Vec<String>,
}

#[cfg(test)]
mod tests {
    use std::fs;

    use camino::{Utf8Path, Utf8PathBuf};
    use tempfile::TempDir;

    use super::remove_status_dir;

    /// A status directory as a running launcher leaves it, plus a subdirectory:
    /// only `--custom-domains-file` can put anything but a file in here, but
    /// nothing stops a caller from doing so.
    fn populated_status_dir(parent: &Utf8Path) -> Utf8PathBuf {
        let status_dir = parent.join("status");
        fs::create_dir(&status_dir).expect("failed to create status directory");
        fs::write(status_dir.join("status.json"), "{}\n").expect("failed to write status file");
        fs::write(status_dir.join("custom-domains.txt"), "")
            .expect("failed to write custom domains file");
        fs::create_dir(status_dir.join("nested")).expect("failed to create nested directory");
        fs::write(status_dir.join("nested/file"), "").expect("failed to write nested file");
        status_dir
    }

    fn utf8_tempdir() -> (TempDir, Utf8PathBuf) {
        let dir = TempDir::new().expect("failed to create temporary directory");
        let path = Utf8Path::from_path(dir.path())
            .expect("temporary directory should be utf8")
            .to_owned();
        (dir, path)
    }

    #[test]
    fn removes_the_status_dir_and_everything_in_it() {
        let (_tmp, tmp_path) = utf8_tempdir();
        let status_dir = populated_status_dir(&tmp_path);

        remove_status_dir(&status_dir).expect("removing the status directory should succeed");

        assert!(!status_dir.exists(), "{status_dir} was left behind");
    }

    #[test]
    fn an_already_gone_status_dir_is_not_an_error() {
        let (_tmp, tmp_path) = utf8_tempdir();

        remove_status_dir(&tmp_path.join("never-created"))
            .expect("a missing status directory should not be an error");
    }

    /// A stand-in for the bind mount of container mode, where the `rmdir` cannot
    /// succeed but the contents still have to go. Emptying it is what tells
    /// callers the network stopped, so that has to happen — and it must not be
    /// reported as a failed shutdown.
    #[cfg(unix)]
    #[test]
    fn empties_a_status_dir_that_cannot_be_removed() {
        use std::{fs::Permissions, os::unix::fs::PermissionsExt};

        let (_tmp, tmp_path) = utf8_tempdir();
        let status_dir = populated_status_dir(&tmp_path);
        // Removing a directory means writing to its parent, so a read-only parent
        // makes the final rmdir fail while leaving the directory's own contents
        // removable.
        fs::set_permissions(&tmp_path, Permissions::from_mode(0o555))
            .expect("failed to make the parent directory read-only");

        let result = remove_status_dir(&status_dir);

        // Before the assertions: the TempDir cannot clean itself up otherwise.
        fs::set_permissions(&tmp_path, Permissions::from_mode(0o755))
            .expect("failed to restore the parent directory permissions");
        result.expect("an unremovable status directory should not fail the shutdown");
        assert!(
            status_dir.exists(),
            "{status_dir} should still be there - the test's premise is that it cannot be removed"
        );
        let leftovers: Vec<String> = fs::read_dir(&status_dir)
            .expect("failed to read the status directory")
            .map(|entry| {
                entry
                    .expect("failed to read a status directory entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert!(
            leftovers.is_empty(),
            "the status directory was not emptied: {}",
            leftovers.join(", ")
        );
    }
}
