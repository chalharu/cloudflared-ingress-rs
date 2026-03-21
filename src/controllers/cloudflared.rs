//! Cloudflared controller that reconciles CRDs, Cloudflare resources, and backing workloads.

mod cf_api;
mod cfd_config;
mod customresource;
mod kube_api;

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use base64::Engine;
use cloudflare::{
    endpoints::{
        cfd_tunnel::Tunnel,
        dns::dns::{DnsContent, DnsRecord},
    },
    framework::{
        Environment,
        auth::Credentials,
        client::{ClientConfig, async_api::Client as HttpApiClient},
    },
};
pub use customresource::{
    CloudflaredTunnel, CloudflaredTunnelAccess, CloudflaredTunnelIngress,
    CloudflaredTunnelOriginRequest, CloudflaredTunnelSpec,
};
use futures::{
    StreamExt as _,
    future::{BoxFuture, try_join_all},
};
use k8s_openapi::{
    ByteString, api::core::v1::Secret, apimachinery::pkg::apis::meta::v1::OwnerReference,
};
use kube::{
    Api, Client, Resource, ResourceExt as _,
    api::{DeleteParams, ObjectMeta, Patch, PatchParams},
    runtime::{Controller, controller::Action, finalizer::finalizer, watcher::Config},
};
use rand::{Rng, SeedableRng};
use tracing::{info, warn};
use uuid::Uuid;

use self::{
    cf_api::{CloudflareApi, tunnel_cname},
    kube_api::{
        get_cloudflaredtunnel, patch_cloudflaredtunnel_status, patch_deployment,
        patch_opaque_secret_string, restart_deployment,
    },
};
use crate::{Error, Result, cli::ControllerArgs};

const TUNNEL_SECRET_KEY: &str = "tunnel_secret";
const CFD_CONFIG_FILENAME: &str = "config.yml";
const PATCH_PARAMS_APPLY_NAME: &str = "cloudflaredtunnel.chalharu.top";
const CFD_DEPLOYMENT_IMAGE: &str = "cloudflare/cloudflared:2026.3.0";

#[derive(Debug, PartialEq, Eq)]
struct RenderedTunnelConfig {
    credential_filename: String,
    secret_data: BTreeMap<String, String>,
}

fn hostname_matches_zone(hostname: &str, zone_name: &str) -> bool {
    hostname == zone_name || hostname.ends_with(&format!(".{zone_name}"))
}

fn best_matching_zone_id<'a>(
    hostname: &str,
    zones: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Option<&'a str> {
    zones
        .into_iter()
        .filter(|(zone_name, _)| hostname_matches_zone(hostname, zone_name))
        .max_by_key(|(zone_name, _)| zone_name.len())
        .map(|(_, zone_id)| zone_id)
}

#[allow(clippy::result_large_err)]
fn desired_dns_records(
    spec: &CloudflaredTunnelSpec,
    zones: &[cloudflare::endpoints::zones::zone::Zone],
) -> Result<HashSet<(String, String)>> {
    let mut dns_list = HashSet::new();

    for ingress in spec
        .ingress
        .as_ref()
        .iter()
        .flat_map(|ingress| ingress.iter())
    {
        let Some(zone_id) = best_matching_zone_id(
            &ingress.hostname,
            zones
                .iter()
                .map(|zone| (zone.name.as_str(), zone.id.as_str())),
        )
        .map(str::to_string) else {
            return Err(Error::illegal_document());
        };
        dns_list.insert((ingress.hostname.clone(), zone_id));
    }

    Ok(dns_list)
}

#[allow(clippy::result_large_err)]
fn render_tunnel_config(
    account_id: &str,
    spec: &CloudflaredTunnelSpec,
    tunnel_id: &str,
    tunnel_secret: &[u8],
) -> Result<RenderedTunnelConfig> {
    let credential = cfd_config::Credentials {
        account_tag: account_id.to_string(),
        tunnel_secret: base64::engine::general_purpose::STANDARD.encode(tunnel_secret),
        tunnel_id: tunnel_id.to_string(),
    };
    let credential_filename = format!("{tunnel_id}.json");
    let credential_string = serde_json::to_string(&credential)?;
    let config = cfd_config::Config {
        tunnel: tunnel_id.to_string(),
        credentials_file: Some(format!("/etc/cloudflared/{credential_filename}")),
        origin_request: spec.origin_request.as_ref().cloned().map(Into::into),
        ingress: spec
            .ingress
            .as_ref()
            .iter()
            .flat_map(|ingress| ingress.iter().cloned().map(Into::into))
            .chain([cfd_config::Ingress {
                hostname: None,
                service: spec.default_ingress_service.clone(),
                path: None,
                origin_request: None,
            }])
            .collect(),
    };
    let config_string = serde_yaml::to_string(&config)?;

    Ok(RenderedTunnelConfig {
        credential_filename: credential_filename.clone(),
        secret_data: BTreeMap::from([
            (credential_filename, credential_string),
            (CFD_CONFIG_FILENAME.to_string(), config_string),
        ]),
    })
}

#[derive(Clone, Copy)]
struct DnsRecordRef<'a> {
    id: &'a str,
    name: &'a str,
    content: &'a DnsContent,
}

fn dns_record_ref(record: &DnsRecord) -> DnsRecordRef<'_> {
    DnsRecordRef {
        id: record.id.as_str(),
        name: record.name.as_str(),
        content: &record.content,
    }
}

fn collect_current_cname_records(
    zone_dns_list: &HashMap<String, Vec<DnsRecord>>,
    cname_content: &str,
) -> HashSet<(String, String)> {
    zone_dns_list
        .iter()
        .flat_map(|(zone_id, records)| {
            records
                .iter()
                .filter_map(move |record| match &record.content {
                    DnsContent::CNAME { content } if content.as_str() == cname_content => {
                        Some((record.id.clone(), zone_id.clone()))
                    }
                    _ => None,
                })
        })
        .collect()
}

fn split_reconcile_targets(
    cfdt_list: Vec<CloudflaredTunnel>,
    tunnel_list: Vec<Tunnel>,
    prefix: &str,
) -> (Vec<(CloudflaredTunnel, Option<Tunnel>)>, Vec<String>) {
    let mut tunnels_by_id = tunnel_list
        .into_iter()
        .map(|tunnel| (tunnel.id, tunnel))
        .collect::<HashMap<_, _>>();
    let reconcile_targets = cfdt_list
        .into_iter()
        .map(|cfdt| {
            let tunnel = cfdt
                .status
                .as_ref()
                .and_then(|status| status.tunnel_id.as_deref())
                .and_then(|id| Uuid::parse_str(id).ok())
                .and_then(|id| tunnels_by_id.remove(&id));
            (cfdt, tunnel)
        })
        .collect();
    let stale_tunnel_ids = tunnels_by_id
        .into_iter()
        .filter(|(_, tunnel)| tunnel.name.starts_with(prefix))
        .map(|(id, _)| {
            id.as_hyphenated()
                .encode_lower(&mut Uuid::encode_buffer())
                .to_string()
        })
        .collect();

    (reconcile_targets, stale_tunnel_ids)
}

#[allow(clippy::result_large_err)]
fn matching_hostname_cname_record<'a>(
    dns_records: impl IntoIterator<Item = DnsRecordRef<'a>>,
    hostname: &str,
    cname_content: &str,
) -> Result<Option<&'a str>> {
    let mut matching_dns_record_id = None;

    for dns_record in dns_records {
        if dns_record.name != hostname {
            continue;
        }

        match dns_record.content {
            DnsContent::CNAME { content } if content.as_str() == cname_content => {
                matching_dns_record_id = Some(dns_record.id);
            }
            DnsContent::A { .. } | DnsContent::AAAA { .. } | DnsContent::CNAME { .. } => {
                return Err(Error::illegal_document());
            }
            _ => {}
        }
    }

    Ok(matching_dns_record_id)
}

async fn sync_tunnel_dns_records(
    cloudflare_api: &CloudflareApi,
    desired_dns_records: &HashSet<(String, String)>,
    zone_dns_list: &HashMap<String, Vec<DnsRecord>>,
    tunnel_id: &str,
) -> Result<()> {
    let cname_content = tunnel_cname(tunnel_id);
    let mut current_cname_records = collect_current_cname_records(zone_dns_list, &cname_content);

    for (hostname, zone_id) in desired_dns_records {
        let dns_records = zone_dns_list
            .get(zone_id)
            .ok_or_else(Error::illegal_document)?;

        if let Some(dns_record_id) = matching_hostname_cname_record(
            dns_records.iter().map(dns_record_ref),
            hostname,
            &cname_content,
        )? {
            current_cname_records.remove(&(dns_record_id.to_string(), zone_id.clone()));
            continue;
        }

        cloudflare_api
            .create_dns_cname(zone_id.as_str(), tunnel_id, hostname.as_str())
            .await?;
    }

    for (dns_id, zone_id) in current_cname_records {
        cloudflare_api
            .delete_dns_cname(zone_id.as_str(), dns_id.as_str())
            .await?;
    }

    Ok(())
}

#[allow(clippy::result_large_err)]
fn cloudflared_owner_reference(cfdt: &CloudflaredTunnel) -> Result<OwnerReference> {
    Ok(OwnerReference {
        api_version: CloudflaredTunnel::api_version(&()).to_string(),
        kind: CloudflaredTunnel::kind(&()).to_string(),
        name: cfdt.name_any(),
        uid: cfdt.uid().ok_or_else(Error::illegal_document)?,
        ..Default::default()
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SecretRefSelection<'a> {
    Existing(&'a str),
    UseSpec {
        spec_ref: &'a str,
        previous_status_ref: Option<&'a str>,
    },
    GenerateNew,
}

fn select_tunnel_secret_ref<'a>(
    spec_ref: Option<&'a str>,
    status_ref: Option<&'a str>,
) -> SecretRefSelection<'a> {
    match (spec_ref, status_ref) {
        (None, Some(status_ref)) => SecretRefSelection::Existing(status_ref),
        (Some(spec_ref), Some(status_ref)) if spec_ref == status_ref => {
            SecretRefSelection::Existing(status_ref)
        }
        (Some(spec_ref), previous_status_ref) => SecretRefSelection::UseSpec {
            spec_ref,
            previous_status_ref,
        },
        (None, None) => SecretRefSelection::GenerateNew,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigRefSelection<'a> {
    Existing(&'a str),
    GenerateNew,
}

fn select_config_secret_ref(status_ref: Option<&str>) -> ConfigRefSelection<'_> {
    match status_ref {
        Some(status_ref) => ConfigRefSelection::Existing(status_ref),
        None => ConfigRefSelection::GenerateNew,
    }
}

#[allow(clippy::result_large_err)]
fn validate_tunnel_secret(secret: Vec<u8>) -> Result<Vec<u8>> {
    if secret.len() < 32 {
        return Err(Error::illegal_document());
    }

    Ok(secret)
}

// Context for our reconciler
struct Context {
    /// Kubernetes client
    client: Client,
    args: ControllerArgs,
    cloudflare_api: CloudflareApi,
}

pub async fn run_controller(args: ControllerArgs) -> Result<()> {
    info!("Starting controller for CloudflaredTunnel");

    let client = Client::try_default().await?;
    let credential = Credentials::UserAuthToken {
        token: args.cloudflare_token().to_string(),
    };
    let cloudflare_api = CloudflareApi::new(Arc::new(HttpApiClient::new(
        credential,
        ClientConfig::default(),
        Environment::Production,
    )?));

    let context = Arc::new(Context {
        client: client.clone(),
        args,
        cloudflare_api,
    });

    let api = Api::<CloudflaredTunnel>::all(client);

    Controller::new(api, Config::default().any_semantic())
        .shutdown_on_signal()
        .run(reconcile, error_policy, context)
        .filter_map(|x| async move { std::result::Result::ok(x) })
        .for_each(|_| futures::future::ready(()))
        .await;

    info!("controller for CloudflaredTunnel shutdown");
    Ok(())
}

async fn reconcile(res: Arc<CloudflaredTunnel>, ctx: Arc<Context>) -> Result<Action> {
    // let name = res.name_any();
    let ns = res.namespace().ok_or_else(Error::illegal_document)?;
    // info!("Reconciling CloudflaredTunnel \"{name}\" in {ns}");
    let api = Api::<CloudflaredTunnel>::namespaced(ctx.client.clone(), &ns);
    let finalizer_name = format!("{}/finalizer", PATCH_PARAMS_APPLY_NAME);
    finalizer(&api, &finalizer_name, res, |e| async move {
        match e {
            kube::runtime::finalizer::Event::Apply(_) => ctx.reconcile().await?,
            kube::runtime::finalizer::Event::Cleanup(t) => ctx.delete_tunnel(t).await?,
        }
        Ok(Action::requeue(Duration::from_secs(60 * 60)))
    })
    .await
    .map_err(|e| Error::from(Box::new(e)))
}

fn error_policy<K>(_: Arc<K>, error: &Error, _ctx: Arc<Context>) -> Action {
    warn!("reconcile failed: {error:?}");
    Action::requeue(Duration::from_secs(60))
}

impl Context {
    async fn delete_tunnel(&self, cfdt: Arc<CloudflaredTunnel>) -> Result<()> {
        let Some(tunnel_id) = cfdt.status.as_ref().and_then(|x| x.tunnel_id.as_ref()) else {
            return Ok(());
        };

        let tunnel = self
            .cloudflare_api
            .get_tunnel_opt(self.args.cloudflare_account_id(), tunnel_id)
            .await?;

        let zones = self.cloudflare_api.list_zone().await?;
        try_join_all(zones.iter().map(|z| async {
            let dns_records = self
                .cloudflare_api
                .list_dns_cname(z.id.as_str(), tunnel_id)
                .await?;
            for d in dns_records.into_iter() {
                self.cloudflare_api
                    .delete_dns_cname(z.id.as_str(), d.id.as_str())
                    .await?;
            }
            Result::<_, Error>::Ok(())
        }))
        .await?;

        if tunnel.is_some() {
            self.cloudflare_api
                .delete_tunnel(self.args.cloudflare_account_id(), tunnel_id)
                .await?;
        }
        Ok(())
    }

    async fn reconcile(&self) -> Result<()> {
        let cfdt_list = get_cloudflaredtunnel(&self.client).await?;
        let account_id = self.args.cloudflare_account_id().to_string();
        let tunnel_list = self
            .cloudflare_api
            .list_tunnels(account_id.as_str(), self.args.cloudflare_tunnel_prefix())
            .await?;
        let (reconcile_targets, stale_tunnel_ids) =
            split_reconcile_targets(cfdt_list, tunnel_list, self.args.cloudflare_tunnel_prefix());

        for (cfdt, tunnel) in reconcile_targets {
            self.reconcile_tunnel(cfdt, tunnel).await?;
        }

        for tunnel_id in stale_tunnel_ids {
            if let Err(e) = self
                .cloudflare_api
                .delete_tunnel(account_id.as_str(), &tunnel_id)
                .await
            {
                // tunnel削除の失敗は警告のみとする
                warn!("Delete cloudflare tunnel failed: {}", e);
            }
        }

        Ok(())
    }

    async fn create_tunnel(
        &self,
        name: &str,
        namespace: &str,
        tunnel_secret: &[u8],
    ) -> Result<Tunnel> {
        let tunnel_name_prefix = self.args.cloudflare_tunnel_prefix();
        let uid = Uuid::new_v4().as_hyphenated().to_string();
        let tunnel_name = format!("{tunnel_name_prefix}{uid}");
        let tunnel = self
            .cloudflare_api
            .create_tunnel(
                self.args.cloudflare_account_id(),
                tunnel_name.as_str(),
                tunnel_secret,
            )
            .await?;
        patch_cloudflaredtunnel_status(&self.client, namespace, name, |status| {
            status.tunnel_id = Some(tunnel.id.as_hyphenated().to_string())
        })
        .await?;
        Ok(tunnel)
    }

    async fn reconcile_tunnel(
        &self,
        cfdt: CloudflaredTunnel,
        tunnel: Option<Tunnel>,
    ) -> Result<()> {
        info!("Reconcile cloudflaredTunnel: {}", cfdt.name_any());
        let name = cfdt.name_any();
        let namespace = cfdt.namespace().ok_or_else(Error::illegal_document)?;
        let owner_ref = cloudflared_owner_reference(&cfdt)?;

        // DNS ZoneのリストをCloudflareから取得
        let zones = self.cloudflare_api.list_zone().await?;
        let dns_list = desired_dns_records(&cfdt.spec, &zones)?;

        // ZoneIDからDNSレコードを引く辞書を作成
        let zone_dns_list = try_join_all(zones.iter().map(|z| async {
            Result::<_, Error>::Ok(
                self.cloudflare_api
                    .list_dns(z.id.as_str())
                    .await?
                    .into_iter()
                    .fold(
                        HashMap::new(),
                        |mut acc: HashMap<String, Vec<DnsRecord>>, value| {
                            acc.entry(z.id.clone()).or_default().push(value);
                            acc
                        },
                    ),
            )
        }))
        .await?
        .into_iter()
        .flat_map(|x| x.into_iter())
        .collect::<HashMap<_, _>>();

        let tunnel_secret = self.get_tunnel_secret(&cfdt, owner_ref.clone()).await?;

        let tunnel = tunnel
            .map_or_else::<BoxFuture<Result<_>>, _, _>(
                || Box::pin(self.create_tunnel(&name, &namespace, &tunnel_secret)),
                |x| Box::pin(async { Ok(x) }),
            )
            .await?;
        let tunnel_id = tunnel.id.as_hyphenated().to_string();
        sync_tunnel_dns_records(&self.cloudflare_api, &dns_list, &zone_dns_list, &tunnel_id)
            .await?;

        let (tunnel_config_secret_name, secret_updated) = self
            .get_tunnel_config(&cfdt, owner_ref.clone(), tunnel, &tunnel_secret)
            .await?;

        let deployment_name = format!("{}-{}", name, "cloudflared");
        let created = patch_deployment(
            &self.client,
            &deployment_name,
            &namespace,
            &tunnel_config_secret_name,
            &tunnel_id,
            self.args.deployment_replicas().try_into()?,
            &cfdt.spec,
            Some(vec![owner_ref]),
        )
        .await?;

        // secretが更新されている場合はrestartを行う
        if !created && secret_updated {
            restart_deployment(&self.client, &deployment_name, &namespace).await?;
        }

        Ok(())
    }

    async fn get_tunnel_secret(
        &self,
        cfdt: &CloudflaredTunnel,
        owner_ref: OwnerReference,
    ) -> Result<Vec<u8>> {
        let spec_ref = cfdt.spec.secret_ref.as_ref();
        let status_ref = cfdt
            .status
            .as_ref()
            .and_then(|s| s.tunnel_secret_ref.as_ref());
        let ns = cfdt.namespace().ok_or_else(Error::illegal_document)?;
        let api = Api::<Secret>::namespaced(self.client.clone(), &ns);

        let secret_ref = match select_tunnel_secret_ref(
            spec_ref.map(String::as_str),
            status_ref.map(String::as_str),
        ) {
            SecretRefSelection::Existing(secret_ref) => secret_ref.to_string(),
            SecretRefSelection::UseSpec {
                spec_ref,
                previous_status_ref,
            } => {
                if let Some(previous_status_ref) = previous_status_ref
                    && let Some(secret) = api.get_opt(previous_status_ref).await?
                    && secret.owner_references().contains(&owner_ref)
                {
                    api.delete(&secret.name_any(), &DeleteParams::background())
                        .await?;
                }

                let secret_ref = spec_ref.to_string();
                patch_cloudflaredtunnel_status(&self.client, &ns, &cfdt.name_any(), |status| {
                    status.tunnel_secret_ref = Some(secret_ref.clone())
                })
                .await?;
                secret_ref
            }
            SecretRefSelection::GenerateNew => {
                let secret_ref = Uuid::new_v4()
                    .as_hyphenated()
                    .encode_lower(&mut Uuid::encode_buffer())
                    .to_string();
                patch_cloudflaredtunnel_status(&self.client, &ns, &cfdt.name_any(), |status| {
                    status.tunnel_secret_ref = Some(secret_ref.clone())
                })
                .await?;
                secret_ref
            }
        };

        let secret = if let Some(mut data) = api
            .get_opt(&secret_ref)
            .await?
            .and_then(|secret| secret.data)
        {
            data.remove(TUNNEL_SECRET_KEY)
                .ok_or_else(Error::illegal_document)?
                .0
        } else {
            let raw_data = tokio::task::spawn_blocking(|| {
                let mut raw_data = vec![0u8; 32];
                let mut rng = rand::rngs::StdRng::try_from_rng(&mut rand::rngs::SysRng)
                    .expect("system RNG should be available");
                rng.fill_bytes(raw_data.as_mut_slice());
                raw_data
            })
            .await?;
            let data =
                BTreeMap::from([(TUNNEL_SECRET_KEY.to_string(), ByteString(raw_data.clone()))]);
            api.patch(
                &secret_ref,
                &PatchParams::apply(PATCH_PARAMS_APPLY_NAME).force(),
                &Patch::Apply(Secret {
                    data: Some(data),
                    type_: Some("Opaque".to_string()),
                    metadata: ObjectMeta {
                        owner_references: Some(vec![owner_ref.clone()]),
                        name: Some(secret_ref.clone()),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
            )
            .await?;
            raw_data
        };

        validate_tunnel_secret(secret)
    }

    async fn get_tunnel_config(
        &self,
        cfdt: &CloudflaredTunnel,
        owner_ref: OwnerReference,
        tunnel: Tunnel,
        tunnel_secret: &[u8],
    ) -> Result<(String, bool)> {
        let tunnel_id = tunnel.id.as_hyphenated().to_string();
        let ns = cfdt.namespace().ok_or_else(Error::illegal_document)?;
        let rendered = render_tunnel_config(
            self.args.cloudflare_account_id(),
            &cfdt.spec,
            &tunnel_id,
            tunnel_secret,
        )?;

        let config_ref = match select_config_secret_ref(
            cfdt.status
                .as_ref()
                .and_then(|status| status.config_secret_ref.as_deref()),
        ) {
            ConfigRefSelection::Existing(config_ref) => config_ref.to_string(),
            ConfigRefSelection::GenerateNew => {
                let config_ref = Uuid::new_v4()
                    .as_hyphenated()
                    .encode_lower(&mut Uuid::encode_buffer())
                    .to_string();

                // statusに新しいconfig_refを設定
                patch_cloudflaredtunnel_status(&self.client, &ns, &cfdt.name_any(), |status| {
                    status.config_secret_ref = Some(config_ref.clone())
                })
                .await?;
                config_ref
            }
        };

        let secret_updated = patch_opaque_secret_string(
            &self.client,
            &config_ref,
            &ns,
            rendered.secret_data,
            Some(vec![owner_ref.clone()]),
        )
        .await?;

        Ok((config_ref, secret_updated))
    }
}

#[cfg(test)]
mod tests {
    use super::customresource::CloudflaredTunnelStatus;
    use super::*;
    use cloudflare::endpoints::zones::zone::Zone;
    use cloudflare::framework::{
        Environment,
        auth::Credentials,
        client::{ClientConfig, async_api::Client as HttpApiClient},
    };

    use serde_json::json;
    use serde_yaml::Value;

    const ZONE_ID: &str = "00000000000000000000000000000001";
    const STALE_DNS_ID: &str = "00000000000000000000000000000002";

    #[test]
    fn hostname_matches_zone_accepts_apex_and_subdomains_only() {
        assert!(hostname_matches_zone("example.com", "example.com"));
        assert!(hostname_matches_zone("api.example.com", "example.com"));
        assert!(!hostname_matches_zone("badexample.com", "example.com"));
    }

    #[test]
    fn best_matching_zone_id_matches_zone_apex() {
        assert_eq!(
            best_matching_zone_id("example.com", [("example.com", "zone-a")]),
            Some("zone-a")
        );
    }

    #[test]
    fn best_matching_zone_id_prefers_the_most_specific_zone() {
        assert_eq!(
            best_matching_zone_id(
                "api.eu.example.com",
                [("example.com", "zone-a"), ("eu.example.com", "zone-b"),],
            ),
            Some("zone-b")
        );
    }

    #[test]
    fn best_matching_zone_id_rejects_partial_suffix_matches() {
        assert_eq!(
            best_matching_zone_id("badexample.com", [("example.com", "zone-a")]),
            None
        );
    }

    fn zone(id: &str, name: &str) -> Zone {
        serde_json::from_value(json!({
            "id": id,
            "name": name,
            "status": "active",
            "paused": false,
            "type": "full",
            "development_mode": 0,
            "name_servers": [],
            "original_name_servers": [],
            "original_registrar": null,
            "original_dnshost": null,
            "modified_on": "2000-01-01T00:00:00.000000Z",
            "created_on": "2000-01-01T00:00:00.000000Z",
            "activated_on": "2000-01-01T00:00:00.000000Z",
            "meta": {
                "step": 0,
                "custom_certificate_quota": 0,
                "page_rule_quota": 0,
                "phishing_detected": false
            },
            "owner": {
                "id": null,
                "type": "user",
                "email": null
            },
            "account": {
                "id": "",
                "name": "Example account"
            },
            "tenant": {},
            "tenant_unit": {},
            "permissions": [],
            "plan": {
                "id": "",
                "name": "",
                "price": 0,
                "currency": "",
                "frequency": "",
                "is_subscribed": false,
                "can_subscribe": false,
                "legacy_id": "",
                "legacy_discount": false,
                "externally_managed": false
            }
        }))
        .expect("zone should deserialize")
    }

    #[test]
    fn desired_dns_records_returns_empty_when_there_is_no_ingress_configuration() {
        let records = desired_dns_records(
            &CloudflaredTunnelSpec {
                default_ingress_service: "http_status:404".to_string(),
                ..Default::default()
            },
            &[zone("zone-a", "example.com")],
        )
        .expect("missing ingress rules should not fail");

        assert!(records.is_empty());
    }

    #[test]
    fn desired_dns_records_uses_the_most_specific_matching_zone() {
        let records = desired_dns_records(
            &CloudflaredTunnelSpec {
                ingress: Some(vec![
                    CloudflaredTunnelIngress {
                        hostname: "api.example.com".to_string(),
                        service: "https://service".to_string(),
                        path: None,
                        origin_request: None,
                    },
                    CloudflaredTunnelIngress {
                        hostname: "app.eu.example.com".to_string(),
                        service: "https://service".to_string(),
                        path: None,
                        origin_request: None,
                    },
                ]),
                default_ingress_service: "http_status:404".to_string(),
                ..Default::default()
            },
            &[
                zone("zone-a", "example.com"),
                zone("zone-b", "eu.example.com"),
            ],
        )
        .expect("zones should match");

        assert!(records.contains(&("api.example.com".to_string(), "zone-a".to_string())));
        assert!(records.contains(&("app.eu.example.com".to_string(), "zone-b".to_string())));
    }

    #[test]
    fn desired_dns_records_rejects_hostnames_outside_known_zones() {
        let result = desired_dns_records(
            &CloudflaredTunnelSpec {
                ingress: Some(vec![CloudflaredTunnelIngress {
                    hostname: "api.other.com".to_string(),
                    service: "https://service".to_string(),
                    path: None,
                    origin_request: None,
                }]),
                default_ingress_service: "http_status:404".to_string(),
                ..Default::default()
            },
            &[zone("zone-a", "example.com")],
        );

        assert!(matches!(result, Err(Error::IllegalDocument { .. })));
    }

    #[test]
    fn render_tunnel_config_renders_credentials_and_default_ingress() {
        let rendered = render_tunnel_config(
            "account-id",
            &CloudflaredTunnelSpec {
                origin_request: Some(CloudflaredTunnelOriginRequest {
                    no_tls_verify: Some(true),
                    ..Default::default()
                }),
                ingress: Some(vec![CloudflaredTunnelIngress {
                    hostname: "example.com".to_string(),
                    service: "https://service.default.svc".to_string(),
                    path: Some("^/api".to_string()),
                    origin_request: None,
                }]),
                default_ingress_service: "http_status:404".to_string(),
                ..Default::default()
            },
            "tunnel-id",
            b"secret",
        )
        .expect("config should render");

        assert_eq!(rendered.credential_filename, "tunnel-id.json");
        assert!(rendered.secret_data.contains_key("tunnel-id.json"));
        assert!(rendered.secret_data.contains_key(CFD_CONFIG_FILENAME));
        assert!(rendered.secret_data["tunnel-id.json"].contains("\"AccountTag\":\"account-id\""));

        let yaml: Value = serde_yaml::from_str(&rendered.secret_data[CFD_CONFIG_FILENAME])
            .expect("yaml should parse");
        assert_eq!(yaml["tunnel"], "tunnel-id");
        assert_eq!(yaml["credentials-file"], "/etc/cloudflared/tunnel-id.json");
        assert_eq!(
            yaml["ingress"]
                .as_sequence()
                .expect("ingress should be a list")
                .len(),
            2
        );
        assert_eq!(yaml["ingress"][1]["service"], "http_status:404");
    }

    async fn create_cloudflare_api(url: &str) -> CloudflareApi {
        let api = HttpApiClient::new(
            Credentials::UserAuthToken {
                token: "DEADBEAF".to_string(),
            },
            ClientConfig::default(),
            Environment::Custom(url.to_string()),
        )
        .expect("client should build");

        CloudflareApi::new(Arc::new(api))
    }

    fn dns_record(id: &str, name: &str, content: &str) -> DnsRecord {
        serde_json::from_value(json!({
            "id": id,
            "zone_id": ZONE_ID,
            "zone_name": "example.com",
            "name": name,
            "type": "CNAME",
            "content": content,
            "proxiable": true,
            "proxied": true,
            "ttl": 1,
            "settings": {},
            "meta": {
                "auto_added": false,
                "managed_by_apps": false,
                "managed_by_argo_tunnel": false
            },
            "comment": null,
            "tags": [],
            "created_on": "2000-01-01T00:00:00.000000Z",
            "modified_on": "2000-01-01T00:00:00.000000Z"
        }))
        .expect("dns record should deserialize")
    }

    #[test]
    fn dns_record_ref_returns_borrowed_record_fields() {
        let record = dns_record("dns-1", "app.example.com", "demo.cfargotunnel.com");
        let record_ref = dns_record_ref(&record);

        assert_eq!(record_ref.id, "dns-1");
        assert_eq!(record_ref.name, "app.example.com");
        assert!(matches!(
            record_ref.content,
            DnsContent::CNAME { content } if content == "demo.cfargotunnel.com"
        ));
    }

    #[test]
    fn collect_current_cname_records_keeps_only_records_for_the_target_tunnel() {
        let records = collect_current_cname_records(
            &HashMap::from([(
                ZONE_ID.to_string(),
                vec![
                    dns_record("dns-1", "app.example.com", "demo.cfargotunnel.com"),
                    dns_record("dns-2", "other.example.com", "other.cfargotunnel.com"),
                ],
            )]),
            "demo.cfargotunnel.com",
        );

        assert_eq!(
            records,
            HashSet::from([("dns-1".to_string(), ZONE_ID.to_string())])
        );
    }

    #[test]
    fn matching_hostname_cname_record_returns_the_matching_record_id() {
        let other_content = DnsContent::CNAME {
            content: "demo.cfargotunnel.com".to_string(),
        };
        let matching_content = DnsContent::CNAME {
            content: "demo.cfargotunnel.com".to_string(),
        };

        let record_id = matching_hostname_cname_record(
            [
                DnsRecordRef {
                    id: "dns-1",
                    name: "other.example.com",
                    content: &other_content,
                },
                DnsRecordRef {
                    id: "dns-2",
                    name: "app.example.com",
                    content: &matching_content,
                },
            ],
            "app.example.com",
            "demo.cfargotunnel.com",
        )
        .expect("matching CNAME should be accepted");

        assert_eq!(record_id, Some("dns-2"));
    }

    #[test]
    fn matching_hostname_cname_record_returns_none_when_no_match() {
        let other_content = DnsContent::CNAME {
            content: "demo.cfargotunnel.com".to_string(),
        };

        let record_id = matching_hostname_cname_record(
            [DnsRecordRef {
                id: "dns-1",
                name: "other.example.com",
                content: &other_content,
            }],
            "app.example.com",
            "demo.cfargotunnel.com",
        )
        .expect("unrelated records should be ignored");

        assert_eq!(record_id, None);
    }

    #[test]
    fn matching_hostname_cname_record_rejects_conflicting_cname() {
        let conflicting_content = DnsContent::CNAME {
            content: "other.cfargotunnel.com".to_string(),
        };

        let result = matching_hostname_cname_record(
            [DnsRecordRef {
                id: "dns-1",
                name: "app.example.com",
                content: &conflicting_content,
            }],
            "app.example.com",
            "demo.cfargotunnel.com",
        );

        assert!(matches!(result, Err(Error::IllegalDocument { .. })));
    }

    #[test]
    fn matching_hostname_cname_record_rejects_a_record() {
        let conflicting_content = DnsContent::A {
            content: std::net::Ipv4Addr::new(192, 0, 2, 1),
        };

        let result = matching_hostname_cname_record(
            [DnsRecordRef {
                id: "dns-1",
                name: "app.example.com",
                content: &conflicting_content,
            }],
            "app.example.com",
            "demo.cfargotunnel.com",
        );

        assert!(matches!(result, Err(Error::IllegalDocument { .. })));
    }

    #[test]
    fn matching_hostname_cname_record_rejects_aaaa_record() {
        let conflicting_content = DnsContent::AAAA {
            content: std::net::Ipv6Addr::LOCALHOST,
        };

        let result = matching_hostname_cname_record(
            [DnsRecordRef {
                id: "dns-1",
                name: "app.example.com",
                content: &conflicting_content,
            }],
            "app.example.com",
            "demo.cfargotunnel.com",
        );

        assert!(matches!(result, Err(Error::IllegalDocument { .. })));
    }

    #[tokio::test]
    async fn sync_tunnel_dns_records_requires_zone_dns_entries_for_each_desired_record() {
        let server = mockito::Server::new_async().await;
        let api = create_cloudflare_api(server.url().as_str()).await;
        let desired_dns_records =
            HashSet::from([("app.example.com".to_string(), ZONE_ID.to_string())]);
        let zone_dns_list = HashMap::new();

        let result = sync_tunnel_dns_records(&api, &desired_dns_records, &zone_dns_list, "demo");

        assert!(matches!(result.await, Err(Error::IllegalDocument { .. })));
    }

    #[tokio::test]
    async fn sync_tunnel_dns_records_keeps_matching_records_without_api_calls() {
        let server = mockito::Server::new_async().await;
        let api = create_cloudflare_api(server.url().as_str()).await;
        let tunnel_id = "a0000000000000000000000000000002";
        let tunnel_target = tunnel_cname(tunnel_id);
        let desired_dns_records =
            HashSet::from([("app.example.com".to_string(), ZONE_ID.to_string())]);
        let zone_dns_list = HashMap::from([(
            ZONE_ID.to_string(),
            vec![dns_record("dns-1", "app.example.com", &tunnel_target)],
        )]);

        sync_tunnel_dns_records(&api, &desired_dns_records, &zone_dns_list, tunnel_id)
            .await
            .expect("matching records should not require API changes");
    }

    #[tokio::test]
    async fn sync_tunnel_dns_records_creates_missing_records_and_deletes_stale_ones() {
        let mut server = mockito::Server::new_async().await;
        let tunnel_id = "a0000000000000000000000000000002";
        let tunnel_target = tunnel_cname(tunnel_id);
        let create_dns_mock = server
            .mock("POST", format!("/zones/{ZONE_ID}/dns_records").as_str())
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"result":{"id":"created-record","zone_id":"00000000000000000000000000000001","zone_name":"example.com","name":"app.example.com","type":"CNAME","content":"a0000000000000000000000000000002.cfargotunnel.com","proxiable":true,"proxied":true,"ttl":1,"settings":{},"meta":{"auto_added":false,"managed_by_apps":false,"managed_by_argo_tunnel":false},"comment":null,"tags":[],"created_on":"2000-01-01T00:00:00.000000Z","modified_on":"2000-01-01T00:00:00.000000Z"},"result_info":{},"success":true,"errors":[],"messages":[]}"#,
            )
            .create_async()
            .await;
        let delete_dns_mock = server
            .mock(
                "DELETE",
                format!("/zones/{ZONE_ID}/dns_records/{STALE_DNS_ID}").as_str(),
            )
            .with_status(201)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"result":{"id":"00000000000000000000000000000001"},"result_info":{},"success":true,"errors":[],"messages":[]}"#,
            )
            .create_async()
            .await;
        let api = create_cloudflare_api(server.url().as_str()).await;
        let desired_dns_records =
            HashSet::from([("app.example.com".to_string(), ZONE_ID.to_string())]);
        let zone_dns_list = HashMap::from([(
            ZONE_ID.to_string(),
            vec![dns_record(
                STALE_DNS_ID,
                "stale.example.com",
                &tunnel_target,
            )],
        )]);

        sync_tunnel_dns_records(&api, &desired_dns_records, &zone_dns_list, tunnel_id)
            .await
            .expect("missing and stale records should be reconciled");

        create_dns_mock.assert_async().await;
        delete_dns_mock.assert_async().await;
    }

    fn cloudflared_tunnel(
        name: &str,
        tunnel_id: Option<&str>,
        uid: Option<&str>,
    ) -> CloudflaredTunnel {
        CloudflaredTunnel {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some("default".to_string()),
                uid: uid.map(str::to_string),
                ..Default::default()
            },
            spec: CloudflaredTunnelSpec {
                default_ingress_service: "http_status:404".to_string(),
                ..Default::default()
            },
            status: tunnel_id.map(|tunnel_id| CloudflaredTunnelStatus {
                tunnel_id: Some(tunnel_id.to_string()),
                ..Default::default()
            }),
        }
    }

    fn tunnel(id: &str, name: &str) -> Tunnel {
        serde_json::from_value(json!({
            "id": id,
            "created_at": "2000-01-01T00:00:00.000000Z",
            "deleted_at": null,
            "name": name,
            "connections": [],
            "metadata": {}
        }))
        .expect("tunnel should deserialize")
    }

    #[test]
    fn split_reconcile_targets_matches_existing_tunnels_and_filters_prefixed_stale_tunnels() {
        let managed_tunnel_id = "00000000-0000-0000-0000-000000000001";
        let stale_tunnel_id = "00000000-0000-0000-0000-000000000002";
        let external_tunnel_id = "00000000-0000-0000-0000-000000000003";
        let (reconcile_targets, stale_tunnel_ids) = split_reconcile_targets(
            vec![
                cloudflared_tunnel("managed", Some(managed_tunnel_id), Some("uid-managed")),
                cloudflared_tunnel("new", None, Some("uid-new")),
            ],
            vec![
                tunnel(managed_tunnel_id, "k8s-ingress-managed"),
                tunnel(stale_tunnel_id, "k8s-ingress-stale"),
                tunnel(external_tunnel_id, "external-shared"),
            ],
            "k8s-ingress-",
        );

        assert_eq!(reconcile_targets.len(), 2);
        assert_eq!(reconcile_targets[0].0.name_any(), "managed");
        assert_eq!(
            reconcile_targets[0]
                .1
                .as_ref()
                .map(|tunnel| tunnel.name.as_str()),
            Some("k8s-ingress-managed")
        );
        assert_eq!(reconcile_targets[1].0.name_any(), "new");
        assert!(reconcile_targets[1].1.is_none());
        assert_eq!(stale_tunnel_ids, vec![stale_tunnel_id.to_string()]);
    }

    #[test]
    fn cloudflared_owner_reference_uses_the_resource_identity() {
        let owner_ref =
            cloudflared_owner_reference(&cloudflared_tunnel("managed", None, Some("uid-managed")))
                .expect("owner reference should build");

        assert_eq!(owner_ref.api_version, "chalharu.top/v1alpha1");
        assert_eq!(owner_ref.kind, "CloudflaredTunnel");
        assert_eq!(owner_ref.name, "managed");
        assert_eq!(owner_ref.uid, "uid-managed");
    }

    #[test]
    fn cloudflared_owner_reference_requires_a_uid() {
        let result = cloudflared_owner_reference(&cloudflared_tunnel("managed", None, None));

        assert!(matches!(result, Err(Error::IllegalDocument { .. })));
    }

    #[test]
    fn select_tunnel_secret_ref_covers_existing_spec_and_generated_states() {
        assert_eq!(
            select_tunnel_secret_ref(None, Some("status-secret")),
            SecretRefSelection::Existing("status-secret")
        );
        assert_eq!(
            select_tunnel_secret_ref(Some("spec-secret"), Some("spec-secret")),
            SecretRefSelection::Existing("spec-secret")
        );
        assert_eq!(
            select_tunnel_secret_ref(Some("spec-secret"), Some("old-secret")),
            SecretRefSelection::UseSpec {
                spec_ref: "spec-secret",
                previous_status_ref: Some("old-secret"),
            }
        );
        assert_eq!(
            select_tunnel_secret_ref(Some("spec-secret"), None),
            SecretRefSelection::UseSpec {
                spec_ref: "spec-secret",
                previous_status_ref: None,
            }
        );
        assert_eq!(
            select_tunnel_secret_ref(None, None),
            SecretRefSelection::GenerateNew
        );
    }

    #[test]
    fn select_config_secret_ref_reuses_status_or_requests_generation() {
        assert_eq!(
            select_config_secret_ref(Some("config-secret")),
            ConfigRefSelection::Existing("config-secret")
        );
        assert_eq!(
            select_config_secret_ref(None),
            ConfigRefSelection::GenerateNew
        );
    }

    #[test]
    fn validate_tunnel_secret_requires_at_least_32_bytes() {
        assert!(matches!(
            validate_tunnel_secret(vec![0; 31]),
            Err(Error::IllegalDocument { .. })
        ));
        assert_eq!(
            validate_tunnel_secret(vec![7; 32]).expect("32-byte secret should be accepted"),
            vec![7; 32]
        );
    }
}
