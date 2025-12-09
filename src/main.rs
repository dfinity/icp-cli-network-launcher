use std::{
    fs,
    io::ErrorKind,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    time::Duration,
};

use clap::{ArgAction, Parser, ValueEnum};
use ic_principal::Principal;
use notify::{Event, RecursiveMode, Watcher, recommended_watcher};
use pocket_ic::{
    PocketIcBuilder,
    common::rest::{AutoProgressConfig, IcpFeatures, IcpFeaturesConfig, InstanceHttpGatewayConfig},
};
use reqwest::Client;
use serde::Serialize;
use sysinfo::{ProcessesToUpdate, Signal, System};
use tempfile::TempDir;
use tokio::select;
use tokio::{process::Command, signal::unix::SignalKind};

#[derive(Parser)]
#[command(version)]
struct Cli {
    #[arg(long)]
    gateway_port: Option<u16>,
    #[arg(long)]
    config_port: Option<u16>,
    #[arg(long)]
    bind: Option<IpAddr>,
    #[arg(long)]
    state_dir: Option<PathBuf>,
    #[arg(long)]
    artificial_delay_ms: Option<u64>,
    #[arg(long, value_enum, action = ArgAction::Append)]
    subnet: Vec<SubnetKind>,
    #[arg(long, action = ArgAction::Append)]
    bitcoind_addr: Vec<SocketAddr>,
    #[arg(long, action = ArgAction::Append)]
    dogecoind_addr: Vec<SocketAddr>,
    #[arg(long)]
    ii: bool,
    #[arg(long)]
    nns: bool,
    #[arg(long)]
    pocketic_server_path: Option<PathBuf>,
    #[arg(long)]
    stdout_file: Option<PathBuf>,
    #[arg(long)]
    stderr_file: Option<PathBuf>,
    #[arg(long)]
    status_dir: Option<PathBuf>,
    #[arg(long)]
    verbose: bool,
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
async fn main() {
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
    } = Cli::parse();
    // pocket-ic is expected to be installed next to the launcher (see package.sh)
    let pocketic_server_path = if let Some(path) = pocketic_server_path {
        path
    } else {
        let assumed = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .join("pocket-ic");
        if !assumed.exists() {
            eprintln!(
                "Error: --pocketic-server-path not provided and could not find pocket-ic next to the launcher"
            );
            std::process::exit(1);
        }
        assumed
    };
    // We learn the port by pocket-ic writing it to a file
    let tmpdir = TempDir::new().unwrap();
    let port_file = tmpdir.path().join("pocketic.port");
    let (tx, mut rx) = tokio::sync::mpsc::channel(10);
    let mut watcher = recommended_watcher({
        let port_file = port_file.clone();
        move |event: Result<Event, notify::Error>| {
            event.unwrap();
            match fs::read_to_string(&port_file) {
                Ok(contents) => {
                    if contents.ends_with('\n') {
                        let port: u16 = contents.trim().parse().unwrap();
                        let _ = tx.blocking_send(port);
                    }
                }
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => panic!("Failed to read port file: {}", e),
            };
        }
    })
    .unwrap();
    watcher
        .watch(tmpdir.path(), RecursiveMode::Recursive)
        .unwrap();
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
        let file = std::fs::File::create(stdout_file).unwrap();
        cmd.stdout(file);
    }
    if let Some(stderr_file) = stderr_file {
        let file = std::fs::File::create(stderr_file).unwrap();
        cmd.stderr(file);
    }
    if !verbose {
        cmd.args(["--log-levels", "error"]);
    }
    #[cfg(unix)]
    {
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().unwrap();
    let config_port = rx.recv().await.unwrap();
    drop(watcher);
    // pocket-ic CLI setup ends here
    // initial HTTP setup
    let mut pic = PocketIcBuilder::new()
        .with_server_url(format!("http://127.0.0.1:{config_port}/").parse().unwrap())
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
        .unwrap();
    client
        .post(progress_url)
        .json(&AutoProgressConfig {
            artificial_delay_ms,
        })
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let topology = pic.topology().await;
    let default_ecid = Principal::from_slice(&topology.default_effective_canister_id.canister_id);
    let gateway_url = pic.url().unwrap();
    // write everything to the status file
    if let Some(status_dir) = status_dir {
        let status_file = status_dir.join("status.json");
        let status = Status {
            v: "1".to_string(),
            instance_id: pic.instance_id,
            config_port,
            gateway_port: gateway_url.port().unwrap(),
            root_key: hex::encode(pic.root_key().await.unwrap()),
            default_effective_canister_id: default_ecid,
        };
        let mut contents = serde_json::to_string(&status).unwrap();
        contents.push('\n');
        println!("launcher: writing status to {}", status_file.display());
        fs::write(status_file, contents).unwrap();
    }
    let ctrlc = tokio::signal::ctrl_c();
    let mut sigterm = tokio::signal::unix::signal(SignalKind::terminate()).unwrap();
    select! {
        _ = ctrlc => {},
        _ = sigterm.recv() => {},
    }
    pic.drop().await;
    let pid = child.id().unwrap() as usize;
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
