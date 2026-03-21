//! Thin Cloudflare API wrapper used by the controller.

use std::sync::Arc;

use cloudflare::{
    endpoints::{
        cfd_tunnel::Tunnel,
        dns::dns::{DeleteDnsRecordResponse, DnsRecord},
        zones::zone::Zone,
    },
    framework::{client::async_api::Client as HttpApiClient, response::ApiFailure},
};
use tracing::info;

use crate::{Error, Result};

pub(super) fn tunnel_cname(tunnel_id: &str) -> String {
    format!("{tunnel_id}.cfargotunnel.com")
}

fn invalid_decode_response(error: &ApiFailure) -> bool {
    matches!(error, ApiFailure::Invalid(inner) if inner.is_decode())
}

pub struct CloudflareApi {
    api: Arc<HttpApiClient>,
}

impl CloudflareApi {
    pub fn new(api: Arc<HttpApiClient>) -> Self {
        Self { api }
    }

    pub async fn list_tunnels(&self, account_id: &str, prefix: &str) -> Result<Vec<Tunnel>> {
        use cloudflare::endpoints::cfd_tunnel::list_tunnels::{ListTunnels, Params};

        let endpoint = ListTunnels {
            params: Params {
                is_deleted: Some(false),
                include_prefix: Some(prefix.to_string()),
                ..Default::default()
            },
            account_identifier: account_id,
        };
        let response = self.api.request(&endpoint).await?;
        Ok(response.result)
    }

    pub(super) async fn get_tunnel_opt(
        &self,
        account_id: &str,
        tunnel_id: &str,
    ) -> Result<Option<Tunnel>> {
        use cloudflare::endpoints::cfd_tunnel::list_tunnels::{ListTunnels, Params};

        let endpoint = ListTunnels {
            params: Params {
                uuid: Some(tunnel_id.to_string()),
                is_deleted: Some(false),
                ..Default::default()
            },
            account_identifier: account_id,
        };
        let response = self.api.request(&endpoint).await?;
        Ok(response.result.into_iter().next())
    }

    pub(super) async fn create_tunnel(
        &self,
        account_id: &str,
        tunnel_name: &str,
        tunnel_secret: &[u8],
    ) -> Result<Tunnel> {
        use cloudflare::endpoints::cfd_tunnel::{
            ConfigurationSrc,
            create_tunnel::{CreateTunnel, Params},
        };

        info!("Create cloudflare tunnel: {tunnel_name}");
        let tunnel_secret = tunnel_secret.to_vec();

        let endpoint = CreateTunnel {
            account_identifier: account_id,
            params: Params {
                name: tunnel_name,
                tunnel_secret: &tunnel_secret,
                metadata: None,
                config_src: &ConfigurationSrc::Local,
            },
        };
        let response = self.api.request(&endpoint).await?;
        Ok(response.result)
    }

    pub(super) async fn delete_tunnel(&self, account_id: &str, tunnel_id: &str) -> Result<()> {
        use cloudflare::endpoints::cfd_tunnel::delete_tunnel::{DeleteTunnel, Params};

        info!("Delete cloudflare tunnel: {tunnel_id}");

        let endpoint = DeleteTunnel {
            account_identifier: account_id,
            tunnel_id,
            params: Params { cascade: false },
        };

        match self.api.request(&endpoint).await {
            Ok(_) => Ok(()),
            Err(error) if invalid_decode_response(&error) => Ok(()),
            Err(error) => Err(Error::from(error)),
        }
    }

    pub(super) async fn list_dns_cname(
        &self,
        zone_id: &str,
        tunnel_id: &str,
    ) -> Result<Vec<DnsRecord>> {
        use cloudflare::endpoints::dns::dns::{DnsContent, ListDnsRecords, ListDnsRecordsParams};

        let endpoint = ListDnsRecords {
            zone_identifier: zone_id,
            params: ListDnsRecordsParams {
                record_type: Some(DnsContent::CNAME {
                    content: tunnel_cname(tunnel_id),
                }),
                ..Default::default()
            },
        };

        let result = self.api.request(&endpoint).await?;
        Ok(result.result)
    }

    pub(super) async fn list_dns(&self, zone_id: &str) -> Result<Vec<DnsRecord>> {
        use cloudflare::endpoints::dns::dns::{ListDnsRecords, ListDnsRecordsParams};

        let endpoint = ListDnsRecords {
            zone_identifier: zone_id,
            params: ListDnsRecordsParams::default(),
        };

        let result = self.api.request(&endpoint).await?;
        Ok(result.result)
    }

    pub(super) async fn create_dns_cname(
        &self,
        zone_id: &str,
        tunnel_id: &str,
        hostname: &str,
    ) -> Result<DnsRecord> {
        use cloudflare::endpoints::dns::dns::{CreateDnsRecord, CreateDnsRecordParams, DnsContent};

        info!(
            "Create cloudflare dns cname record: {{ zone_id: {zone_id}, hostname: {hostname}, tunnel_id: {tunnel_id} }}"
        );

        let endpoint = CreateDnsRecord {
            zone_identifier: zone_id,
            params: CreateDnsRecordParams {
                name: hostname,
                content: DnsContent::CNAME {
                    content: tunnel_cname(tunnel_id),
                },
                proxied: Some(true),
                ttl: None,
                priority: None,
            },
        };
        let result = self.api.request(&endpoint).await?;
        Ok(result.result)
    }

    pub(super) async fn delete_dns_cname(
        &self,
        zone_id: &str,
        dns_record_id: &str,
    ) -> Result<DeleteDnsRecordResponse> {
        use cloudflare::endpoints::dns::dns::DeleteDnsRecord;

        info!(
            "Delete cloudflare dns cname record: {{ zone_id: {zone_id}, dns_record_id: {dns_record_id} }}"
        );
        let endpoint = DeleteDnsRecord {
            zone_identifier: zone_id,
            identifier: dns_record_id,
        };

        let result = self.api.request(&endpoint).await?;
        Ok(result.result)
    }

    pub(super) async fn list_zone(&self) -> Result<Vec<Zone>> {
        use cloudflare::endpoints::zones::zone::{ListZones, ListZonesParams};

        let endpoint = ListZones {
            params: ListZonesParams::default(),
        };

        let result = self.api.request(&endpoint).await?;
        Ok(result.result)
    }
}

#[cfg(test)]
mod tests {
    use cloudflare::framework::{
        Environment,
        auth::Credentials,
        client::{ClientConfig, async_api::Client as HttpApiClient},
    };
    use mockito::{Matcher, ServerGuard};

    use super::*;

    const ACCOUNT_ID: &str = "a0000000000000000000000000000001";
    const TUNNEL_ID: &str = "a0000000000000000000000000000002";
    const ZONE_ID: &str = "00000000000000000000000000000001";
    const DNS_RECORD_ID: &str = "00000000000000000000000000000002";

    async fn start_mock_server() -> ServerGuard {
        let mut server = mockito::Server::new_async().await;

        server
            .mock("GET", format!("/accounts/{ACCOUNT_ID}/cfd_tunnel").as_str())
            .match_query(Matcher::AllOf(vec![
                Matcher::UrlEncoded("is_deleted".into(), "false".into()),
                Matcher::UrlEncoded("include_prefix".into(), "test-prefix".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"result":[{"id":"00000000-0000-0000-0000-000000000001","created_at":"2000-01-01T00:00:00.000000Z","deleted_at":null,"name":"test-prefix-demo","connections":[],"metadata":{}}],"result_info":{},"success":true,"errors":[],"messages":[]}"#,
            )
            .create_async()
            .await;

        server
            .mock("GET", format!("/accounts/{ACCOUNT_ID}/cfd_tunnel").as_str())
            .match_query(Matcher::AllOf(vec![
                Matcher::UrlEncoded("is_deleted".into(), "false".into()),
                Matcher::UrlEncoded("uuid".into(), TUNNEL_ID.into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"result":[{"id":"00000000-0000-0000-0000-000000000001","created_at":"2000-01-01T00:00:00.000000Z","deleted_at":null,"name":"lookup-demo","connections":[],"metadata":{}}],"result_info":{},"success":true,"errors":[],"messages":[]}"#,
            )
            .create_async()
            .await;

        server
            .mock(
                "DELETE",
                format!("/accounts/{ACCOUNT_ID}/cfd_tunnel/{TUNNEL_ID}").as_str(),
            )
            .match_query(Matcher::UrlEncoded("cascade".into(), "false".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"result":{"id":"00000000000000000000000000000001"},"result_info":{},"success":true,"errors":[],"messages":[]}"#,
            )
            .create_async()
            .await;

        server
            .mock("GET", "/zones")
            .match_query(Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"result":[{"id":"00000000000000000000000000000001","name":"example.com","status":"active","paused":false,"type":"full","development_mode":0,"name_servers":[],"original_name_servers":[],"original_registrar":null,"original_dnshost":null,"modified_on":"2000-01-01T00:00:00.000000Z","created_on":"2000-01-01T00:00:00.000000Z","activated_on":"2000-01-01T00:00:00.000000Z","meta":{"step":0,"custom_certificate_quota":0,"page_rule_quota":0,"phishing_detected":false},"owner":{"id":null,"type":"user","email":null},"account":{"id":"","name":"Example account"},"tenant":{},"tenant_unit":{},"permissions":[],"plan":{"id":"","name":"","price":0,"currency":"","frequency":"","is_subscribed":false,"can_subscribe":false,"legacy_id":"","legacy_discount":false,"externally_managed":false}}],"result_info":{},"success":true,"errors":[],"messages":[]}"#,
            )
            .create_async()
            .await;

        server
            .mock("POST", format!("/zones/{ZONE_ID}/dns_records").as_str())
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"result":{"id":"a0000000000000000000000000000001","zone_id":"00000000000000000000000000000001","zone_name":"example.com","name":"example.example.com","type":"CNAME","content":"example.com","proxiable":true,"proxied":true,"ttl":1,"settings":{},"meta":{"auto_added":false,"managed_by_apps":false,"managed_by_argo_tunnel":false},"comment":null,"tags":[],"created_on":"2000-01-01T00:00:00.000000Z","modified_on":"2000-01-01T00:00:00.000000Z"},"result_info":{},"success":true,"errors":[],"messages":[]}"#,
            )
            .create_async()
            .await;

        server
            .mock("POST", format!("/accounts/{ACCOUNT_ID}/cfd_tunnel").as_str())
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"result":{"id":"00000000-0000-0000-0000-000000000001","created_at":"2000-01-01T00:00:00.000000Z","deleted_at":null,"name":"example-tunnel","connections":[],"metadata":{}},"result_info":{},"success":true,"errors":[],"messages":[]}"#,
            )
            .create_async()
            .await;

        server
            .mock("GET", format!("/zones/{ZONE_ID}/dns_records").as_str())
            .match_query(Matcher::AllOf(vec![
                Matcher::UrlEncoded("type".into(), "CNAME".into()),
                Matcher::UrlEncoded("content".into(), tunnel_cname(TUNNEL_ID)),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"result":[{"id":"record-1","zone_id":"00000000000000000000000000000001","zone_name":"example.com","name":"example.example.com","type":"CNAME","content":"a0000000000000000000000000000002.cfargotunnel.com","proxiable":true,"proxied":true,"ttl":1,"settings":{},"meta":{"auto_added":false,"managed_by_apps":false,"managed_by_argo_tunnel":false},"comment":null,"tags":[],"created_on":"2000-01-01T00:00:00.000000Z","modified_on":"2000-01-01T00:00:00.000000Z"}],"result_info":{},"success":true,"errors":[],"messages":[]}"#,
            )
            .create_async()
            .await;

        server
            .mock("GET", format!("/zones/{ZONE_ID}/dns_records").as_str())
            .match_query(Matcher::Missing)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"result":[{"id":"record-2","zone_id":"00000000000000000000000000000001","zone_name":"example.com","name":"example.example.com","type":"CNAME","content":"a0000000000000000000000000000002.cfargotunnel.com","proxiable":true,"proxied":true,"ttl":1,"settings":{},"meta":{"auto_added":false,"managed_by_apps":false,"managed_by_argo_tunnel":false},"comment":null,"tags":[],"created_on":"2000-01-01T00:00:00.000000Z","modified_on":"2000-01-01T00:00:00.000000Z"}],"result_info":{},"success":true,"errors":[],"messages":[]}"#,
            )
            .create_async()
            .await;

        server
            .mock(
                "DELETE",
                format!("/zones/{ZONE_ID}/dns_records/{DNS_RECORD_ID}").as_str(),
            )
            .with_status(201)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"result":{"id":"00000000000000000000000000000001"},"result_info":{},"success":true,"errors":[],"messages":[]}"#,
            )
            .create_async()
            .await;

        server
    }

    async fn create_api(url: &str) -> CloudflareApi {
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

    #[test]
    fn tunnel_cname_helper_formats_expected_hostname() {
        assert_eq!(
            tunnel_cname("a0000000000000000000000000000002"),
            "a0000000000000000000000000000002.cfargotunnel.com"
        );
    }

    #[tokio::test]
    async fn list_tunnels_returns_matching_tunnels() {
        let server = start_mock_server().await;
        let api = create_api(server.url().as_str()).await;

        let response = api
            .list_tunnels(ACCOUNT_ID, "test-prefix")
            .await
            .expect("list_tunnels should succeed");

        assert_eq!(response.len(), 1);
        assert_eq!(response[0].name, "test-prefix-demo");
    }

    #[tokio::test]
    async fn get_tunnel_opt_returns_first_match() {
        let server = start_mock_server().await;
        let api = create_api(server.url().as_str()).await;

        let response = api
            .get_tunnel_opt(ACCOUNT_ID, TUNNEL_ID)
            .await
            .expect("get_tunnel_opt should succeed");

        assert_eq!(response.expect("tunnel should exist").name, "lookup-demo");
    }

    #[tokio::test]
    async fn create_tunnel_returns_created_tunnel() {
        let server = start_mock_server().await;
        let api = create_api(server.url().as_str()).await;

        let response = api
            .create_tunnel(ACCOUNT_ID, "tunnel-name", b"tunnel-secret")
            .await
            .expect("create_tunnel should succeed");

        assert_eq!(response.name, "example-tunnel");
    }

    #[tokio::test]
    async fn delete_tunnel_succeeds_for_standard_response() {
        let server = start_mock_server().await;
        let api = create_api(server.url().as_str()).await;

        api.delete_tunnel(ACCOUNT_ID, TUNNEL_ID)
            .await
            .expect("delete_tunnel should succeed");
    }

    #[tokio::test]
    async fn delete_tunnel_treats_decode_failures_as_success() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock(
                "DELETE",
                format!("/accounts/{ACCOUNT_ID}/cfd_tunnel/{TUNNEL_ID}").as_str(),
            )
            .match_query(Matcher::UrlEncoded("cascade".into(), "false".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("not-json")
            .create_async()
            .await;

        let api = create_api(server.url().as_str()).await;

        api.delete_tunnel(ACCOUNT_ID, TUNNEL_ID)
            .await
            .expect("decode failures should be ignored");
    }

    #[tokio::test]
    async fn list_dns_cname_returns_matching_records() {
        let server = start_mock_server().await;
        let api = create_api(server.url().as_str()).await;

        let response = api
            .list_dns_cname(ZONE_ID, TUNNEL_ID)
            .await
            .expect("list_dns_cname should succeed");

        assert_eq!(response.len(), 1);
        assert_eq!(response[0].name, "example.example.com");
    }

    #[tokio::test]
    async fn list_dns_returns_zone_records() {
        let server = start_mock_server().await;
        let api = create_api(server.url().as_str()).await;

        let response = api
            .list_dns(ZONE_ID)
            .await
            .expect("list_dns should succeed");

        assert_eq!(response.len(), 1);
        assert_eq!(response[0].id, "record-2");
    }

    #[tokio::test]
    async fn create_dns_cname_returns_created_record() {
        let server = start_mock_server().await;
        let api = create_api(server.url().as_str()).await;

        let response = api
            .create_dns_cname(ZONE_ID, TUNNEL_ID, "example.example.com")
            .await
            .expect("create_dns_cname should succeed");

        assert_eq!(response.name, "example.example.com");
    }

    #[tokio::test]
    async fn delete_dns_cname_returns_response_payload() {
        let server = start_mock_server().await;
        let api = create_api(server.url().as_str()).await;

        let response = api
            .delete_dns_cname(ZONE_ID, DNS_RECORD_ID)
            .await
            .expect("delete_dns_cname should succeed");

        assert_eq!(response.id, "00000000000000000000000000000001");
    }

    #[tokio::test]
    async fn list_zone_returns_available_zones() {
        let server = start_mock_server().await;
        let api = create_api(server.url().as_str()).await;

        let zones = api.list_zone().await.expect("list_zone should succeed");

        assert_eq!(zones.len(), 1);
        assert_eq!(zones[0].name, "example.com");
    }
}
