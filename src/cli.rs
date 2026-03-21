//! Command-line parsing for the controller binary.

use clap::{Args, Parser, Subcommand};

/// Top-level CLI for the `cloudflared-ingress-rs` binary.
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct Cli {
    #[command(subcommand)]
    commands: Commands,
}

/// Supported subcommands for the binary.
#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum Commands {
    #[command(about = "Print the CloudflaredTunnel CRD as YAML")]
    CreateYaml,
    #[command(about = "Run the ingress and cloudflared controllers")]
    Run(ControllerArgs),
}

/// Controller runtime configuration.
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct ControllerArgs {
    #[arg(long, env)]
    ingress_class: Option<String>,
    #[arg(
        long,
        env,
        default_value = "chalharu.top/cloudflared-ingress-controller"
    )]
    ingress_controller: String,
    #[arg(long, env)]
    cloudflare_token: String,
    #[arg(long, env)]
    cloudflare_account_id: String,
    #[arg(long, env, default_value = "k8s-ingress-")]
    cloudflare_tunnel_prefix: String,
    #[arg(long, env, default_value = "cloudflared")]
    cloudflare_tunnel_namespace: String,
    #[arg(long, env, default_value = "1")]
    deployment_replicas: usize,
}

impl ControllerArgs {
    pub fn ingress_class(&self) -> Option<&String> {
        self.ingress_class.as_ref()
    }

    pub fn ingress_controller(&self) -> &str {
        &self.ingress_controller
    }

    pub fn cloudflare_token(&self) -> &str {
        &self.cloudflare_token
    }

    pub fn cloudflare_account_id(&self) -> &str {
        &self.cloudflare_account_id
    }

    pub fn cloudflare_tunnel_prefix(&self) -> &str {
        &self.cloudflare_tunnel_prefix
    }

    pub fn cloudflare_tunnel_namespace(&self) -> &str {
        &self.cloudflare_tunnel_namespace
    }

    pub fn deployment_replicas(&self) -> usize {
        self.deployment_replicas
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(
        ingress_class: Option<String>,
        ingress_controller: impl Into<String>,
    ) -> Self {
        Self {
            ingress_class,
            ingress_controller: ingress_controller.into(),
            cloudflare_token: "token".to_string(),
            cloudflare_account_id: "account".to_string(),
            cloudflare_tunnel_prefix: "prefix-".to_string(),
            cloudflare_tunnel_namespace: "cloudflared".to_string(),
            deployment_replicas: 1,
        }
    }
}

impl Cli {
    pub fn commands(&self) -> &Commands {
        &self.commands
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Cli {
        Cli::try_parse_from(arguments).expect("command should parse")
    }

    #[test]
    fn create_yaml_command_parses() {
        let cli = parse(&["cloudflared-ingress-rs", "create-yaml"]);

        assert_eq!(cli.commands(), &Commands::CreateYaml);
    }

    #[test]
    fn run_command_uses_defaults_for_optional_arguments() {
        let cli = parse(&[
            "cloudflared-ingress-rs",
            "run",
            "--cloudflare-token",
            "token",
            "--cloudflare-account-id",
            "account",
        ]);

        assert_eq!(
            cli,
            Cli {
                commands: Commands::Run(ControllerArgs {
                    ingress_class: None,
                    ingress_controller: "chalharu.top/cloudflared-ingress-controller".to_string(),
                    cloudflare_token: "token".to_string(),
                    cloudflare_account_id: "account".to_string(),
                    cloudflare_tunnel_prefix: "k8s-ingress-".to_string(),
                    cloudflare_tunnel_namespace: "cloudflared".to_string(),
                    deployment_replicas: 1,
                }),
            }
        );
    }

    #[test]
    fn run_command_parses_explicit_overrides() {
        let cli = parse(&[
            "cloudflared-ingress-rs",
            "run",
            "--ingress-class",
            "public",
            "--ingress-controller",
            "example.com/controller",
            "--cloudflare-token",
            "token",
            "--cloudflare-account-id",
            "account",
            "--cloudflare-tunnel-prefix",
            "prod-",
            "--cloudflare-tunnel-namespace",
            "edge",
            "--deployment-replicas",
            "3",
        ]);

        assert_eq!(
            cli.commands(),
            &Commands::Run(ControllerArgs {
                ingress_class: Some("public".to_string()),
                ingress_controller: "example.com/controller".to_string(),
                cloudflare_token: "token".to_string(),
                cloudflare_account_id: "account".to_string(),
                cloudflare_tunnel_prefix: "prod-".to_string(),
                cloudflare_tunnel_namespace: "edge".to_string(),
                deployment_replicas: 3,
            })
        );
    }

    #[test]
    fn accessors_return_expected_values() {
        let args = ControllerArgs::new_for_test(
            Some("public".to_string()),
            "chalharu.top/cloudflared-ingress-controller",
        );

        assert_eq!(args.ingress_class().map(String::as_str), Some("public"));
        assert_eq!(
            args.ingress_controller(),
            "chalharu.top/cloudflared-ingress-controller"
        );
        assert_eq!(args.cloudflare_token(), "token");
        assert_eq!(args.cloudflare_account_id(), "account");
        assert_eq!(args.cloudflare_tunnel_prefix(), "prefix-");
        assert_eq!(args.cloudflare_tunnel_namespace(), "cloudflared");
        assert_eq!(args.deployment_replicas(), 1);
    }
}
