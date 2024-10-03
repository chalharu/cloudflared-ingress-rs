mod cli;
mod controllers;
mod error;

use actix_web::{get, middleware, App, HttpRequest, HttpResponse, HttpServer, Responder};
use clap::Parser as _;
use cli::{Cli, Commands};
use kube::CustomResourceExt as _;
use tracing_subscriber::{layer::SubscriberExt as _, util::SubscriberInitExt as _};

pub use crate::error::{ControllerError as Error, Result};

#[get("/health")]
async fn health(_: HttpRequest) -> impl Responder {
    HttpResponse::Ok().json("healthy")
}

#[get("/")]
async fn index(_req: HttpRequest) -> impl Responder {
    HttpResponse::Ok()
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Cli::parse();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    match args.commands() {
        Commands::CreateYaml => {
            serde_yaml::to_writer(
                std::io::stdout(),
                &controllers::cloudflared::CloudflaredTunnel::crd(),
            )?;
        }
        Commands::Run(args) => {
            // Both runtimes implements graceful shutdown, so poll until both are done
            tokio::join!(
                controllers::ingress::run_controllers(args.clone()),
                controllers::cloudflared::run_controller(args.clone()),
                run_server()
            )
            .1?;
        }
    }

    Ok(())
}

async fn run_server() -> Result<(), std::io::Error> {
    // Start web server
    let server = HttpServer::new(move || {
        App::new()
            .wrap(middleware::Logger::default().exclude("/health"))
            .service(index)
            .service(health)
    })
    .bind("0.0.0.0:8080")?
    .workers(2)
    .shutdown_timeout(5);

    server.run().await
}
