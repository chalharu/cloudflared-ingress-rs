mod error;

use std::{sync::Arc, time::Duration};

use crate::error::{Error, Result};
use actix_web::{get, middleware, App, HttpRequest, HttpResponse, HttpServer, Responder};
use futures::StreamExt as _;
use k8s_openapi::api::networking::v1::Ingress;
use kube::{
    runtime::{controller::Action, watcher::Config, Controller},
    Api, Client, ResourceExt,
};
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt as _, util::SubscriberInitExt as _};

#[get("/health")]
async fn health(_: HttpRequest) -> impl Responder {
    HttpResponse::Ok().json("healthy")
}

#[get("/")]
async fn index(_req: HttpRequest) -> impl Responder {
    HttpResponse::Ok()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Initiatilize Kubernetes controller state
    let controller = run();

    // Start web server
    let server = HttpServer::new(move || {
        App::new()
            .wrap(middleware::Logger::default().exclude("/health"))
            .service(index)
            .service(health)
    })
    .bind("0.0.0.0:8080")?
    .shutdown_timeout(5);

    // Both runtimes implements graceful shutdown, so poll until both are done
    tokio::join!(controller, server.run()).1?;
    Ok(())
}

/// Initialize the controller and shared state (given the crd is installed)
pub async fn run() {
    let client = Client::try_default()
        .await
        .expect("failed to create kube Client");
    let ingress_api = Api::<Ingress>::all(client.clone());
    Controller::new(ingress_api, Config::default().any_semantic())
        .shutdown_on_signal()
        .run(reconcile, error_policy, Arc::new(Context { client }))
        .filter_map(|x| async move { std::result::Result::ok(x) })
        .for_each(|_| futures::future::ready(()))
        .await;
}

// Context for our reconciler
#[derive(Clone)]
struct Context {
    /// Kubernetes client
    pub client: Client,
}

async fn reconcile(ingress: Arc<Ingress>, _ctx: Arc<Context>) -> Result<Action> {
    let ns = ingress.namespace().unwrap(); // doc is namespace scoped
    info!("Reconciling Ingress \"{}\" in {}", ingress.name_any(), ns);
    Ok(Action::requeue(Duration::from_secs(60 * 60)))
}

fn error_policy(_ingress: Arc<Ingress>, error: &Error, _ctx: Arc<Context>) -> Action {
    warn!("reconcile failed: {:?}", error);
    Action::requeue(Duration::from_secs(5 * 60))
}
