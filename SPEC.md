# Interface spec

The CLI interface of `icp-cli-network-launcher` is stable across revisions. Interface versions are versioned according to [SemVer](https://semver.org).

All interface versions accept an `--interface-version=<VER>` flag and an `ICP_CLI_NETWORK_LAUNCHER_INTERFACE_VERSION=<ver>` environment variable (the flag takes precedence). When one of these is provided, all flags and output are according to this spec.

Backwards compatibility is supported in both directions. A launcher developed for the 1.1 interface supports the 1.0 interface, but also does its best to support the 1.2 interface; if 1.1 is the latest launcher version, and 1.2 is requested, all unknown flags will be treated as warnings instead of errors (this behavior does not apply when the same version or an earlier version is requested). Accordingly, new flags which alter the behavior of existing flags will require a major SemVer bump, not minor.

## Interface v1.0.0

### Flags

The following flags are accepted by the CLI. All flags are optional.

* `--gateway-port=<PORT>`: Specifies the port to host the ICP gateway API on.
* `--config-port=<PORT>`: Specifies the port to host the PocketIC config interface on.
* `--bind=<IP>`: Specifies the IP address/network interface to bind both ports to.
* `--state-dir=<DIR>`: Specifies the directory that PocketIC's state should be saved to/loaded from.
* `--artificial-delay-ms=<DELAY_MS>`: In milliseconds, specifies an artificial network latency that update calls should incur.
* `--subnet=<KIND>`†: Adds a subnet of kind `KIND` to the subnet list. This flag is repeatable. Valid subnet kinds are `application`, `system`, `verified-application`, `bitcoin`, `fiduciary`, `nns`, and `sns`. Regardless of flags, the system subnet will be created. If no flags are specified, one application subnet is created.
* `--bitcoind-addr=<ADDR>`: Specifies a bitcoind node to connect to, enables regtest Bitcoin support, and implies `--subnet=bitcoin`. This flag is repeatable.
* `--dogecoind-addr=<ADDR>`: Specifies a dogecoind node to connect to, enables regtest Dogecoin support, and implies `--subnet=bitcoin`. This flag is repeatable.
* `--ii`†: Installs the Internet Identity canister.
* `--nns`†: Installs the NNS and SNS. Implies `--ii` and `--subnet=sns`.
* `--pocketic-server-path=<PATH>`: Overrides the path to the pocket-ic server binary.
* `--stdout-file=<FILE>`: Specifies a file to redirect pocket-ic's stdout to.
* `--stderr-file=<FILE>`: Specifies a file to redirect pocket-ic's stderr to.
* `--status-dir=<DIR>`: Specifies the status directory (see below).
* `--verbose`: Enables verbose logging from pocket-ic. What 'verbose' means is not spec-defined.

†These flags represent opt-in features. A compliant binary must do what they imply when they are specified, but will not necessarily require them to be specified to do what they imply; their effect is allowed to be default behavior.

### Output

The required output of the launcher is in the form of the status directory, specified via CLI flag. The launcher writes a file `status.json` to this directory, containing a JSON object with the following fields:

- `v`: string, always `"1"`.
- `gateway_port`: uint, container-side port of the ICP HTTP gateway.
- `root_key`: string, hex-encoded root key of the network.
- `config_port`: uint, the pocket-ic server's configuration port.
- `instance_id`: uint, the ID of the pocket-ic instance.
- `default_effective_canister_id`: string, the principal that provisional management canister methods should be called under.

This file is written when the network is ready to be connected to, and not before.

### Behavior

A network successfully created by the launcher has a functional cycles minting canister, cycles ledger, and ICP ledger installed. The anonymous principal `2vxsx-fae` has an unspecified but very large amount of both ICP and cycles.

When the launcher receives the signal `SIGINT` (or `CTRL_C_EVENT` on Windows), it gracefully shuts down the PocketIC instance, preserving state. 
