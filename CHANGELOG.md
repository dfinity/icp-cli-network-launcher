<!--
Convention: one section per date (`YYYY-MM-DD`), newest first, dated when the
change merged to main — update the date if your PR sits for a while.

Releases are a separate track: they are cut weekly to follow the bundled
pocket-ic, and the date inside a release version is the dfinity/ic release date
of that pocket-ic, which says nothing about when the launcher itself changed. For
what a given release contains, see its release notes on GitHub.

Only launcher behavior belongs here — CLI flags, defaults, subnet topology,
process lifecycle, the status file. Not pocket-ic bumps, Rust toolchain updates,
CI, or dependency bumps.
-->

# 2026-07-31

* fix: Shut the pocket-ic server down when the launcher fails to start, instead of leaving it running orphaned for the length of its 30-day TTL

# 2026-06-02

* feat: The NNS, fiduciary, and TestThresholdKeys subnets are now always created, independent of `--subnet`. TestThresholdKeys holds `test_key_1` and `dfx_test_key` for every threshold algorithm (ECDSA, Schnorr, VetKd), which as of pocket-ic 14 are no longer held by the II or fiduciary subnets

# 2026-03-23

* feat: Add a `cloud-engine` build feature, which adds `--subnet=cloud-engine`, and publish a Docker image built with it

# 2026-03-04

* feat: The status file now reports what the launcher supports in a `supported_features` field, so automated setups can detect optional features instead of inferring them from the version

# 2026-03-02

* feat: Add a `--custom-domains-file` flag pointing the HTTP gateway at a file of custom domain mappings. Defaults to `custom-domains.txt` in `--status-dir` when that is given

# 2026-02-27

* feat: Add a `--pocketic-config-bind` flag to bind the PocketIC admin interface separately from the HTTP gateway, which keeps using `--bind`

# 2026-02-18

* feat: Add a `--domain` flag to allow configuring additional domains for the gateway
