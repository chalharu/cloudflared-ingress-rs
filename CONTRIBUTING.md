# Contributing

This document is the source of truth for contribution rules.

## 1. Development Flow

1. Create a branch from `main`.
2. Implement changes and add/update tests.
3. Run the relevant validation before opening a PR.
4. Commit using Conventional Commits.

## 2. Branch Rules

- `main`: always releasable.
- Working branches: `feature/<topic>`, `fix/<topic>`, `chore/<topic>`.
- Direct push to `main` is not allowed.

## 3. Validation

- Rust changes: `cargo test`
- Rust formatting/lint-sensitive changes: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`
- `.github/hooks/**` changes: `node --test .github/hooks/postToolUse/main.test.mjs`
- `.github/scripts/**` changes: `node --test .github/scripts/*.test.mjs`
- Hosted CI runs SonarQube Cloud analysis from `.github/workflows/sonarqube-cloud.yaml`; keep `sonar-project.properties` in sync with the repository layout and with the Rust LCOV coverage path produced by `cargo llvm-cov`.

## 4. Commit Message Rules (Conventional Commits)

Format:

`<type>(<scope>): <subject>`

Examples:

- `feat(api): add user profile endpoint`
- `fix(parser): handle empty input`
- `docs(readme): clarify setup steps`
- `chore(ci): update workflow cache key`

Types:

- `feat`: new feature
- `fix`: bug fix
- `docs`: documentation only
- `refactor`: code change without behavior change
- `test`: tests
- `chore`: maintenance/configuration

## 5. Release Labels

- PRs targeting `main` must carry exactly one of `semver:major`, `semver:minor`, or `semver:patch`.
- After merge, GitHub Actions bumps `Cargo.toml`, `Cargo.lock`, and `helm/Chart.yaml`, then creates the matching `vX.Y.Z` tag.
- Docker publishes `latest` and `sha-*` tags from `main`, semantic version tags from release tags, and prunes older non-semver or untagged GHCR versions while retaining the newest configured set.
