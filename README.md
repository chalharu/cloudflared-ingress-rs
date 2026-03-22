# cloudflared-ingress-rs

`cloudflared-ingress-rs` is a Kubernetes controller that publishes matching `Ingress` resources through Cloudflare Tunnel. You keep using standard Kubernetes `Ingress` objects; the controller turns them into `CloudflaredTunnel` resources, provisions the Cloudflare tunnel and DNS records, and keeps the backing `cloudflared` workload in sync.

If you are developing on this repository, start with `DEVELOPERS.md`. Contribution policy lives in `CONTRIBUTING.md`.

## When to use it

Use this controller when you want to expose Kubernetes services through Cloudflare Tunnel without manually maintaining tunnel configuration or DNS records for each application.

## What it manages

- Watches `Ingress` and `IngressClass` resources
- Renders a `CloudflaredTunnel` custom resource from matching ingress rules
- Creates or updates the Cloudflare tunnel and DNS CNAME records
- Stores rendered tunnel configuration in Kubernetes Secrets
- Runs and updates a managed `cloudflared` Deployment

## How it works

1. Apply or reference an `IngressClass` whose `spec.controller` matches the controller string.
2. Create an `Ingress` that uses that class.
3. The controller renders or updates a `CloudflaredTunnel` resource for that `IngressClass` from the matching ingress rules.
4. A second reconciler provisions the Cloudflare tunnel, DNS records, Secrets, and managed `cloudflared` workload.

## Before you begin

- A Kubernetes cluster you can deploy controllers into
- A Cloudflare account with permission to manage Tunnels and DNS for the zone or zones you want to publish
- A Cloudflare account ID
- A Cloudflare API token for tunnel and DNS management
- Helm if you plan to install the published chart

## Quick start with Helm

Released charts are published at `oci://ghcr.io/chalharu/charts/cloudflared-ingress`.

### 1. Create a namespace and credentials secret

```bash
kubectl create namespace cloudflared-ingress-system

kubectl -n cloudflared-ingress-system create secret generic cloudflare-credentials \
  --from-literal=ACCOUNT_ID=<cloudflare-account-id> \
  --from-literal=ACCOUNT_TOKEN=<cloudflare-api-token>
```

For a chart install, the required credential environment variables are `ACCOUNT_ID` and `ACCOUNT_TOKEN`. The chart passes those values to the controller's `--cloudflare-account-id` and `--cloudflare-token` arguments. Optional settings such as `INGRESS_CLASS`, `INGRESS_CONTROLLER`, or `CLOUDFLARE_TUNNEL_NAMESPACE` can be provided through chart `env` or `envFrom` values.

By default, the controller writes `CloudflaredTunnel`, Secret, and managed `cloudflared` resources into the `cloudflared` namespace. The quick start below keeps those resources in the install namespace instead.

### 2. Create a values file

```yaml
envFrom:
  - secretRef:
      name: cloudflare-credentials
env:
  - name: CLOUDFLARE_TUNNEL_NAMESPACE
    value: cloudflared-ingress-system
```

### 3. Install the chart

```bash
helm upgrade --install cloudflared-ingress oci://ghcr.io/chalharu/charts/cloudflared-ingress \
  --version <version> \
  --namespace cloudflared-ingress-system \
  --create-namespace \
  -f values.yaml
```

Use a released chart version for production installs. Released charts default to the matching controller image version. Override `image.tag` only if you intentionally want a different published image alias such as `latest` or `X.Y`.

## Connect an `IngressClass`

The controller only reconciles `IngressClass` objects whose `spec.controller` matches the configured ingress controller string. The default controller string is `chalharu.top/cloudflared-ingress-controller`.

```yaml
apiVersion: networking.k8s.io/v1
kind: IngressClass
metadata:
  name: cloudflared
spec:
  controller: chalharu.top/cloudflared-ingress-controller
```

If you override `INGRESS_CONTROLLER`, update the `IngressClass` to match.

## Publish an application

Create a standard `Ingress` that points at the matching `IngressClass`:

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

After reconciliation, the controller updates the `CloudflaredTunnel` for that `IngressClass`, the Cloudflare tunnel and DNS record, and the backing `cloudflared` resources needed to serve the ingress.

## What the controller creates

Expect the following managed state:

- A `CloudflaredTunnel` custom resource named after the `IngressClass`, in the configured tunnel namespace, populated from matching `Ingress` rules
- Cloudflare tunnel state and DNS CNAMEs for the ingress hostnames
- A config Secret and tunnel Secret referenced from `CloudflaredTunnel.status`
- A managed `cloudflared` Deployment in the configured tunnel namespace, which defaults to `cloudflared`

You can inspect the generated custom resources with:

```bash
kubectl get cloudflaredtunnels -A
kubectl describe cloudflaredtunnel <name> -n <namespace>
```

## Configuration reference

### Running the binary directly

The table below is for invoking the controller binary yourself. In that mode, every CLI flag also has a clap-provided environment variable.

| Purpose | CLI flag | Environment variable | Default |
| --- | --- | --- | --- |
| Restrict reconciliation to one `IngressClass` name | `--ingress-class` | `INGRESS_CLASS` | unset |
| Match `IngressClass.spec.controller` | `--ingress-controller` | `INGRESS_CONTROLLER` | `chalharu.top/cloudflared-ingress-controller` |
| Cloudflare API token | `--cloudflare-token` | `CLOUDFLARE_TOKEN` | required |
| Cloudflare account ID | `--cloudflare-account-id` | `CLOUDFLARE_ACCOUNT_ID` | required |
| Prefix for generated tunnel names | `--cloudflare-tunnel-prefix` | `CLOUDFLARE_TUNNEL_PREFIX` | `k8s-ingress-` |
| Namespace for managed `cloudflared` resources | `--cloudflare-tunnel-namespace` | `CLOUDFLARE_TUNNEL_NAMESPACE` | `cloudflared` |
| Replica count for the managed `cloudflared` Deployment | `--deployment-replicas` | `DEPLOYMENT_REPLICAS` | `1` |

### Required credentials for the published Helm chart

For the published Helm chart and the quick start above, the required container environment variables are `ACCOUNT_ID` and `ACCOUNT_TOKEN`. These are the names end users should place in their Secret or `env` and `envFrom` configuration. The chart then forwards those values to the controller as CLI arguments.

If you leave `CLOUDFLARE_TUNNEL_NAMESPACE` at its default `cloudflared`, make sure that namespace exists before the controller reconciles resources.

## Health and operations

The controller process also starts an HTTP server on `0.0.0.0:8080` with:

- `GET /health` returning `"healthy"`
- `GET /` returning `200 OK`

These endpoints are suitable for basic liveness and readiness style checks around the controller process.

This repository also includes:

- `helm/` for the published installable chart
- `yaml/crd.yaml` for the standalone CRD manifest
- `yaml/ingressclass.yaml` for a sample `IngressClass` manifest

If you build the binary from source, `cargo run -- create-yaml` prints the current CRD YAML.

## Troubleshooting

- If an `Ingress` is not being picked up, confirm `spec.ingressClassName` points at an `IngressClass` whose `spec.controller` matches the configured `INGRESS_CONTROLLER`.
- If tunnel or DNS reconciliation fails, confirm the Cloudflare token can manage both Tunnels and DNS records for the target zone.
- If the managed `cloudflared` Deployment is missing or stale, inspect the related `CloudflaredTunnel` status and the referenced Secrets.
- If you install with Helm, verify the pod receives `ACCOUNT_ID` and `ACCOUNT_TOKEN` from your Secret via `env` or `envFrom`.

## More docs

- `DEVELOPERS.md` for architecture, local development, validation, and release workflow
- `CONTRIBUTING.md` for contribution policy and commit and branch rules
