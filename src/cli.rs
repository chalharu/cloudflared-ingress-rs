use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug, Clone)]
pub struct Cli {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(Debug, Subcommand, Clone)]
pub enum Commands {
    #[command(about = "Create crd yaml")]
    CreateYaml,
    #[command()]
    Run(ControllerArgs),
}

#[derive(Debug, Clone, Args)]
pub struct ControllerArgs {
    #[arg(long, env)]
    ingress_class: Option<String>,
    #[arg(long, env, default_value = "chalharu.top/cloudflared-ingress-controller")]
    ingress_controller: String,
    #[arg(long, env)]
    cloudflare_token: String,
    #[arg(long, env)]
    cloudflare_account_id: String,
    #[arg(long, env, default_value = "k8s-ingress-")]
    cloudflare_tunnel_prefix: String,
    #[arg(long, env, default_value = "cloudflared")]
    cloudflare_tunnel_namespace: String,
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
}

impl Cli {
    pub fn commands(&self) -> &Commands {
        &self.commands
    }
}
