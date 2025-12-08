# icp-cli-network-launcher

This is a simple CLI interface for `pocket-ic`, tailored to the needs of `icp-cli`. It exposes just enough features via CLI flags to support diverse use-cases, but not so much complexity that future features will require any breaking changes.

The CLI interface should be stable across releases of `pocket-ic`, and the primary way `pocket-ic` is installed for use with `icp-cli` is by installing `icp-cli-network-launcher`. The downloadable package contains both the launcher and the `pocket-ic` binary it supports.

One version of the launcher is tied to one version of `pocket-ic`. If the `pocket-ic` version is a published version, then the launcher version will match, e.g. `10.0.0`. If the `pocket-ic` version is a git hash of the dfinity/ic repo, it is added as a tag after the most recent published version, e.g. `10.0.0+97ad9167`. The launcher expects to be in the same folder as its corresponding version of `pocket-ic`.

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
