# cloudflared-ingress-rs

`cloudflared-ingress-rs` is a Rust-based Kubernetes controller and health server that turns Kubernetes `Ingress` objects into Cloudflare Tunnel configuration.

It watches `Ingress` and `IngressClass` resources, renders a `CloudflaredTunnel` custom resource, provisions Cloudflare tunnels and DNS records, and keeps the backing `cloudflared` workload in sync.

## What this repository contains

- A CLI binary for running the controllers or emitting the CRD YAML
- A small Actix Web health server exposed on port `8080`
- Kubernetes reconcilers for `Ingress` and `CloudflaredTunnel`
- Helm and raw YAML assets for deployment

## How it works

1. `src/controllers/ingress.rs` watches `Ingress` and `IngressClass`.
2. Matching ingress rules are converted into a `CloudflaredTunnel` custom resource.
3. `src/controllers/cloudflared.rs` reconciles that CRD with Cloudflare tunnels, DNS CNAME records, Kubernetes Secrets, and a Deployment running `cloudflared`.
4. `src/main.rs` runs both controllers together with the health server.

## Repository layout

- `src/main.rs`: CLI entrypoint, health routes, controller startup
- `src/cli.rs`: command-line and environment parsing
- `src/error.rs`: shared error types
- `src/controllers/ingress.rs`: `Ingress` -> `CloudflaredTunnel` reconciliation
- `src/controllers/cloudflared.rs`: Cloudflare and Kubernetes reconciliation for the CRD
- `src/controllers/cloudflared/*.rs`: Cloudflare API, config rendering, CRD definitions, and Kubernetes helper modules
- `helm/`: Helm chart assets
- `yaml/`: raw manifest assets

## Requirements

- Rust `1.94` or newer
- Access to a Kubernetes cluster
- A Cloudflare account with tunnel and DNS permissions
- A Cloudflare API token and account ID

### Cloudflare permissions

The controller needs a token that can manage Cloudflare tunnels and DNS records for the zones you want to expose.

### Kubernetes permissions

The controller needs RBAC that allows it to watch `Ingress`, `IngressClass`, `Service`, and `CloudflaredTunnel` resources and to manage Secrets and Deployments in the target namespace. The deployment assets under `helm/` and `yaml/` are the intended place to provide those permissions.

## Getting started

1. Prepare a Kubernetes cluster and install the CRD:

   ```bash
   cargo run -- create-yaml | kubectl apply -f -
   ```

2. Configure an `IngressClass` whose `spec.controller` matches the controller string you run this binary with.

3. Start the controller with a Cloudflare token and account ID.

4. Apply an `Ingress` that references the matching `IngressClass`.

Example `IngressClass`:

```yaml
apiVersion: networking.k8s.io/v1
kind: IngressClass
metadata:
  name: cloudflared
spec:
  controller: chalharu.top/cloudflared-ingress-controller
```

Example `Ingress`:

```yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: example
  namespace: default
spec:
  ingressClassName: cloudflared
  rules:
    - host: app.example.com
      http:
        paths:
          - path: /
            pathType: Prefix
            backend:
              service:
                name: app
                port:
                  number: 80
```

## CLI usage

Generate the CRD YAML:

```bash
cargo run -- create-yaml
```

Run the controllers locally:

```bash
cargo run -- run \
  --cloudflare-token "$CLOUDFLARE_TOKEN" \
  --cloudflare-account-id "$CLOUDFLARE_ACCOUNT_ID"
```

The process also starts an HTTP server on `0.0.0.0:8080` with:

- `GET /health` -> `"healthy"`
- `GET /` -> `200 OK`

The health server is intended for lightweight liveness/readiness style checks around the controller process.

## Configuration

Every CLI option can also be supplied via environment variables because the project uses `clap`'s `env` support.

| CLI flag | Environment variable | Default |
| --- | --- | --- |
| `--ingress-class` | `INGRESS_CLASS` | unset |
| `--ingress-controller` | `INGRESS_CONTROLLER` | `chalharu.top/cloudflared-ingress-controller` |
| `--cloudflare-token` | `CLOUDFLARE_TOKEN` | required |
| `--cloudflare-account-id` | `CLOUDFLARE_ACCOUNT_ID` | required |
| `--cloudflare-tunnel-prefix` | `CLOUDFLARE_TUNNEL_PREFIX` | `k8s-ingress-` |
| `--cloudflare-tunnel-namespace` | `CLOUDFLARE_TUNNEL_NAMESPACE` | `cloudflared` |
| `--deployment-replicas` | `DEPLOYMENT_REPLICAS` | `1` |

## Development

Common validation commands:

```bash
cargo test --all-targets
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build
```

This repository also includes containerized Rust helpers for environments where local toolchains are inconvenient:

```bash
bash .github/skills/containerized-rust-ops/scripts/podman-rust.sh test
bash .github/skills/containerized-rust-ops/scripts/podman-rust.sh clippy
```

### Release automation

- PRs targeting `main` can stay unlabeled during review. If more than one semver label is present, the guard fails. If no semver label is present at merge time, the release workflow defaults to `patch`.
- The merge-to-`main` release workflow derives the current release from the latest `vX.Y.Z` tag when one exists, then creates an isolated release-only commit with updated `Cargo.toml`, `Cargo.lock`, and `helm/Chart.yaml` and pushes only the new `vX.Y.Z` tag.
- Release tags are the source of truth for published versions. Because `main` remains pull-request-only, the checked-in version metadata on `main` may lag behind the latest release tag and may intentionally use a `-dev` suffix as long as the repository still builds correctly.
- Docker publishes `latest` and `sha-*` tags from `main`, semantic version tags from release tags, and prunes older non-semver or untagged GHCR versions while retaining the newest configured set.

GitHub Actions also runs SonarQube Cloud analysis via `.github/workflows/sonarqube-cloud.yaml`. That workflow targets the checked-in `chalharu_cloudflared-ingress-rs` project, generates Rust coverage with `cargo llvm-cov`, imports `target/llvm-cov/lcov.info`, and expects the `SONAR_TOKEN` repository secret to remain configured.

Contribution conventions are documented in `CONTRIBUTING.md`.

## Deployment assets

- `helm/` contains chart assets for chart-driven installs
- `yaml/` contains raw manifests for environments that prefer plain Kubernetes YAML

## Troubleshooting

- If an `Ingress` is not being picked up, check that its `ingressClassName` points at an `IngressClass` whose `spec.controller` matches the configured `--ingress-controller` value.
- If DNS records are not being created, confirm the Cloudflare token has permission to manage tunnels and DNS for the target zone.
- If the managed `cloudflared` Deployment does not update, inspect the generated `CloudflaredTunnel` resource and the Secrets referenced from its status.

## Notes

- The controller only reconciles `IngressClass` objects whose controller string matches the configured ingress controller.
- Rendered Cloudflared configuration is stored in Kubernetes Secrets and mounted into the managed Deployment.
- The default Cloudflared image is defined in `src/controllers/cloudflared.rs`.
