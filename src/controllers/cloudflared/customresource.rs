use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(CustomResource, Debug, PartialEq, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[kube(
    // Required properties
    group = "chalharu.top",
    version = "v1alpha1",
    kind = "CloudflaredTunnel",
    // Optional properties
    singular = "cloudflaredtunnel",
    plural = "cloudflaredtunnels",
    shortname = "cfdt",
    status = "CloudflaredTunnelStatus",
    namespaced,
)]
pub struct CloudflaredTunnelSpec {
    pub origin_request: Option<CloudflaredTunnelOriginRequest>,
    pub ingress: Option<Vec<CloudflaredTunnelIngress>>,
    pub secret_ref: Option<String>,
    pub image: Option<String>,
    pub args: Option<Vec<String>>,
    pub command: Option<Vec<String>>,
    pub default_ingress_service: String,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CloudflaredTunnelIngress {
    pub hostname: String,
    pub service: String,
    pub path: Option<String>,
    pub origin_request: Option<CloudflaredTunnelOriginRequest>,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CloudflaredTunnelOriginRequest {
    pub origin_server_name: Option<String>,
    pub ca_pool: Option<String>,
    pub no_tls_verify: Option<bool>,
    pub tls_timeout: Option<String>,
    pub http2_origin: Option<bool>,
    pub http_host_header: Option<String>,
    pub disable_chunked_encoding: Option<bool>,
    pub connect_timeout: Option<String>,
    pub no_happy_eyeballs: Option<bool>,
    pub proxy_type: Option<String>,
    pub proxy_address: Option<String>,
    pub proxy_port: Option<u16>,
    pub keep_alive_timeout: Option<String>,
    pub keep_alive_connections: Option<u32>,
    pub tcp_keep_alive: Option<String>,
    // pub access: Option<CloudflaredTunnelAccess>
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CloudflaredTunnelStatus {
    pub tunnel_id: Option<String>,
    pub config_secret_ref: Option<String>,
    pub tunnel_secret_ref: Option<String>,
}
