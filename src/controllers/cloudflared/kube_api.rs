use std::collections::BTreeMap;

use k8s_openapi::{
    api::{
        apps::v1::{Deployment, DeploymentSpec},
        core::v1::{
            Container, PodSpec, PodTemplateSpec, Secret, SecretVolumeSource, Volume, VolumeMount,
        },
    },
    apimachinery::pkg::apis::meta::v1::{LabelSelector, OwnerReference},
    ByteString,
};
use kube::{
    api::{ListParams, ObjectMeta, Patch, PatchParams},
    Api, Client,
};

use super::{
    customresource::{CloudflaredTunnelSpec, CloudflaredTunnelStatus},
    CloudflaredTunnel, CFD_DEPLOYMENT_IMAGE, PATCH_PARAMS_APPLY_NAME,
};
use crate::Result;

pub(super) async fn patch_cloudflaredtunnel_status<F: FnOnce(&mut CloudflaredTunnelStatus)>(
    client: &Client,
    namespace: &str,
    name: &str,
    update_fn: F,
) -> Result<CloudflaredTunnel> {
    let api = Api::<CloudflaredTunnel>::namespaced(client.clone(), namespace);
    let current_status = api.get_status(name).await?;
    let mut new_status = current_status
        .status
        .as_ref()
        .cloned()
        .unwrap_or(CloudflaredTunnelStatus::default());
    update_fn(&mut new_status);
    if current_status
        .status
        .as_ref()
        .map_or(false, |current_status| new_status == *current_status)
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
    let binary_data = data
        .into_iter()
        .map(|(k, v)| (k, ByteString(v.as_bytes().to_vec())))
        .collect();

    patch_opaque_secret(client, name, namespace, binary_data, owner_ref).await
}

pub(super) async fn patch_opaque_secret(
    client: &Client,
    name: &str,
    namespace: &str,
    data: BTreeMap<String, ByteString>,
    owner_ref: Option<Vec<OwnerReference>>,
) -> Result<bool> {
    let api = Api::<Secret>::namespaced(client.clone(), namespace);
    let secret = Secret {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            owner_references: owner_ref,
            ..Default::default()
        },
        data: Some(data),
        type_: Some("Opaque".to_string()),
        ..Default::default()
    };

    let before = api.get_opt(name).await?;

    let patched = api
        .patch(
            name,
            &PatchParams::apply(PATCH_PARAMS_APPLY_NAME).force(),
            &Patch::Apply(secret),
        )
        .await?;

    Ok(!before.map_or(false, |b| {
        b.metadata.resource_version == patched.metadata.resource_version
    }))
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

pub(super) async fn patch_deployment(
    client: &Client,
    name: &str,
    namespace: &str,
    tunnel_config_secret_name: &str,
    tunnel_id: &str,
    cfdt: &CloudflaredTunnelSpec,
    owner_ref: Option<Vec<OwnerReference>>,
) -> Result<bool> {
    let api = Api::<Deployment>::namespaced(client.clone(), namespace);

    let deployment = Deployment {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(namespace.to_string()),
            owner_references: owner_ref,
            ..Default::default()
        },
        spec: Some(DeploymentSpec {
            replicas: Some(1),
            selector: LabelSelector {
                match_labels: Some(BTreeMap::from([(
                    "app".to_string(),
                    "cloudflared".to_string(),
                )])),
                ..Default::default()
            },
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(BTreeMap::from([(
                        "app".to_string(),
                        "cloudflared".to_string(),
                    )])),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    containers: vec![Container {
                        command: cfdt.command.as_ref().cloned(),
                        args: cfdt.args.as_ref().cloned().or_else(|| {
                            Some(vec![
                                "tunnel".to_string(),
                                "--no-autoupdate".to_string(),
                                "--config".to_string(),
                                "/etc/cloudflared/config.yml".to_string(),
                                "run".to_string(),
                                tunnel_id.to_string(),
                            ])
                        }),
                        image: cfdt
                            .image
                            .as_ref()
                            .cloned()
                            .or(Some(CFD_DEPLOYMENT_IMAGE.to_string())),
                        name: name.to_string(),
                        volume_mounts: Some(vec![VolumeMount {
                            mount_path: "/etc/cloudflared".to_string(),
                            name: "tunne-config".to_string(),
                            read_only: Some(true),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    }],
                    volumes: Some(vec![Volume {
                        name: "tunne-config".to_string(),
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
    };

    let before = api.get_metadata_opt(name).await?;
    let patched = api
        .patch(
            name,
            &PatchParams::apply(PATCH_PARAMS_APPLY_NAME).force(),
            &Patch::Apply(deployment),
        )
        .await?;

    Ok(!before.map_or(false, |b| {
        b.metadata.generation == patched.metadata.generation
    }))
}
