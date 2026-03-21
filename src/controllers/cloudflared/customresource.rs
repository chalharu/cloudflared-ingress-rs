//! Custom resource definitions for `CloudflaredTunnel`.

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Desired state for a Cloudflare tunnel managed by this controller.
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

/// A single hostname/path rule routed through the tunnel.
#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CloudflaredTunnelIngress {
    pub hostname: String,
    pub service: String,
    pub path: Option<String>,
    pub origin_request: Option<CloudflaredTunnelOriginRequest>,
}

/// Request-level Cloudflared options exposed by the CRD.
#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize, JsonSchema, Default)]
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
    pub access: Option<CloudflaredTunnelAccess>,
}

/// Controller-managed status fields persisted on the CRD.
#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CloudflaredTunnelStatus {
    pub tunnel_id: Option<String>,
    pub config_secret_ref: Option<String>,
    pub tunnel_secret_ref: Option<String>,
}

/// Cloudflare Access requirements for a tunnel ingress.
#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CloudflaredTunnelAccess {
    pub required: bool,
    pub team_name: String,
    pub aud_tag: Vec<String>,
}

#[cfg(test)]
mod tests {
    use kube::CustomResourceExt as _;

    use super::*;

    #[test]
    fn defaults_leave_optional_fields_empty() {
        let spec = CloudflaredTunnelSpec::default();
        let status = CloudflaredTunnelStatus::default();

        assert_eq!(spec.origin_request, None);
        assert_eq!(spec.ingress, None);
        assert_eq!(spec.secret_ref, None);
        assert_eq!(spec.image, None);
        assert_eq!(spec.args, None);
        assert_eq!(spec.command, None);
        assert!(spec.default_ingress_service.is_empty());
        assert_eq!(status.tunnel_id, None);
        assert_eq!(status.config_secret_ref, None);
        assert_eq!(status.tunnel_secret_ref, None);
    }

    #[test]
    fn crd_metadata_matches_expected_names() {
        let crd = CloudflaredTunnel::crd();
        let expected_short_names = ["cfdt".to_string()];

        assert_eq!(
            crd.metadata.name.as_deref(),
            Some("cloudflaredtunnels.chalharu.top")
        );
        assert_eq!(crd.spec.group, "chalharu.top");
        assert_eq!(crd.spec.names.kind, "CloudflaredTunnel");
        assert_eq!(crd.spec.names.plural, "cloudflaredtunnels");
        assert_eq!(
            crd.spec.names.singular.as_deref(),
            Some("cloudflaredtunnel")
        );
        assert_eq!(
            crd.spec.names.short_names.as_deref(),
            Some(expected_short_names.as_slice())
        );
    }

    #[test]
    fn access_rules_round_trip_through_json() {
        let access = CloudflaredTunnelAccess {
            required: true,
            team_name: "team".to_string(),
            aud_tag: vec!["aud-a".to_string(), "aud-b".to_string()],
        };

        let rendered = serde_json::to_string(&access).expect("access should serialize");
        let parsed: CloudflaredTunnelAccess =
            serde_json::from_str(&rendered).expect("access should deserialize");

        assert_eq!(parsed, access);
    }
}
