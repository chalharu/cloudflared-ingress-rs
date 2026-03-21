//! Cloudflared configuration models rendered into Kubernetes Secrets.

use serde::{Deserialize, Serialize};

use super::customresource::{
    CloudflaredTunnelAccess, CloudflaredTunnelIngress, CloudflaredTunnelOriginRequest,
};

/// Cloudflared tunnel credentials stored alongside the rendered YAML config.
#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
pub struct Credentials {
    #[serde(rename = "AccountTag")]
    pub account_tag: String,
    #[serde(rename = "TunnelSecret")]
    pub tunnel_secret: String,
    #[serde(rename = "TunnelID")]
    pub tunnel_id: String,
}

/// Top-level `config.yml` content for the `cloudflared` sidecar.
#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
pub struct Config {
    pub tunnel: String,
    #[serde(rename = "credentials-file", skip_serializing_if = "Option::is_none")]
    pub credentials_file: Option<String>,
    #[serde(rename = "originRequest", skip_serializing_if = "Option::is_none")]
    pub origin_request: Option<OriginRequest>,
    #[serde(rename = "ingress")]
    pub ingress: Vec<Ingress>,
}

/// Request-level tuning options understood by `cloudflared`.
#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize, Default)]
pub struct OriginRequest {
    #[serde(rename = "originServerName", skip_serializing_if = "Option::is_none")]
    pub origin_server_name: Option<String>,
    #[serde(rename = "caPool", skip_serializing_if = "Option::is_none")]
    pub ca_pool: Option<String>,
    #[serde(rename = "noTLSVerify", skip_serializing_if = "Option::is_none")]
    pub no_tls_verify: Option<bool>,
    #[serde(rename = "tlsTimeout", skip_serializing_if = "Option::is_none")]
    pub tls_timeout: Option<String>,
    #[serde(rename = "http2Origin", skip_serializing_if = "Option::is_none")]
    pub http2_origin: Option<bool>,
    #[serde(rename = "httpHostHeader", skip_serializing_if = "Option::is_none")]
    pub http_host_header: Option<String>,
    #[serde(
        rename = "disableChunkedEncoding",
        skip_serializing_if = "Option::is_none"
    )]
    pub disable_chunked_encoding: Option<bool>,
    #[serde(rename = "connectTimeout", skip_serializing_if = "Option::is_none")]
    pub connect_timeout: Option<String>,
    #[serde(rename = "noHappyEyeballs", skip_serializing_if = "Option::is_none")]
    pub no_happy_eyeballs: Option<bool>,
    #[serde(rename = "proxyType", skip_serializing_if = "Option::is_none")]
    pub proxy_type: Option<String>,
    #[serde(rename = "proxyAddress", skip_serializing_if = "Option::is_none")]
    pub proxy_address: Option<String>,
    #[serde(rename = "proxyPort", skip_serializing_if = "Option::is_none")]
    pub proxy_port: Option<u16>,
    #[serde(rename = "keepAliveTimeout", skip_serializing_if = "Option::is_none")]
    pub keep_alive_timeout: Option<String>,
    #[serde(
        rename = "keepAliveConnections",
        skip_serializing_if = "Option::is_none"
    )]
    pub keep_alive_connections: Option<u32>,
    #[serde(rename = "tcpKeepAlive", skip_serializing_if = "Option::is_none")]
    pub tcp_keep_alive: Option<String>,
    #[serde(rename = "access", skip_serializing_if = "Option::is_none")]
    pub access: Option<Access>,
}

/// A single ingress rule in the rendered Cloudflared config.
#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
pub struct Ingress {
    #[serde(rename = "hostname", skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(rename = "service")]
    pub service: String,
    #[serde(rename = "path", skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(rename = "originRequest", skip_serializing_if = "Option::is_none")]
    pub origin_request: Option<OriginRequest>,
}

/// Cloudflare Access requirements for a rendered ingress.
#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
pub struct Access {
    #[serde(rename = "required")]
    pub required: bool,
    #[serde(rename = "teamName")]
    pub team_name: String,
    #[serde(rename = "audTag", skip_serializing_if = "Vec::is_empty")]
    pub aud_tag: Vec<String>,
}

impl From<CloudflaredTunnelOriginRequest> for OriginRequest {
    fn from(value: CloudflaredTunnelOriginRequest) -> Self {
        Self {
            origin_server_name: value.origin_server_name,
            ca_pool: value.ca_pool,
            no_tls_verify: value.no_tls_verify,
            tls_timeout: value.tls_timeout,
            http2_origin: value.http2_origin,
            http_host_header: value.http_host_header,
            disable_chunked_encoding: value.disable_chunked_encoding,
            connect_timeout: value.connect_timeout,
            no_happy_eyeballs: value.no_happy_eyeballs,
            proxy_type: value.proxy_type,
            proxy_address: value.proxy_address,
            proxy_port: value.proxy_port,
            keep_alive_timeout: value.keep_alive_timeout,
            keep_alive_connections: value.keep_alive_connections,
            tcp_keep_alive: value.tcp_keep_alive,
            access: value.access.map(Into::into),
        }
    }
}

impl From<CloudflaredTunnelIngress> for Ingress {
    fn from(value: CloudflaredTunnelIngress) -> Self {
        Self {
            hostname: Some(value.hostname),
            service: value.service,
            path: value.path,
            origin_request: value.origin_request.map(Into::into),
        }
    }
}

impl From<CloudflaredTunnelAccess> for Access {
    fn from(value: CloudflaredTunnelAccess) -> Self {
        Self {
            required: value.required,
            team_name: value.team_name,
            aud_tag: value.aud_tag,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_serialize_with_cloudflared_field_names() {
        let credentials = Credentials {
            account_tag: "account".to_string(),
            tunnel_secret: "secret".to_string(),
            tunnel_id: "tunnel-id".to_string(),
        };

        let rendered = serde_json::to_value(credentials).expect("credentials should serialize");

        assert_eq!(rendered["AccountTag"], "account");
        assert_eq!(rendered["TunnelSecret"], "secret");
        assert_eq!(rendered["TunnelID"], "tunnel-id");
    }

    #[test]
    fn origin_request_conversion_preserves_access_configuration() {
        let origin_request = CloudflaredTunnelOriginRequest {
            no_tls_verify: Some(true),
            http2_origin: Some(true),
            access: Some(CloudflaredTunnelAccess {
                required: true,
                team_name: "team".to_string(),
                aud_tag: vec!["aud".to_string()],
            }),
            ..Default::default()
        };

        let converted = OriginRequest::from(origin_request);

        assert_eq!(converted.no_tls_verify, Some(true));
        assert_eq!(converted.http2_origin, Some(true));
        assert_eq!(
            converted.access,
            Some(Access {
                required: true,
                team_name: "team".to_string(),
                aud_tag: vec!["aud".to_string()],
            })
        );
    }

    #[test]
    fn ingress_conversion_preserves_hostname_service_and_path() {
        let ingress = CloudflaredTunnelIngress {
            hostname: "example.com".to_string(),
            service: "https://service.example.com".to_string(),
            path: Some("^/api".to_string()),
            origin_request: Some(CloudflaredTunnelOriginRequest {
                no_tls_verify: Some(true),
                ..Default::default()
            }),
        };

        let converted = Ingress::from(ingress);

        assert_eq!(converted.hostname.as_deref(), Some("example.com"));
        assert_eq!(converted.service, "https://service.example.com");
        assert_eq!(converted.path.as_deref(), Some("^/api"));
        assert_eq!(
            converted
                .origin_request
                .as_ref()
                .and_then(|origin_request| origin_request.no_tls_verify),
            Some(true)
        );
    }

    #[test]
    fn config_serialization_omits_empty_optional_fields() {
        let config = Config {
            tunnel: "tunnel-id".to_string(),
            credentials_file: None,
            origin_request: Some(OriginRequest {
                no_tls_verify: Some(true),
                ..Default::default()
            }),
            ingress: vec![Ingress {
                hostname: Some("example.com".to_string()),
                service: "https://service.default.svc".to_string(),
                path: None,
                origin_request: Some(OriginRequest {
                    access: Some(Access {
                        required: true,
                        team_name: "team".to_string(),
                        aud_tag: Vec::new(),
                    }),
                    no_tls_verify: Some(true),
                    ..Default::default()
                }),
            }],
        };

        let rendered = serde_yaml::to_string(&config).expect("config should serialize");

        assert!(rendered.contains("originRequest:"));
        assert!(rendered.contains("noTLSVerify: true"));
        assert!(!rendered.contains("credentials-file:"));
        assert!(!rendered.contains("tcpKeepAlive:"));
        assert!(!rendered.contains("audTag:"));
    }
}
