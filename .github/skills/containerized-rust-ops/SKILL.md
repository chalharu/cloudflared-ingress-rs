---
name: containerized-rust-ops
description: Run this repository's Rust fmt/check/clippy/build/test through local Podman or control-plane Kubernetes jobs with the verified mount, namespace, and cache workarounds. Use when validating Rust changes without a host toolchain, rerunning long cargo build/test commands on the control plane, or debugging containerized Rust workflows in this repository.
---

# Containerized Rust Ops

Use the bundled scripts instead of rebuilding Podman or `control-plane-run` commands by hand.

## Choose the workflow

1. Need local lint, check, clippy, build, or test against the current worktree? Run `scripts/podman-rust.sh`.
2. Need a long-running build or test on the control plane? Run `scripts/k8s-rust.sh`.
3. Need to understand why a containerized run is behaving strangely? Read `references/runtime-quirks.md` before changing the commands.
4. Need `cargo llvm-cov`? Use the bundled release-bootstrap path instead of `cargo install`; the helper scripts install `cargo-llvm-cov` from GitHub releases when you invoke `cargo llvm-cov`.

## Run local Podman validation

Run these commands from the repository root:

- `bash .github/skills/containerized-rust-ops/scripts/podman-rust.sh fmt-check`
- `bash .github/skills/containerized-rust-ops/scripts/podman-rust.sh check`
- `bash .github/skills/containerized-rust-ops/scripts/podman-rust.sh clippy`
- `bash .github/skills/containerized-rust-ops/scripts/podman-rust.sh build`
- `bash .github/skills/containerized-rust-ops/scripts/podman-rust.sh test`
- `bash .github/skills/containerized-rust-ops/scripts/podman-rust.sh -- cargo test --workspace --all-targets -- --nocapture`
- `bash .github/skills/containerized-rust-ops/scripts/podman-rust.sh -- cargo llvm-cov --workspace --all-targets --summary-only`

The script always uses an absolute bind mount for the repository root and persists `rustup`, `cargo`, `target`, and `sccache` under `git rev-parse --git-path .copilot-cache/...`.

## Run control-plane Kubernetes validation

Use this only after the branch state you want is on `origin`, because the job clones from the remote branch into the PVC-backed workspace instead of reading the local worktree.

Run:

- `bash .github/skills/containerized-rust-ops/scripts/k8s-rust.sh build`
- `bash .github/skills/containerized-rust-ops/scripts/k8s-rust.sh test`
- `bash .github/skills/containerized-rust-ops/scripts/k8s-rust.sh -- cargo test --workspace --all-targets -- --nocapture`
- `bash .github/skills/containerized-rust-ops/scripts/k8s-rust.sh -- cargo llvm-cov --workspace --all-targets --summary-only`

The script targets `CONTROL_PLANE_K8S_NAMESPACE`, clears the broken `CONTROL_PLANE_JOB_SERVICE_ACCOUNT` entry from the runtime env, clones the current branch into `/workspace/src/...`, and reuses persistent `cargo`, `rustup`, `target`, and `sccache` directories under `/workspace/cache/...`.

## Keep cache-aware workflows aligned

When containerized performance regresses, keep these three surfaces aligned:

- `scripts/podman-rust.sh` for local containerized runs
- `scripts/k8s-rust.sh` for long control-plane jobs
- `scripts/install-cargo-llvm-cov.sh` for release-based `cargo-llvm-cov` bootstrap
- repo CI files such as `.github/workflows/rust-ci.yaml`, `.github/workflows/docker-image.yaml`, and `Dockerfile`

Do not reintroduce relative Podman mounts, `bash -lc` inside the Rust image, or host-worktree assumptions in k8s jobs. Those are known-bad in this environment.
