use std::sync::Arc;

use cloudflare::{
    endpoints::{
        cfd_tunnel::Tunnel,
        dns::{DeleteDnsRecordResponse, DnsRecord},
        zone::Zone,
    },
    framework::{async_api::Client as HttpApiClient, response::ApiFailure},
};
use tracing::info;

use crate::{Error, Result};

pub struct CloudflareApi {
    api: Arc<HttpApiClient>,
}

impl CloudflareApi {
    pub fn new(api: Arc<HttpApiClient>) -> Self {
        Self { api }
    }

    pub async fn list_tunnels(&self, account_id: String, prefix: String) -> Result<Vec<Tunnel>> {
        use cloudflare::endpoints::cfd_tunnel::list_tunnels::{ListTunnels, Params};
        let api = self.api.clone();

        let endpoint = ListTunnels {
            params: Params {
                is_deleted: Some(false),
                include_prefix: Some(prefix),
                ..Default::default()
            },
            account_identifier: account_id.as_str(),
        };
        let response = api.request(&endpoint).await?;
        Ok(response.result)
    }

    pub(super) async fn get_tunnel_opt(
        &self,
        account_id: String,
        tunnel_id: String,
    ) -> Result<Option<Tunnel>> {
        use cloudflare::endpoints::cfd_tunnel::list_tunnels::{ListTunnels, Params};
        let api = self.api.clone();

        let endpoint = ListTunnels {
            params: Params {
                uuid: Some(tunnel_id),
                is_deleted: Some(false),
                ..Default::default()
            },
            account_identifier: account_id.as_str(),
        };
        let response = api.request(&endpoint).await?;
        Ok(response.result.into_iter().next())
    }

    pub(super) async fn create_tunnel(
        &self,
        account_id: String,
        tunnel_name: String,
        tunnel_secret: Vec<u8>,
    ) -> Result<Tunnel> {
        use cloudflare::endpoints::cfd_tunnel::{
            create_tunnel::{CreateTunnel, Params},
            ConfigurationSrc,
        };
        let api = self.api.clone();
        info!("Create cloudflare tunnel: {}", tunnel_name);

        let endpoint = CreateTunnel {
            account_identifier: account_id.as_str(),
            params: Params {
                name: tunnel_name.as_str(),
                tunnel_secret: &tunnel_secret,
                metadata: None,
                config_src: &ConfigurationSrc::Local,
            },
        };
        let response = api.request(&endpoint).await?;
        Ok(response.result)
    }

    pub(super) async fn delete_tunnel(&self, account_id: String, tunnel_id: String) -> Result<()> {
        use cloudflare::endpoints::cfd_tunnel::delete_tunnel::{DeleteTunnel, Params};
        let api = self.api.clone();

        info!("Delete cloudflare tunnel: {}", tunnel_id);

        let endpoint = DeleteTunnel {
            account_identifier: account_id.as_str(),
            tunnel_id: &tunnel_id,
            params: Params { cascade: false },
        };

        api.request(&endpoint).await.map_or_else(
            |e| match e {
                // Tunnelが削除済みであった場合、Decode errorが発生する
                ApiFailure::Invalid(inner) if inner.is_decode() => Ok(()),
                _ => Err(Error::from(e)),
            },
            |_| Ok(()),
        )
    }

    pub(super) async fn list_dns_cname(
        &self,
        zone_id: String,
        tunnel_id: String,
    ) -> Result<Vec<DnsRecord>> {
        use cloudflare::endpoints::dns::{DnsContent, ListDnsRecords, ListDnsRecordsParams};
        let api = self.api.clone();
        let endpoint = ListDnsRecords {
            zone_identifier: zone_id.as_str(),
            params: ListDnsRecordsParams {
                record_type: Some(DnsContent::CNAME {
                    content: format!("{}.cfargotunnel.com", tunnel_id),
                }),
                ..Default::default()
            },
        };

        let result = api.request(&endpoint).await?;

        Ok(result.result)
    }

    pub(super) async fn list_dns(&self, zone_id: String) -> Result<Vec<DnsRecord>> {
        use cloudflare::endpoints::dns::{ListDnsRecords, ListDnsRecordsParams};
        let api = self.api.clone();

        let endpoint = ListDnsRecords {
            zone_identifier: zone_id.as_str(),
            params: ListDnsRecordsParams::default(),
        };

        let result = api.request(&endpoint).await?;

        Ok(result.result)
    }

    pub(super) async fn create_dns_cname(
        &self,
        zone_id: String,
        tunnel_id: String,
        target: String,
    ) -> Result<DnsRecord> {
        use cloudflare::endpoints::dns::{CreateDnsRecord, CreateDnsRecordParams, DnsContent};
        let api = self.api.clone();
        info!(
            "Create cloudflare dns cname record: {{ zone_id: {} , tunnel_id: {}, tunnel_id: {}}}",
            zone_id, target, tunnel_id
        );

        let endpoint = CreateDnsRecord {
            zone_identifier: zone_id.as_str(),
            params: CreateDnsRecordParams {
                name: target.as_str(),
                content: DnsContent::CNAME {
                    content: format!("{}.cfargotunnel.com", tunnel_id),
                },
                proxied: Some(true),
                ttl: None,
                priority: None,
            },
        };
        let result = api.request(&endpoint).await?;

        Ok(result.result)
    }

    pub(super) async fn delete_dns_cname(
        &self,
        zone_id: String,
        dns_record_id: String,
    ) -> Result<DeleteDnsRecordResponse> {
        use cloudflare::endpoints::dns::DeleteDnsRecord;
        let api = self.api.clone();
        info!(
            "Delete cloudflare dns cname record: {{ zone_id: {} , dns_record_id: {}}}",
            zone_id, dns_record_id
        );
        let endpoint = DeleteDnsRecord {
            zone_identifier: zone_id.as_str(),
            identifier: dns_record_id.as_str(),
        };

        let result = api.request(&endpoint).await?;

        Ok(result.result)
    }

    pub(super) async fn list_zone(&self) -> Result<Vec<Zone>> {
        use cloudflare::endpoints::zone::{ListZones, ListZonesParams};
        let api = self.api.clone();
        let endpoint = ListZones {
            params: ListZonesParams::default(),
        };

        let result = api.request(&endpoint).await?;

        Ok(result.result)
    }
}
