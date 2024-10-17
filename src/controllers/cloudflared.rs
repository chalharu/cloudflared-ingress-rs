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
        dns::{DnsContent, DnsRecord},
    },
    framework::{
        async_api::Client as HttpApiClient, auth::Credentials, Environment, HttpApiClientConfig,
    },
};
pub use customresource::{CloudflaredTunnel, CloudflaredTunnelIngress, CloudflaredTunnelSpec};
use futures::{
    future::{try_join_all, BoxFuture},
    StreamExt as _,
};
use k8s_openapi::{
    api::core::v1::Secret, apimachinery::pkg::apis::meta::v1::OwnerReference, ByteString,
};
use kube::{
    api::{DeleteParams, ObjectMeta, Patch, PatchParams},
    runtime::{controller::Action, finalizer::finalizer, watcher::Config, Controller},
    Api, Client, Resource, ResourceExt as _,
};
use rand::{Rng, SeedableRng};
use tracing::{info, warn};
use uuid::Uuid;

use self::{cf_api::*, kube_api::*};
use crate::{cli::ControllerArgs, Error, Result};

const TUNNEL_SECRET_KEY: &str = "tunnel_secret";
const CFD_CONFIG_FILENAME: &str = "config.yml";
const PATCH_PARAMS_APPLY_NAME: &str = "cloudflaredtunnel.chalharu.top";
const CFD_DEPLOYMENT_IMAGE: &str = "cloudflare/cloudflared:2024.9.1";

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
        HttpApiClientConfig::default(),
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
    let ns = res.namespace().unwrap();
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
            .get_tunnel_opt(
                self.args.cloudflare_account_id().to_string(),
                tunnel_id.to_string(),
            )
            .await?;

        let zones = self.cloudflare_api.list_zone().await?;
        try_join_all(zones.iter().map(|z| async {
            let dns_records = self
                .cloudflare_api
                .list_dns_cname(z.id.clone(), tunnel_id.clone())
                .await?;
            for d in dns_records.into_iter() {
                self.cloudflare_api
                    .delete_dns_cname(d.zone_id, d.id)
                    .await?;
            }
            Result::<_, Error>::Ok(())
        }))
        .await?;

        if tunnel.is_some() {
            self.cloudflare_api
                .delete_tunnel(
                    self.args.cloudflare_account_id().to_string(),
                    tunnel_id.clone(),
                )
                .await?;
        }
        Ok(())
    }

    async fn reconcile(&self) -> Result<()> {
        let cfdt_list = get_cloudflaredtunnel(&self.client).await?;
        let account_id = self.args.cloudflare_account_id().to_string();
        let tunnel_list = self
            .cloudflare_api
            .list_tunnels(
                account_id.clone(),
                self.args.cloudflare_tunnel_prefix().to_string(),
            )
            .await?;
        let mut tunnel_dic_by_id = tunnel_list
            .into_iter()
            .map(|x| (x.id, x))
            .collect::<HashMap<_, _>>();

        for cfdt in cfdt_list {
            let tunnel = cfdt
                .status
                .as_ref()
                .and_then(|s| s.tunnel_id.as_ref())
                .and_then(|id| Uuid::parse_str(id).ok())
                .and_then(|id| tunnel_dic_by_id.remove(&id));
            self.reconcile_tunnel(cfdt, tunnel).await?;
        }

        for t in tunnel_dic_by_id {
            if t.1.name.starts_with(self.args.cloudflare_tunnel_prefix()) {
                if let Err(e) = self
                    .cloudflare_api
                    .delete_tunnel(
                        account_id.clone(),
                        t.0.as_hyphenated()
                            .encode_lower(&mut Uuid::encode_buffer())
                            .to_string(),
                    )
                    .await
                {
                    // tunnel削除の失敗は警告のみとする
                    warn!("Delete cloudflare tunnel failed: {}", e);
                }
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
                self.args.cloudflare_account_id().to_string(),
                tunnel_name.to_string(),
                tunnel_secret.to_owned(),
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
        let namespace = cfdt.namespace().unwrap();
        let uid = cfdt.uid().unwrap();
        let owner_ref = OwnerReference {
            api_version: CloudflaredTunnel::api_version(&()).to_string(),
            kind: CloudflaredTunnel::kind(&()).to_string(),
            name: name.clone(),
            uid,
            ..Default::default()
        };

        // DNS ZoneのリストをCloudflareから取得
        let zones = self.cloudflare_api.list_zone().await?;

        // CloudflaredTunnel.spec.ingress[].hostnameがどの　DNS Zoneに当てはまるか確認
        let mut dns_list = HashSet::new();
        for ingress in cfdt.spec.ingress.as_ref().iter().flat_map(|x| x.iter()) {
            let Some(zone_id) = zones
                .iter()
                .filter_map(|z| {
                    if ingress.hostname.ends_with(&format!(".{}", z.name)) {
                        Some(z.id.clone())
                    } else {
                        None
                    }
                })
                .next()
            else {
                // hostnameがzoneに当てはまらない場合
                return Err(Error::illegal_document());
            };
            dns_list.insert((ingress.hostname.clone(), zone_id));
        }

        // ZoneIDからDNSレコードを引く辞書を作成
        let zone_dns_list = try_join_all(zones.iter().map(|z| async {
            Result::<_, Error>::Ok(
                self.cloudflare_api
                    .list_dns(z.id.clone())
                    .await?
                    .into_iter()
                    .fold(
                        HashMap::new(),
                        |mut acc: HashMap<String, Vec<DnsRecord>>, value| {
                            acc.entry(value.zone_id.clone()).or_default().push(value);
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

        // {tunnelid}.cfargotunnel.comのCNAMEレコードリストを作成する
        let cname_content = format!("{tunnel_id}.cfargotunnel.com");
        let mut current_cname_list = zone_dns_list
            .iter()
            .flat_map(|(_, rec)| {
                rec.iter().flat_map(|rec| match rec.content {
                    DnsContent::CNAME { ref content } if content.as_str() == cname_content => {
                        Some((rec.id.clone(), rec.zone_id.clone()))
                    }
                    _ => None,
                })
            })
            .collect::<HashSet<_>>();

        // {tunnelid}.cfargotunnel.com以外のCNAMEレコード、Aレコード・AAAAレコードが無いことを確認する
        for (ref hostname, ref zone_id) in &dns_list {
            if let Some(dns_record) = zone_dns_list
                .get(zone_id)
                .ok_or_else(|| unreachable!())
                .and_then(|dns_records| {
                    dns_records
                        .iter()
                        .filter(|dns_record| dns_record.name.as_str() == hostname.as_str())
                        .try_fold(None, |acc, dns_record| match &dns_record.content {
                            DnsContent::CNAME { content } if content.as_str() == cname_content => {
                                Ok(Some(dns_record))
                            }
                            DnsContent::A { .. }
                            | DnsContent::AAAA { .. }
                            | DnsContent::CNAME { .. } => Err(Error::illegal_document()),
                            _ => Ok(acc),
                        })
                })?
            {
                current_cname_list.remove(&(dns_record.id.clone(), dns_record.zone_id.clone()));
            } else {
                self.cloudflare_api
                    .create_dns_cname(zone_id.clone(), tunnel_id.clone(), hostname.clone())
                    .await?;
            }
        }
        for (dns_id, zone_id) in current_cname_list {
            self.cloudflare_api
                .delete_dns_cname(zone_id, dns_id)
                .await?;
        }

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

        let secret_ref = match (spec_ref, status_ref) {
            (None, Some(st)) => st.to_string(),
            (Some(sp), Some(st)) if sp == st => st.to_string(),
            (sp, st) => {
                let secret_ref = if let Some(sp) = sp {
                    // もし自分自身が作成したリソースなら削除
                    if let Some(st) = st {
                        if let Some(secret) = api.get_opt(st.as_str()).await? {
                            if secret.owner_references().contains(&owner_ref) {
                                api.delete(&secret.name_any(), &DeleteParams::background())
                                    .await?;
                            }
                        }
                    }
                    sp.to_string()
                } else {
                    Uuid::new_v4()
                        .as_hyphenated()
                        .encode_lower(&mut Uuid::encode_buffer())
                        .to_string()
                };

                // statusに新しいsecret_refを設定
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
            let mut raw_data = vec![0u8; 32];
            tokio::task::spawn_blocking(rand::rngs::StdRng::from_entropy)
                .await?
                .try_fill(raw_data.as_mut_slice())?;
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

        if secret.len() < 32 {
            return Err(Error::illegal_document());
        };

        Ok(secret)
    }

    async fn get_tunnel_config(
        &self,
        cfdt: &CloudflaredTunnel,
        owner_ref: OwnerReference,
        tunnel: Tunnel,
        tunnel_secret: &Vec<u8>,
    ) -> Result<(String, bool)> {
        let tunnel_id = tunnel.id.as_hyphenated().to_string();
        let ns = cfdt.namespace().ok_or_else(Error::illegal_document)?;

        let credential = cfd_config::Credentials {
            account_tag: self.args.cloudflare_account_id().to_string(),
            tunnel_secret: base64::engine::general_purpose::STANDARD
                .encode(tunnel_secret.as_slice()),
            tunnel_id: tunnel_id.clone(),
        };
        let credential_filename = format!("{tunnel_id}.json");

        let credential_string = serde_json::to_string(&credential)?;
        let config = cfd_config::Config {
            tunnel: tunnel_id.clone(),
            credentials_file: Some(format!("/etc/cloudflared/{}", credential_filename)),
            origin_request: cfdt.spec.origin_request.as_ref().cloned().map(Into::into),
            ingress: cfdt
                .spec
                .ingress
                .as_ref()
                .iter()
                .flat_map(|x| x.iter().cloned().map(Into::into))
                .chain([cfd_config::Ingress {
                    hostname: None,
                    service: cfdt.spec.default_ingress_service.clone(),
                    path: None,
                    origin_request: None,
                }])
                .collect(),
        };
        let config_string = serde_yaml::to_string(&config)?;
        let secret_data = BTreeMap::from([
            (credential_filename, credential_string),
            (CFD_CONFIG_FILENAME.to_string(), config_string),
        ]);

        let config_ref = if let Some(config_ref) = cfdt
            .status
            .as_ref()
            .and_then(|s| s.config_secret_ref.as_ref())
        {
            config_ref.to_string()
        } else {
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
        };

        let secret_updated = patch_opaque_secret_string(
            &self.client,
            &config_ref,
            &ns,
            secret_data,
            Some(vec![owner_ref.clone()]),
        )
        .await?;

        Ok((config_ref, secret_updated))
    }
}
