use std::{future::Future, time::Duration};

use k8s_openapi::{
    api::core::v1::Namespace,
    apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition,
};
use kube::{
    Api, Client, CustomResourceExt as _,
    api::{DeleteParams, ObjectMeta, Patch, PatchParams},
};
use uuid::Uuid;

use super::cloudflared::CloudflaredTunnel;

const APPLY_MANAGER: &str = "cloudflared-ingress-kind-tests";

pub(crate) fn kind_tests_enabled() -> bool {
    std::env::var_os("RUN_KIND_TESTS").is_some()
}

pub(crate) async fn kind_client() -> Option<Client> {
    if !kind_tests_enabled() {
        return None;
    }

    Some(
        Client::try_default()
            .await
            .expect("kind-backed tests require a working kubeconfig"),
    )
}

pub(crate) fn unique_name(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4().simple())
}

pub(crate) async fn ensure_namespace(client: &Client, name: &str) {
    let api = Api::<Namespace>::all(client.clone());
    api.patch(
        name,
        &PatchParams::apply(APPLY_MANAGER).force(),
        &Patch::Apply(Namespace {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
    )
    .await
    .expect("namespace should apply");
}

pub(crate) async fn cleanup_namespace(client: &Client, name: &str) {
    let api = Api::<Namespace>::all(client.clone());
    match api.delete(name, &DeleteParams::background()).await {
        Ok(_) => {}
        Err(kube::Error::Api(error)) if error.code == 404 => return,
        Err(error) => panic!("failed to delete namespace {name}: {error}"),
    }

    wait_for(
        format!("namespace deletion for {name}"),
        Duration::from_secs(30),
        || async {
            api.get_opt(name)
                .await
                .expect("namespace lookup should succeed")
                .is_none()
                .then_some(())
        },
    )
    .await;
}

pub(crate) async fn ensure_cloudflared_crd(client: &Client) {
    let api = Api::<CustomResourceDefinition>::all(client.clone());
    let crd = CloudflaredTunnel::crd();
    let name = crd
        .metadata
        .name
        .clone()
        .expect("generated CRD should have a name");

    api.patch(
        &name,
        &PatchParams::apply(APPLY_MANAGER).force(),
        &Patch::Apply(crd),
    )
    .await
    .expect("CloudflaredTunnel CRD should apply");

    wait_for(
        "CloudflaredTunnel CRD establishment",
        Duration::from_secs(30),
        || async {
            api.get(&name)
                .await
                .expect("CRD lookup should succeed")
                .status
                .as_ref()
                .and_then(|status| status.conditions.as_ref())
                .and_then(|conditions| {
                    conditions
                        .iter()
                        .find(|condition| condition.type_ == "Established")
                })
                .is_some_and(|condition| condition.status == "True")
                .then_some(())
        },
    )
    .await;
}

pub(crate) async fn wait_for<T, F, Fut>(
    description: impl AsRef<str>,
    timeout: Duration,
    mut check: F,
) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let start = tokio::time::Instant::now();
    loop {
        if let Some(value) = check().await {
            return value;
        }

        if start.elapsed() >= timeout {
            panic!("timed out waiting for {}", description.as_ref());
        }

        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
