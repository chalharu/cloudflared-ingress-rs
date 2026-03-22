# Developer guide

This guide is for contributors and maintainers of `cloudflared-ingress-rs`. If you want to install or operate the controller, start with `README.md`. Contribution policy and branch rules live in `CONTRIBUTING.md`.

## Documentation map

- `README.md`: end-user installation, configuration, and operations
- `DEVELOPERS.md`: repository layout, local workflow, validation, and release context
- `CONTRIBUTING.md`: contribution policy, branch naming, and commit rules

## Repository layout

- `src/main.rs`: CLI entrypoint, health server, and controller startup
- `src/cli.rs`: CLI flags and environment variable parsing
- `src/error.rs`: shared error types
- `src/controllers/ingress.rs`: `Ingress` and `IngressClass` reconciliation into a per-`IngressClass` `CloudflaredTunnel`
- `src/controllers/cloudflared.rs`: Cloudflare tunnel, DNS, Secret, and Deployment reconciliation
- `src/controllers/cloudflared/*.rs`: Cloudflare API access, config rendering, CRD types, and Kubernetes helpers
- `helm/`: published Helm chart
- `yaml/`: standalone CRD manifest and sample `IngressClass`

## Toolchain and prerequisites

- Rust `1.94` or newer
- `cargo`
- Access to a Kubernetes cluster if you want to exercise live reconciliation
- A Cloudflare account ID and API token if you want to run against real Cloudflare resources

## Local development workflow

### Render the CRD

```bash
cargo run -- create-yaml
```

### Run the controller locally

```bash
cargo run -- run \
  --cloudflare-token "$CLOUDFLARE_TOKEN" \
  --cloudflare-account-id "$CLOUDFLARE_ACCOUNT_ID"
```

When you run the binary directly without flags, clap also accepts `CLOUDFLARE_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`. In the published Helm deployment, the required credential environment variable names are still `ACCOUNT_ID` and `ACCOUNT_TOKEN`, because the chart maps those values into CLI arguments.

Optional controller behavior can be configured with flags or environment variables such as:

- `INGRESS_CLASS`
- `INGRESS_CONTROLLER`
- `CLOUDFLARE_TUNNEL_PREFIX`
- `CLOUDFLARE_TUNNEL_NAMESPACE`
- `DEPLOYMENT_REPLICAS`

The process also starts a health server on `0.0.0.0:8080`.

## Validation

Run the repository's standard validation commands before opening or updating a pull request:

```bash
cargo build
cargo test
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings
```

Documentation-only changes do not usually need the Rust validation suite, but still review the rendered diff carefully.

## Architecture notes

- The binary runs two reconcilers plus the health server in one process.
- `src/controllers/ingress.rs` turns matching `Ingress` resources into a per-`IngressClass` `CloudflaredTunnel` in the configured tunnel namespace.
- `src/controllers/cloudflared.rs` treats `CloudflaredTunnel` as the desired state for Cloudflare tunnels, DNS records, Secrets, and the managed `cloudflared` Deployment.
- The controller only reacts to `IngressClass` objects whose `spec.controller` matches the configured controller string.
- Rendered `cloudflared` configuration is stored in Kubernetes Secrets and mounted into the managed workload.

## Deployment assets

- The published Helm chart is `oci://ghcr.io/chalharu/charts/cloudflared-ingress`.
- `helm/` contains the installable chart templates and default values.
- `yaml/crd.yaml` is useful when you need the CRD manifest outside Helm.
- `yaml/ingressclass.yaml` is a sample manifest for the default controller string.

## Release and versioning notes

- `main` stays pull-request-only and should remain releasable.
- Checked-in versions on `main` may carry a `-dev` suffix; release tags are the source of truth for published versions.
- Released charts are published to GHCR as OCI artifacts and line up with the release version unless `image.tag` is overridden.
- PRs targeting `main` may be unlabeled during review; if no semver label is present at merge time, release automation defaults to `patch`.

## Contribution flow

1. Branch from `main` using `feature/<topic>`, `fix/<topic>`, or `chore/<topic>`.
2. Implement changes and add or update tests when behavior changes.
3. Run the relevant validation commands.
4. Commit using Conventional Commits.
5. Open or update a pull request against `main`.

`CONTRIBUTING.md` remains the source of truth for contribution policy.
