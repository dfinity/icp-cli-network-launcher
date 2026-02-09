# icp-cli-network-launcher

Launches a local ICP test network powered by [PocketIC](https://github.com/dfinity/pocketic). Designed as the network backend for [`icp-cli`](https://github.com/dfinity/icp-cli), but can also be used standalone.

```sh
# Start a local network with default settings (application + NNS subnets)
icp-cli-network-launcher --gateway-port=4943

# Start with Bitcoin integration (implicitly enables Internet Identity)
icp-cli-network-launcher --gateway-port=4943 --bitcoind-addr=127.0.0.1:18444
```

The CLI interface is stable across PocketIC releases. The downloadable package contains both the launcher and the PocketIC binary it supports. One launcher version is tied to one PocketIC version — if the PocketIC version is a published release, the launcher version matches (e.g. `10.0.0`); if it's a git hash, it's added as a tag (e.g. `10.0.0+97ad9167`). The launcher expects the `pocket-ic` binary in the same directory.

## CLI Reference

### Flags

| Flag | Type | Description |
|------|------|-------------|
| `--gateway-port` | integer | Port for the HTTP gateway (ICP API). Random if not set. |
| `--config-port` | integer | Port for the PocketIC admin interface. Random if not set. |
| `--bind` | IP address | Network interface to bind the PocketIC server on. |
| `--state-dir` | path | Directory to persist PocketIC state across restarts. Ephemeral if not set. |
| `--artificial-delay-ms` | integer | Artificial delay for update call execution (ms). |
| `--subnet` | enum | Subnet to add. Can be specified multiple times. See [Subnet Configuration](#subnet-configuration). |
| `--bitcoind-addr` | host:port | Bitcoin P2P node address. Can be specified multiple times. See [Bitcoin/Dogecoin](#bitcoin-and-dogecoin-integration). |
| `--dogecoind-addr` | host:port | Dogecoin P2P node address. Can be specified multiple times. See [Bitcoin/Dogecoin](#bitcoin-and-dogecoin-integration). |
| `--ii` | flag | Enable Internet Identity. See [Internet Identity](#internet-identity). |
| `--nns` | flag | Enable NNS and SNS. See [NNS](#nns). |
| `--interface-version` | semver | Expected CLI interface version. Also read from `ICP_CLI_NETWORK_LAUNCHER_INTERFACE_VERSION` env var. Used by automated setups. |
| `--status-dir` | path | Directory to write the [status file](#status-file) to. Used by automated setups. |
| `--verbose` | flag | Enable verbose logging from PocketIC. |

### Subnet Configuration

The `--subnet` flag controls which subnets the local network includes. Available types: `application`, `system`, `verified-application`, `bitcoin`, `fiduciary`, `nns`, `sns`.

**Default behavior:**
- With no `--subnet` flags: one **application** subnet is created.
- With any `--subnet` flag: the default application subnet is **not** created. Only explicitly specified subnets are added.
- An **NNS** subnet is **always** created regardless of flags (it is required for system operations).

**Examples:**

```sh
# Default: application + NNS
icp-cli-network-launcher

# Two application subnets + NNS (for cross-subnet testing)
icp-cli-network-launcher --subnet=application --subnet=application

# System + application + NNS
icp-cli-network-launcher --subnet=system --subnet=application

# Only system + NNS (no application subnet!)
icp-cli-network-launcher --subnet=system
```

### Internet Identity

The `--ii` flag creates an II subnet and installs the Internet Identity canister.

The II subnet and canister are also **implicitly enabled** when any of the following are used:
- `--nns`
- `--bitcoind-addr`
- `--dogecoind-addr`

This is because the II subnet provides threshold signature keys (tECDSA/tSchnorr) that are required for Bitcoin and Dogecoin signing operations, and by NNS/SNS governance. Using `--ii` explicitly alongside these flags is valid but redundant.

### NNS

The `--nns` flag installs the NNS (Network Nervous System) and SNS (Service Nervous System). It has the following effects:

- Creates an **SNS** subnet
- Creates an **II** subnet (implies `--ii`)
- Enables NNS governance, NNS UI, SNS, and canister migration features

### Bitcoin and Dogecoin Integration

The `--bitcoind-addr` and `--dogecoind-addr` flags connect the network to external Bitcoin or Dogecoin nodes for chain integration testing.

**Implicit effects:**
- A **bitcoin** subnet is automatically created (shared by both Bitcoin and Dogecoin)
- An **II** subnet is automatically created (provides threshold signing keys)
- The respective chain feature (bitcoin or dogecoin) is enabled

**Address format:** `host:port` — this is the P2P address of the node, not the RPC address.

**Examples:**

```sh
# Connect to a local Bitcoin node
icp-cli-network-launcher --bitcoind-addr=127.0.0.1:18444

# Connect to both Bitcoin and Dogecoin nodes
icp-cli-network-launcher --bitcoind-addr=127.0.0.1:18444 --dogecoind-addr=127.0.0.1:22556

# Multiple Bitcoin nodes
icp-cli-network-launcher --bitcoind-addr=127.0.0.1:18444 --bitcoind-addr=192.168.1.5:18444
```

### Interaction Summary

The following table summarizes the subnets created for common configurations. An NNS subnet is always present.

| Configuration | Subnets created |
|--------------|----------------|
| *(no flags)* | application, NNS |
| `--ii` | application, NNS, II |
| `--nns` | application, NNS, II, SNS |
| `--bitcoind-addr=...` | application, NNS, bitcoin, II |
| `--dogecoind-addr=...` | application, NNS, bitcoin, II |
| `--subnet=system` | system, NNS |
| `--subnet=system --bitcoind-addr=...` | system, NNS, bitcoin, II |
| `--nns --subnet=system` | system, NNS, II, SNS |

**Key points:**
- Specifying any `--subnet` flag replaces the default application subnet. Add `--subnet=application` explicitly if you still need it.
- `--bitcoind-addr` and `--dogecoind-addr` always add a bitcoin subnet and an II subnet, regardless of `--subnet` flags.
- `--nns` always adds an SNS subnet and an II subnet, regardless of `--subnet` flags.

### Status File

When `--status-dir` is provided, the launcher writes a JSON status file (`status.json`) to the specified directory once the network is ready. This is used by `icp-cli` and other automated setups to discover the running network.

| Field | Type | Description |
|-------|------|-------------|
| `v` | string | Status file format version. Currently `"1"`. |
| `gateway_port` | number | Port where the HTTP gateway (ICP API) is listening. |
| `root_key` | string | Hex-encoded root key of the network. |
| `config_port` | number | Port of the PocketIC admin interface. |
| `instance_id` | number | PocketIC instance ID. |
| `default_effective_canister_id` | string | Default effective canister ID for provisional canister creation calls. |

### Shutdown

The launcher handles `SIGINT` (Ctrl+C) and `SIGTERM` for graceful shutdown. It stops the PocketIC server and waits for it to exit before terminating.

## Development

### Prerequisites

* Rust v1.90 or later. If you have Rustup installed it will automatically use the right version.
* Bash, jq, and curl for the `package.sh` script.

### Building

```sh
./package.sh [directory]
```

This will build the code, download the appropriate version of pocket-ic, and place it in a destination folder. If you do not supply a folder it will use `dist/icp-cli-network-launcher-<VERSION>` and additionally create a tarball.

## License

This project is licensed under the [Apache-2.0](./LICENSE) license.

## Contribution

This project does not accept external contributions. Pull requests from individuals outside the organization will be automatically closed.
