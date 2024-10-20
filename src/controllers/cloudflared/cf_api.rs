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

#[cfg(test)]
mod test {
    use cloudflare::framework::{
        async_api::Client as HttpApiClient, auth::Credentials, Environment, HttpApiClientConfig,
    };
    use mockito::{Matcher, ServerGuard};

    use super::*;

    async fn start_mock_server() -> ServerGuard {
        let mut server = mockito::Server::new_async().await;

        // list_tunnel or get_tunnel_opt
        server
            .mock(
                "GET",
                "/accounts/a0000000000000000000000000000001/cfd_tunnel",
            )
            .match_query(Matcher::AllOf(vec![
                Matcher::UrlEncoded("is_deleted".into(), "false".into()),
                Matcher::AnyOf(vec![
                    Matcher::UrlEncoded("include_prefix".into(), "test-prefix".into()),
                    Matcher::UrlEncoded("uuid".into(), "a0000000000000000000000000000002".into()),
                ]),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"result":[],"result_info":{},"success":true,"errors":[],"messages":[]}"#)
            .create_async()
            .await;

        // delete dns record
        server
            .mock("DELETE", "/zones/00000000000000000000000000000001/dns_records/00000000000000000000000000000002")
            .with_status(201)
            .with_header("content-type", "application/json")
            .with_body(r#"{"result":{"id":"00000000000000000000000000000001"},"result_info":{},"success":true,"errors":[],"messages":[]}"#)
            .create_async()
            .await;

        // list zones
        server
            .mock("GET", "/zones?")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"result":[
                {"id":"00000000000000000000000000000001","name":"example.com","status":"active","paused":false,"type":"full","development_mode":0,"name_servers":[],"original_name_servers":[],"original_registrar":null,"original_dnshost":null,"modified_on":"2000-01-01T00:00:00.000000Z","created_on":"2000-01-01T00:00:00.000000Z","activated_on":"2000-01-01T00:00:00.000000Z","meta":{"step":0,"custom_certificate_quota":0,"page_rule_quota":0,"phishing_detected":false},"owner":{"id":null,"type":"user","email":null},"account":{"id":"","name":"Example account"},"tenant":{},"tenant_unit":{},"permissions":[],"plan":{"id":"","name":"","price":0,"currency":"","frequency":"","is_subscribed":false,"can_subscribe":false,"legacy_id":"","legacy_discount":false,"externally_managed":false}}
            ],"result_info":{},"success":true,"errors":[],"messages":[]}"#)
            .create_async()
            .await;

        // delete tunnel
        server
            .mock("DELETE", "/accounts/a0000000000000000000000000000001/cfd_tunnel/a0000000000000000000000000000002")
            .match_query(
                    Matcher::UrlEncoded("cascade".into(), "false".into()),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"result":{"id":"00000000000000000000000000000001"},"result_info":{},"success":true,"errors":[],"messages":[]}"#)
            .create_async()
            .await;

        // create dns recoad
        server
            .mock(
                "POST",
                "/zones/00000000000000000000000000000001/dns_records",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"result":{"id":"a0000000000000000000000000000001","zone_id":"00000000000000000000000000000001","zone_name":"example.com","name":"example.example.com","type":"CNAME","content":"example.com","proxiable":true,"proxied":true,"ttl":1,"settings":{},"meta":{"auto_added":false,"managed_by_apps":false,"managed_by_argo_tunnel":false},"comment":null,"tags":[],"created_on":"2000-01-01T00:00:00.000000Z","modified_on":"2000-01-01T00:00:00.000000Z"},"result_info":{},"success":true,"errors":[],"messages":[]}"#)
            .create_async()
            .await;

        // create tunnel
        server
            .mock(
                "POST",
                "/accounts/a0000000000000000000000000000001/cfd_tunnel",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"result":{"id":"00000000000000000000000000000001","created_at":"2000-01-01T00:00:00.000000Z","deleted_at":null,"name":"example-tunnel","connections":[],"metadata":{}},"result_info":{},"success":true,"errors":[],"messages":[]}"#)
            .create_async()
            .await;

        // list dns records
        server
            .mock("GET", "/zones/00000000000000000000000000000001/dns_records")
            .match_query(Matcher::AnyOf(vec![
                Matcher::Missing,
                Matcher::AllOf(vec![
                    Matcher::UrlEncoded("type".into(), "CNAME".into()),
                    Matcher::UrlEncoded(
                        "content".into(),
                        "a0000000000000000000000000000002.cfargotunnel.com".into(),
                    ),
                ]),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"result":[],"result_info":{},"success":true,"errors":[],"messages":[]}"#)
            .create_async()
            .await;

        server
    }

    async fn create_api_client(url: &str) -> HttpApiClient {
        let api = HttpApiClient::new(
            Credentials::UserAuthToken {
                token: "DEADBEAF".to_string(),
            },
            HttpApiClientConfig::default(),
            Environment::Custom(url::Url::parse(&url).unwrap()),
        )
        .unwrap();

        api
    }

    #[tokio::test]
    async fn list_tunnels() {
        let _ = env_logger::try_init();
        let server = start_mock_server().await;
        let api = create_api_client(server.url().as_str()).await;
        let api = CloudflareApi::new(Arc::new(api));

        let _response = api
            .list_tunnels(
                "a0000000000000000000000000000001".to_string(),
                "test-prefix".to_string(),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn get_tunnel_opt() {
        let _ = env_logger::try_init();
        let server = start_mock_server().await;
        let api = create_api_client(server.url().as_str()).await;
        let api = CloudflareApi::new(Arc::new(api));
        let _response = api
            .get_tunnel_opt(
                "a0000000000000000000000000000001".to_string(),
                "a0000000000000000000000000000002".to_string(),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn create_tunnel() {
        let _ = env_logger::try_init();
        let server = start_mock_server().await;
        let api = create_api_client(server.url().as_str()).await;
        let api = CloudflareApi::new(Arc::new(api));
        let _response = api
            .create_tunnel(
                "a0000000000000000000000000000001".to_string(),
                "tunnel-name".to_string(),
                "tunnel-secret".as_bytes().to_vec(),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn delete_tunnel() {
        let _ = env_logger::try_init();
        let server = start_mock_server().await;
        let api = create_api_client(server.url().as_str()).await;
        let api = CloudflareApi::new(Arc::new(api));
        let _response = api
            .delete_tunnel(
                "a0000000000000000000000000000001".to_string(),
                "a0000000000000000000000000000002".to_string(),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn list_dns_cname() {
        let _ = env_logger::try_init();
        let server = start_mock_server().await;
        let api = create_api_client(server.url().as_str()).await;
        let api = CloudflareApi::new(Arc::new(api));
        let _response = api
            .list_dns_cname(
                "00000000000000000000000000000001".to_string(),
                "a0000000000000000000000000000002".to_string(),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn list_dns() {
        let _ = env_logger::try_init();
        let server = start_mock_server().await;
        let api = create_api_client(server.url().as_str()).await;
        let api = CloudflareApi::new(Arc::new(api));
        let _response = api
            .list_dns("00000000000000000000000000000001".to_string())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn create_dns_cname() {
        let _ = env_logger::try_init();
        let server = start_mock_server().await;
        let api = create_api_client(server.url().as_str()).await;
        let api = CloudflareApi::new(Arc::new(api));

        let _response = api
            .create_dns_cname(
                "00000000000000000000000000000001".to_string(),
                "a0000000000000000000000000000002".to_string(),
                "example.example.com".to_string(),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn delete_dns_cname() {
        let _ = env_logger::try_init();
        let server = start_mock_server().await;
        let api = create_api_client(server.url().as_str()).await;
        let api = CloudflareApi::new(Arc::new(api));

        let _response = api
            .delete_dns_cname(
                "00000000000000000000000000000001".to_string(),
                "00000000000000000000000000000002".to_string(),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn list_zone() {
        let _ = env_logger::try_init();
        let server = start_mock_server().await;
        let api = create_api_client(server.url().as_str()).await;

        let api = CloudflareApi::new(Arc::new(api));

        let zone = api.list_zone().await.unwrap();
        assert_eq!(1, zone.len());
        assert_eq!("example.com", zone.first().unwrap().name);
    }
}
