use serde::{Deserialize, Serialize};

use super::customresource::{CloudflaredTunnelIngress, CloudflaredTunnelOriginRequest};

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct Credentials {
    #[serde(rename = "AccountTag")]
    pub account_tag: String,
    #[serde(rename = "TunnelSecret")]
    pub tunnel_secret: String, // base64 encoded
    #[serde(rename = "TunnelID")]
    pub tunnel_id: String,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct Config {
    pub tunnel: String,
    #[serde(rename = "credentials-file", skip_serializing_if = "Option::is_none")]
    pub credentials_file: Option<String>,
    #[serde(rename = "originRequest", skip_serializing_if = "Option::is_none")]
    pub origin_request: Option<OriginRequest>,
    #[serde(rename = "ingress")]
    pub ingress: Vec<Ingress>,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
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
    // #[serde(rename = "access")]
    // pub access: Option<CloudflaredTunnelAccess>
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
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
