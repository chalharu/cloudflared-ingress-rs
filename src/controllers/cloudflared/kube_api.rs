//! Kubernetes resource helpers used by the Cloudflared controller.

use std::{collections::BTreeMap, time::Duration};

use k8s_openapi::{
    ByteString,
    api::{
        apps::v1::{Deployment, DeploymentSpec},
        core::v1::{
            Container, PodSpec, PodTemplateSpec, Secret, SecretVolumeSource, Volume, VolumeMount,
        },
    },
    apimachinery::pkg::apis::meta::v1::{LabelSelector, OwnerReference},
};
use kube::{
    Api, Client,
    api::{DeleteParams, ListParams, ObjectMeta, Patch, PatchParams, PostParams},
};

use super::{
    CFD_DEPLOYMENT_IMAGE, CloudflaredTunnel, PATCH_PARAMS_APPLY_NAME,
    customresource::{CloudflaredTunnelSpec, CloudflaredTunnelStatus},
};
use crate::Result;

const SELECTOR_ID_LABEL: &str = "cloudflared-ingress.chalharu.top/selector-id";

fn secret_string_data(data: BTreeMap<String, String>) -> BTreeMap<String, ByteString> {
    data.into_iter()
        .map(|(key, value)| (key, ByteString(value.into_bytes())))
        .collect()
}

fn opaque_secret(
    name: &str,
    data: BTreeMap<String, ByteString>,
    owner_ref: Option<Vec<OwnerReference>>,
) -> Secret {
    Secret {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            owner_references: owner_ref,
            ..Default::default()
        },
        data: Some(data),
        type_: Some("Opaque".to_string()),
        ..Default::default()
    }
}

fn default_container_args(tunnel_id: &str) -> Vec<String> {
    vec![
        "tunnel".to_string(),
        "--no-autoupdate".to_string(),
        "--config".to_string(),
        "/etc/cloudflared/config.yml".to_string(),
        "run".to_string(),
        tunnel_id.to_string(),
    ]
}

fn deployment_labels(selector_id: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("app".to_string(), "cloudflared".to_string()),
        (SELECTOR_ID_LABEL.to_string(), selector_id.to_string()),
    ])
}

fn deployment_changed(before: Option<&Deployment>, after: &Deployment) -> bool {
    before.is_none_or(|current| current.metadata.generation != after.metadata.generation)
}

fn deployment_selector_changed(current: &Deployment, desired: &Deployment) -> bool {
    current
        .spec
        .as_ref()
        .map(|spec| spec.selector.match_labels.as_ref())
        != desired
            .spec
            .as_ref()
            .map(|spec| spec.selector.match_labels.as_ref())
}

fn secret_changed(before: Option<&Secret>, after: &Secret) -> bool {
    before
        .is_none_or(|current| current.metadata.resource_version != after.metadata.resource_version)
}

fn cloudflared_deployment(
    name: &str,
    namespace: &str,
    tunnel_config_secret_name: &str,
    tunnel_id: &str,
    replicas: i32,
    cfdt: &CloudflaredTunnelSpec,
    owner_ref: Option<Vec<OwnerReference>>,
) -> Deployment {
    let selector_id = owner_ref
        .as_ref()
        .and_then(|owner_refs| owner_refs.first())
        .map(|owner_ref| owner_ref.uid.clone())
        .unwrap_or_else(|| name.to_string());

    Deployment {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(namespace.to_string()),
            owner_references: owner_ref,
            ..Default::default()
        },
        spec: Some(DeploymentSpec {
            replicas: Some(replicas),
            selector: LabelSelector {
                match_labels: Some(deployment_labels(&selector_id)),
                ..Default::default()
            },
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(deployment_labels(&selector_id)),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    containers: vec![Container {
                        command: cfdt.command.clone(),
                        args: cfdt
                            .args
                            .clone()
                            .or_else(|| Some(default_container_args(tunnel_id))),
                        image: cfdt
                            .image
                            .clone()
                            .or_else(|| Some(CFD_DEPLOYMENT_IMAGE.to_string())),
                        name: name.to_string(),
                        volume_mounts: Some(vec![VolumeMount {
                            mount_path: "/etc/cloudflared".to_string(),
                            name: "tunnel-config".to_string(),
                            read_only: Some(true),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    }],
                    volumes: Some(vec![Volume {
                        name: "tunnel-config".to_string(),
                        secret: Some(SecretVolumeSource {
                            default_mode: Some(0o644),
                            optional: Some(false),
                            secret_name: Some(tunnel_config_secret_name.to_string()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }]),
                    ..Default::default()
                }),
            },
            ..Default::default()
        }),
        ..Default::default()
    }
}

pub(super) async fn patch_cloudflaredtunnel_status<F: FnOnce(&mut CloudflaredTunnelStatus)>(
    client: &Client,
    namespace: &str,
    name: &str,
    update_fn: F,
) -> Result<CloudflaredTunnel> {
    let api = Api::<CloudflaredTunnel>::namespaced(client.clone(), namespace);
    let current_status = api.get_status(name).await?;
    let mut new_status = current_status.status.as_ref().cloned().unwrap_or_default();
    update_fn(&mut new_status);
    if current_status
        .status
        .as_ref()
        .is_some_and(|current_status| new_status == *current_status)
    {
        return Ok(current_status);
    }

    let results = api
        .patch_status(
            name,
            &PatchParams::apply(PATCH_PARAMS_APPLY_NAME).force(),
            &Patch::Apply(CloudflaredTunnel {
                metadata: ObjectMeta::default(),
                spec: CloudflaredTunnelSpec::default(),
                status: Some(new_status),
            }),
        )
        .await?;

    Ok(results)
}

pub(super) async fn patch_opaque_secret_string(
    client: &Client,
    name: &str,
    namespace: &str,
    data: BTreeMap<String, String>,
    owner_ref: Option<Vec<OwnerReference>>,
) -> Result<bool> {
    patch_opaque_secret(client, name, namespace, secret_string_data(data), owner_ref).await
}

pub(super) async fn patch_opaque_secret(
    client: &Client,
    name: &str,
    namespace: &str,
    data: BTreeMap<String, ByteString>,
    owner_ref: Option<Vec<OwnerReference>>,
) -> Result<bool> {
    let api = Api::<Secret>::namespaced(client.clone(), namespace);
    let secret = opaque_secret(name, data, owner_ref);

    let before = api.get_opt(name).await?;

    let patched = api
        .patch(
            name,
            &PatchParams::apply(PATCH_PARAMS_APPLY_NAME).force(),
            &Patch::Apply(secret),
        )
        .await?;

    Ok(secret_changed(before.as_ref(), &patched))
}

pub(super) async fn restart_deployment(
    client: &Client,
    name: &str,
    namespace: &str,
) -> Result<Deployment> {
    let api = Api::<Deployment>::namespaced(client.clone(), namespace);
    Ok(api.restart(name).await?)
}

pub(super) async fn get_cloudflaredtunnel(client: &Client) -> Result<Vec<CloudflaredTunnel>> {
    let api = Api::<CloudflaredTunnel>::all(client.clone());
    let results = api.list(&ListParams::default()).await?.items;
    Ok(results)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn patch_deployment(
    client: &Client,
    name: &str,
    namespace: &str,
    tunnel_config_secret_name: &str,
    tunnel_id: &str,
    replicas: i32,
    cfdt: &CloudflaredTunnelSpec,
    owner_ref: Option<Vec<OwnerReference>>,
) -> Result<bool> {
    let api = Api::<Deployment>::namespaced(client.clone(), namespace);
    let deployment = cloudflared_deployment(
        name,
        namespace,
        tunnel_config_secret_name,
        tunnel_id,
        replicas,
        cfdt,
        owner_ref,
    );

    let before = api.get_opt(name).await?;
    if before
        .as_ref()
        .is_some_and(|current| deployment_selector_changed(current, &deployment))
    {
        api.delete(name, &DeleteParams::background()).await?;

        let mut current = before;
        for _ in 0..30 {
            if current.is_none() {
                break;
            }

            tokio::time::sleep(Duration::from_millis(200)).await;
            current = api.get_opt(name).await?;
        }

        if current.is_some() {
            return Err(std::io::Error::other(
                "deployment selector migration is still in progress",
            )
            .into());
        }

        // Server-side apply can still observe a transient 404 after deleting an
        // immutable-selector Deployment, so recreate it via the collection API.
        api.create(&PostParams::default(), &deployment).await?;
        return Ok(true);
    }

    let patched = api
        .patch(
            name,
            &PatchParams::apply(PATCH_PARAMS_APPLY_NAME).force(),
            &Patch::Apply(deployment),
        )
        .await?;

    Ok(deployment_changed(before.as_ref(), &patched))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controllers::test_support;
    use kube::api::PostParams;

    fn owner_reference() -> OwnerReference {
        OwnerReference {
            api_version: "chalharu.top/v1alpha1".to_string(),
            kind: "CloudflaredTunnel".to_string(),
            name: "demo".to_string(),
            uid: "uid-1".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn secret_string_data_converts_values_to_bytes() {
        let data = secret_string_data(BTreeMap::from([
            ("config.yml".to_string(), "value".to_string()),
            ("cred.json".to_string(), "{}".to_string()),
        ]));

        assert_eq!(data["config.yml"].0, b"value");
        assert_eq!(data["cred.json"].0, b"{}");
    }

    #[test]
    fn opaque_secret_builder_sets_expected_metadata() {
        let secret = opaque_secret(
            "config-secret",
            BTreeMap::from([("config.yml".to_string(), ByteString(b"value".to_vec()))]),
            Some(vec![owner_reference()]),
        );

        assert_eq!(secret.metadata.name.as_deref(), Some("config-secret"));
        assert_eq!(secret.type_.as_deref(), Some("Opaque"));
        assert_eq!(
            secret
                .metadata
                .owner_references
                .as_ref()
                .expect("owner reference should exist")
                .len(),
            1
        );
        assert_eq!(
            secret.data.as_ref().expect("secret data should exist")["config.yml"].0,
            b"value"
        );
    }

    #[test]
    fn default_container_args_include_the_tunnel_id() {
        assert_eq!(
            default_container_args("tunnel-id"),
            vec![
                "tunnel",
                "--no-autoupdate",
                "--config",
                "/etc/cloudflared/config.yml",
                "run",
                "tunnel-id",
            ]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
        );
    }

    #[test]
    fn deployment_labels_include_a_stable_selector_identifier() {
        assert_eq!(
            deployment_labels("selector-id"),
            BTreeMap::from([
                ("app".to_string(), "cloudflared".to_string()),
                (SELECTOR_ID_LABEL.to_string(), "selector-id".to_string()),
            ])
        );
    }

    #[test]
    fn cloudflared_deployment_builder_uses_defaults_when_spec_omits_overrides() {
        let deployment = cloudflared_deployment(
            "demo-cloudflared",
            "default",
            "config-secret",
            "tunnel-id",
            2,
            &CloudflaredTunnelSpec::default(),
            Some(vec![owner_reference()]),
        );

        let spec = deployment.spec.expect("deployment spec should exist");
        let pod_spec = spec.template.spec.expect("pod spec should exist");
        let pod_labels = spec
            .template
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.labels.as_ref())
            .expect("pod labels should exist");
        let container = &pod_spec.containers[0];
        let volume = &pod_spec.volumes.expect("volumes should exist")[0];

        assert_eq!(
            deployment.metadata.name.as_deref(),
            Some("demo-cloudflared")
        );
        assert_eq!(deployment.metadata.namespace.as_deref(), Some("default"));
        assert_eq!(spec.replicas, Some(2));
        assert_eq!(
            spec.selector.match_labels.as_ref(),
            Some(&deployment_labels("uid-1"))
        );
        assert_eq!(
            pod_labels.get(SELECTOR_ID_LABEL).map(String::as_str),
            Some("uid-1")
        );
        assert_eq!(container.image.as_deref(), Some(CFD_DEPLOYMENT_IMAGE));
        assert_eq!(
            container.args.as_deref(),
            Some(default_container_args("tunnel-id").as_slice())
        );
        assert_eq!(volume.name, "tunnel-config");
        assert_eq!(
            volume
                .secret
                .as_ref()
                .and_then(|secret| secret.secret_name.as_deref()),
            Some("config-secret")
        );
    }

    #[test]
    fn cloudflared_deployment_builder_respects_spec_overrides() {
        let deployment = cloudflared_deployment(
            "demo-cloudflared",
            "default",
            "config-secret",
            "tunnel-id",
            1,
            &CloudflaredTunnelSpec {
                image: Some("cloudflare/cloudflared:custom".to_string()),
                command: Some(vec!["cloudflared".to_string()]),
                args: Some(vec!["tunnel".to_string(), "ingress".to_string()]),
                ..Default::default()
            },
            None,
        );

        let container = &deployment
            .spec
            .expect("deployment spec should exist")
            .template
            .spec
            .expect("pod spec should exist")
            .containers[0];

        assert_eq!(
            container.image.as_deref(),
            Some("cloudflare/cloudflared:custom")
        );
        assert_eq!(
            container.command.as_deref(),
            Some(&["cloudflared".to_string()][..])
        );
        assert_eq!(
            container.args.as_deref(),
            Some(&["tunnel".to_string(), "ingress".to_string()][..])
        );
    }

    #[test]
    fn change_detection_helpers_compare_resource_versions_and_generations() {
        let before_secret = Secret {
            metadata: ObjectMeta {
                resource_version: Some("1".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let after_secret = Secret {
            metadata: ObjectMeta {
                resource_version: Some("2".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let before_deployment = Deployment {
            metadata: ObjectMeta {
                generation: Some(1),
                ..Default::default()
            },
            spec: Some(DeploymentSpec {
                selector: LabelSelector {
                    match_labels: Some(BTreeMap::from([(
                        "app".to_string(),
                        "cloudflared".to_string(),
                    )])),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        let after_deployment = Deployment {
            metadata: ObjectMeta {
                generation: Some(2),
                ..Default::default()
            },
            spec: Some(DeploymentSpec {
                selector: LabelSelector {
                    match_labels: Some(deployment_labels("uid-1")),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        assert!(secret_changed(None, &after_secret));
        assert!(!secret_changed(Some(&before_secret), &before_secret));
        assert!(secret_changed(Some(&before_secret), &after_secret));
        assert!(deployment_changed(None, &after_deployment));
        assert!(!deployment_changed(
            Some(&before_deployment),
            &Deployment {
                metadata: ObjectMeta {
                    generation: Some(1),
                    ..Default::default()
                },
                spec: Some(DeploymentSpec {
                    selector: LabelSelector {
                        match_labels: Some(BTreeMap::from([(
                            "app".to_string(),
                            "cloudflared".to_string(),
                        )])),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                ..Default::default()
            }
        ));
        assert!(deployment_changed(
            Some(&before_deployment),
            &after_deployment
        ));
        assert!(deployment_selector_changed(
            &before_deployment,
            &after_deployment
        ));
    }

    fn test_cloudflared_tunnel(namespace: &str, name: &str) -> CloudflaredTunnel {
        CloudflaredTunnel {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(namespace.to_string()),
                ..Default::default()
            },
            spec: CloudflaredTunnelSpec {
                default_ingress_service: "http_status:404".to_string(),
                ..Default::default()
            },
            status: None,
        }
    }

    #[tokio::test]
    async fn kind_patch_helpers_manage_live_kubernetes_resources() {
        let Some(client) = test_support::kind_client().await else {
            return;
        };

        test_support::ensure_cloudflared_crd(&client).await;
        let namespace = test_support::unique_name("kube-api");
        test_support::ensure_namespace(&client, &namespace).await;
        test_support::with_cleanup(
            {
                let client = client.clone();
                let namespace = namespace.clone();
                move || async move {
                    let cfdt_api = Api::<CloudflaredTunnel>::namespaced(client.clone(), &namespace);
                    let owner = cfdt_api
                        .create(
                            &PostParams::default(),
                            &test_cloudflared_tunnel(&namespace, "demo"),
                        )
                        .await
                        .expect("CloudflaredTunnel should create");
                    let migrated_owner = cfdt_api
                        .create(
                            &PostParams::default(),
                            &test_cloudflared_tunnel(&namespace, "demo-migrated"),
                        )
                        .await
                        .expect("migrated CloudflaredTunnel should create");

                    // Use live owner UIDs so Kubernetes GC does not treat dependents as dangling.
                    let owner_ref = super::super::cloudflared_owner_reference(&owner)
                        .expect("owner reference should build");
                    let migrated_owner_ref =
                        super::super::cloudflared_owner_reference(&migrated_owner)
                            .expect("migrated owner reference should build");
                    assert_ne!(owner_ref.uid, migrated_owner_ref.uid);

                    let resources = get_cloudflaredtunnel(&client)
                        .await
                        .expect("resource list should succeed");
                    assert!(resources.iter().any(|resource| {
                        resource.metadata.namespace.as_deref() == Some(namespace.as_str())
                            && resource.metadata.name.as_deref() == Some("demo")
                    }));

                    let updated_status =
                        patch_cloudflaredtunnel_status(&client, &namespace, "demo", |status| {
                            status.tunnel_id = Some("tunnel-id".to_string());
                            status.config_secret_ref = Some("config-secret".to_string());
                        })
                        .await
                        .expect("status patch should succeed");
                    assert_eq!(
                        updated_status
                            .status
                            .as_ref()
                            .and_then(|status| status.tunnel_id.as_deref()),
                        Some("tunnel-id")
                    );
                    let unchanged_status =
                        patch_cloudflaredtunnel_status(&client, &namespace, "demo", |status| {
                            status.tunnel_id = Some("tunnel-id".to_string());
                            status.config_secret_ref = Some("config-secret".to_string());
                        })
                        .await
                        .expect("no-op status patch should succeed");
                    assert_eq!(
                        unchanged_status
                            .status
                            .as_ref()
                            .and_then(|status| status.config_secret_ref.as_deref()),
                        Some("config-secret")
                    );

                    assert!(
                        patch_opaque_secret_string(
                            &client,
                            "config-secret",
                            &namespace,
                            BTreeMap::from([("config.yml".to_string(), "value".to_string())]),
                            Some(vec![owner_ref.clone()]),
                        )
                        .await
                        .expect("secret creation should succeed")
                    );
                    assert!(
                        !patch_opaque_secret_string(
                            &client,
                            "config-secret",
                            &namespace,
                            BTreeMap::from([("config.yml".to_string(), "value".to_string())]),
                            Some(vec![owner_ref.clone()]),
                        )
                        .await
                        .expect("unchanged secret patch should succeed")
                    );

                    let created = patch_deployment(
                        &client,
                        "demo-cloudflared",
                        &namespace,
                        "config-secret",
                        "tunnel-id",
                        1,
                        &CloudflaredTunnelSpec::default(),
                        Some(vec![owner_ref.clone()]),
                    )
                    .await
                    .expect("deployment creation should succeed");
                    assert!(created);
                    let unchanged = patch_deployment(
                        &client,
                        "demo-cloudflared",
                        &namespace,
                        "config-secret",
                        "tunnel-id",
                        1,
                        &CloudflaredTunnelSpec::default(),
                        Some(vec![owner_ref.clone()]),
                    )
                    .await
                    .expect("unchanged deployment patch should succeed");
                    assert!(!unchanged);

                    let migrated = patch_deployment(
                        &client,
                        "demo-cloudflared",
                        &namespace,
                        "config-secret",
                        "tunnel-id",
                        1,
                        &CloudflaredTunnelSpec::default(),
                        Some(vec![migrated_owner_ref.clone()]),
                    )
                    .await
                    .expect("selector migration patch should succeed");
                    assert!(migrated);

                    let deployment_api = Api::<Deployment>::namespaced(client.clone(), &namespace);
                    let deployment = deployment_api
                        .get("demo-cloudflared")
                        .await
                        .expect("deployment should exist after migration");
                    assert_eq!(
                        deployment
                            .spec
                            .as_ref()
                            .and_then(|spec| spec.selector.match_labels.as_ref())
                            .and_then(|labels| labels.get(SELECTOR_ID_LABEL))
                            .map(String::as_str),
                        Some(migrated_owner_ref.uid.as_str())
                    );

                    let restarted = restart_deployment(&client, "demo-cloudflared", &namespace)
                        .await
                        .expect("restart should succeed");
                    assert_eq!(restarted.metadata.name.as_deref(), Some("demo-cloudflared"));
                }
            },
            {
                let client = client.clone();
                let namespace = namespace.clone();
                move || async move {
                    test_support::cleanup_namespace(&client, &namespace).await;
                }
            },
        )
        .await;
    }
}
