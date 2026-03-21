//! Binary entrypoint for the Cloudflare Tunnel ingress controller.

mod cli;
mod controllers;
mod error;

use actix_web::{App, HttpRequest, HttpResponse, HttpServer, Responder, get, middleware, web};
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

fn configure_app(cfg: &mut web::ServiceConfig) {
    cfg.service(index).service(health);
}

#[allow(clippy::result_large_err)]
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
            let (ingress_result, cloudflared_result, server_result) = tokio::join!(
                controllers::ingress::run_controllers(args.clone()),
                controllers::cloudflared::run_controller(args.clone()),
                run_server()
            );
            combine_run_results(ingress_result, cloudflared_result, server_result)?;
        }
    }

    Ok(())
}

#[allow(clippy::result_large_err)]
fn combine_run_results(
    ingress_result: Result<()>,
    cloudflared_result: Result<()>,
    server_result: std::io::Result<()>,
) -> Result<()> {
    ingress_result?;
    cloudflared_result?;
    server_result?;
    Ok(())
}

async fn run_server() -> Result<(), std::io::Error> {
    // Start web server
    let server = HttpServer::new(move || {
        App::new()
            .wrap(middleware::Logger::default().exclude("/health"))
            .configure(configure_app)
    })
    .bind("0.0.0.0:8080")?
    .workers(2)
    .shutdown_timeout(5);

    server.run().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{App, http::StatusCode, test as actix_test};

    #[actix_web::test]
    async fn configure_app_registers_health_endpoint() {
        let app = actix_test::init_service(App::new().configure(configure_app)).await;
        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::get().uri("/health").to_request(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            actix_test::read_body(response).await.as_ref(),
            br#""healthy""#
        );
    }

    #[actix_web::test]
    async fn configure_app_registers_index_endpoint() {
        let app = actix_test::init_service(App::new().configure(configure_app)).await;
        let response =
            actix_test::call_service(&app, actix_test::TestRequest::get().uri("/").to_request())
                .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert!(actix_test::read_body(response).await.is_empty());
    }

    #[test]
    fn combine_run_results_returns_the_first_controller_error() {
        let result = combine_run_results(Err(Error::illegal_document()), Ok(()), Ok(()));

        assert!(matches!(result, Err(Error::IllegalDocument { .. })));
    }

    #[test]
    fn combine_run_results_returns_the_second_controller_error() {
        let result = combine_run_results(Ok(()), Err(Error::illegal_document()), Ok(()));

        assert!(matches!(result, Err(Error::IllegalDocument { .. })));
    }

    #[test]
    fn combine_run_results_returns_the_server_error() {
        let result =
            combine_run_results(Ok(()), Ok(()), Err(std::io::Error::other("server failure")));

        assert!(matches!(result, Err(Error::IoError { .. })));
    }

    #[test]
    fn combine_run_results_returns_ok_when_all_tasks_succeed() {
        let result = combine_run_results(Ok(()), Ok(()), Ok(()));

        assert!(result.is_ok());
    }
}
