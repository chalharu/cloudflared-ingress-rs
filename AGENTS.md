# Agent Guidelines

## Core Principles

- **Do NOT maintain backward compatibility** unless explicitly requested. Break things boldly.
- Prefer explicit over clever, and delete dead code immediately.
- Ignore `.github/skills/**` unless a task explicitly targets it.

---

## Project Overview

**Project type:** Rust Kubernetes controller and HTTP service for Cloudflare Tunnel ingress
**Primary language:** Rust
**Key dependencies:** `actix-web`, `kube`, `cloudflare`, `clap`, `tokio`

---

## Commands

```bash
cargo build
cargo test
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings
node --test .github/hooks/postToolUse/main.test.mjs  # when editing .github/hooks/**
```

---

## Code Conventions

- Follow existing `src/controllers` patterns and keep error handling explicit.

---

## Architecture

- `src/main.rs`: CLI entrypoint, health server, and controller startup
- `src/controllers/ingress.rs`: watches `Ingress`/`IngressClass` and reconciles `CloudflaredTunnel`
- `src/controllers/cloudflared.rs`: manages Cloudflare tunnels and backing Kubernetes resources
- `helm/` and `yaml/`: deployment manifests and chart assets

---

## Maintenance Notes

- Keep this file lean and update commands or architecture notes as workflows change.
