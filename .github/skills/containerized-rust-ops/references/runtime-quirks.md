# Runtime quirks

## Local Podman workflow

- Mount the repository root with an absolute host path. Relative binds can resolve to the wrong worktree in this control-plane environment.
- Resolve persistent cache paths with `git rev-parse --git-path ...` so the same commands work for standard clones and Git worktrees.
- In this control-plane environment, `CONTAINER_HOST` can point at a stale rootful Podman socket. Use local Podman by clearing that variable for repo-local runs.
- For local rootless Podman here, do not force `--user 1000:1000`; container root already maps back to the host user and explicit IDs break writes into bind-mounted cache paths.
- Use `sh -c` inside `docker.io/rust:1.94.0-bookworm`. `bash -lc` in that image drops the Rust toolchain from `PATH`.
- Keep `rustup`, `cargo`, `target`, and `sccache` outside the worktree contents but inside the repo's Git-managed cache area. This lets repeated runs reuse both downloaded dependencies and compiled artifacts.

## Control-plane Kubernetes workflow

- `control-plane-run --workspace <host-path>` does not mount the requested host worktree into Kubernetes jobs in this environment.
- `CONTROL_PLANE_JOB_NAMESPACE` lacks the PVC-backed `/workspace` mount that long-running jobs need.
- `CONTROL_PLANE_K8S_NAMESPACE` does expose the `/workspace` PVC, but the default runtime env points to a missing `control-plane-job` service account there. Clear `CONTROL_PLANE_JOB_SERVICE_ACCOUNT` before starting the job.
- Clone the pushed branch into `/workspace/src/<repo>/<branch>` and keep persistent caches in `/workspace/cache/<repo>/<branch>`. Unpushed local changes are not visible to the job.
- `k8s-rust.sh` injects its local `install-sccache.sh` into the job so bootstrap changes can be validated before push, but the Rust source build/test still runs against the pushed branch clone.
- The default `control-plane-run` job limit is `2Gi` memory here. First-time `cargo install --locked sccache` can be OOM-killed unless it is serialized with `CARGO_BUILD_JOBS=1` (or `SCCACHE_BOOTSTRAP_JOBS=1` when using `k8s-rust.sh`).
- The helper scripts now prefer prebuilt `sccache` release tarballs from `https://github.com/mozilla/sccache/releases/`. `SCCACHE_VERSION` and `SCCACHE_RELEASE_BASE_URL` can override the download source, and unsupported architectures still fall back to serialized `cargo install --locked sccache`.
- For `cargo llvm-cov`, do not use `cargo install`. Use `.github/skills/containerized-rust-ops/scripts/install-cargo-llvm-cov.sh`, which downloads the prebuilt binary from `https://github.com/taiki-e/cargo-llvm-cov/releases/`. `CARGO_LLVM_COV_VERSION`, `CARGO_LLVM_COV_RELEASE_BASE_URL`, and `CARGO_LLVM_COV_ARCHIVE_URL` can override the source, but non-default archives or versions must also set `CARGO_LLVM_COV_ARCHIVE_SHA256`.

## Cache layout

- Podman host caches:
  - `.copilot-cache/podman-rustup`
  - `.copilot-cache/podman-cargo`
  - `.copilot-cache/podman-target`
  - `.copilot-cache/podman-sccache`
- Kubernetes PVC caches:
  - `/workspace/cache/<repo>/<branch>/rustup`
  - `/workspace/cache/<repo>/<branch>/cargo`
  - `/workspace/cache/<repo>/<branch>/target`
  - `/workspace/cache/<repo>/<branch>/sccache`
- Set `RUSTC_WRAPPER=sccache`, `CARGO_TARGET_DIR` to a persistent location, and `CARGO_INCREMENTAL=0` so repeated runs favor reusable cache hits over per-run incremental artifacts.
- When invoking `cargo llvm-cov`, also ensure the toolchain has `llvm-tools-preview` installed before running coverage.

## Choose the right path

- Prefer Podman for current-worktree linting, checking, and debugging.
- Prefer control-plane Kubernetes jobs for long-running `cargo build` and `cargo test` commands after the branch has been pushed.
