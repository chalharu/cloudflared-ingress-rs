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

fn env_filter_from_default_env() -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())
}

#[allow(clippy::result_large_err)]
fn write_crd_yaml<W: std::io::Write>(writer: W) -> Result<()> {
    serde_yaml::to_writer(writer, &controllers::cloudflared::CloudflaredTunnel::crd())?;
    Ok(())
}

#[allow(clippy::result_large_err)]
async fn run_components<I, C, S>(ingress: I, cloudflared: C, server: S) -> Result<()>
where
    I: std::future::Future<Output = Result<()>>,
    C: std::future::Future<Output = Result<()>>,
    S: std::future::Future<Output = std::io::Result<()>>,
{
    let (ingress_result, cloudflared_result, server_result) =
        tokio::join!(ingress, cloudflared, server);
    combine_run_results(ingress_result, cloudflared_result, server_result)
}

#[allow(clippy::result_large_err)]
async fn execute_command_with<W, IF, CF, SF, I, C, S>(
    command: &Commands,
    writer: &mut W,
    ingress_runner: IF,
    cloudflared_runner: CF,
    server_runner: SF,
) -> Result<()>
where
    W: std::io::Write,
    IF: Fn(cli::ControllerArgs) -> I,
    CF: Fn(cli::ControllerArgs) -> C,
    SF: Fn() -> S,
    I: std::future::Future<Output = Result<()>>,
    C: std::future::Future<Output = Result<()>>,
    S: std::future::Future<Output = std::io::Result<()>>,
{
    match command {
        Commands::CreateYaml => write_crd_yaml(writer),
        Commands::Run(args) => {
            run_components(
                ingress_runner(args.clone()),
                cloudflared_runner(args.clone()),
                server_runner(),
            )
            .await
        }
    }
}

#[allow(clippy::result_large_err)]
#[tokio::main]
async fn main() -> Result<()> {
    let args = Cli::parse();

    tracing_subscriber::registry()
        .with(env_filter_from_default_env())
        .with(tracing_subscriber::fmt::layer())
        .init();

    let mut stdout = std::io::stdout();
    execute_command_with(
        args.commands(),
        &mut stdout,
        controllers::ingress::run_controllers,
        controllers::cloudflared::run_controller,
        run_server,
    )
    .await?;

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

fn build_server(bind_address: &str) -> Result<actix_web::dev::Server, std::io::Error> {
    let server = HttpServer::new(move || {
        App::new()
            .wrap(middleware::Logger::default().exclude("/health"))
            .configure(configure_app)
    })
    .bind(bind_address)?
    .workers(2)
    .shutdown_timeout(5);

    Ok(server.run())
}

async fn run_server() -> Result<(), std::io::Error> {
    build_server("0.0.0.0:8080")?.await
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

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

    #[test]
    fn env_filter_from_default_env_returns_a_filter() {
        assert!(!env_filter_from_default_env().to_string().is_empty());
    }

    #[test]
    fn write_crd_yaml_renders_cloudflared_tunnel_crd() {
        let mut output = Vec::new();

        write_crd_yaml(&mut output).expect("CRD YAML should render");

        let rendered = String::from_utf8(output).expect("rendered YAML should be UTF-8");
        assert!(rendered.contains("CustomResourceDefinition"));
        assert!(rendered.contains("cloudflaredtunnels.chalharu.top"));
    }

    #[actix_web::test]
    async fn execute_command_with_runs_controller_branch() {
        let args =
            cli::ControllerArgs::new_for_test(None, "chalharu.top/cloudflared-ingress-controller");
        let ingress_calls = Arc::new(AtomicUsize::new(0));
        let cloudflared_calls = Arc::new(AtomicUsize::new(0));
        let server_calls = Arc::new(AtomicUsize::new(0));
        let mut output = Vec::new();

        let result = execute_command_with(
            &Commands::Run(args.clone()),
            &mut output,
            {
                let ingress_calls = ingress_calls.clone();
                let args = args.clone();
                move |actual| {
                    let ingress_calls = ingress_calls.clone();
                    let args = args.clone();
                    async move {
                        assert_eq!(actual, args);
                        ingress_calls.fetch_add(1, Ordering::Relaxed);
                        Ok(())
                    }
                }
            },
            {
                let cloudflared_calls = cloudflared_calls.clone();
                let args = args.clone();
                move |actual| {
                    let cloudflared_calls = cloudflared_calls.clone();
                    let args = args.clone();
                    async move {
                        assert_eq!(actual, args);
                        cloudflared_calls.fetch_add(1, Ordering::Relaxed);
                        Ok(())
                    }
                }
            },
            {
                let server_calls = server_calls.clone();
                move || {
                    let server_calls = server_calls.clone();
                    async move {
                        server_calls.fetch_add(1, Ordering::Relaxed);
                        Ok(())
                    }
                }
            },
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(ingress_calls.load(Ordering::Relaxed), 1);
        assert_eq!(cloudflared_calls.load(Ordering::Relaxed), 1);
        assert_eq!(server_calls.load(Ordering::Relaxed), 1);
        assert!(output.is_empty());
    }

    #[actix_web::test]
    async fn build_server_starts_and_stops_on_an_ephemeral_port() {
        let server = build_server("127.0.0.1:0").expect("server should bind to an ephemeral port");
        let handle = server.handle();
        let task = actix_web::rt::spawn(server);

        handle.stop(true).await;
        task.await
            .expect("server task should join")
            .expect("server should stop cleanly");
    }
}
