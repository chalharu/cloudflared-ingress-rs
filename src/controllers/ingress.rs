//! Ingress controller that translates `Ingress` resources into `CloudflaredTunnel` CRDs.

use std::{
    collections::HashMap,
    fmt::Debug,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use futures::StreamExt as _;
use k8s_openapi::{
    api::{
        core::v1::Service,
        networking::v1::{
            HTTPIngressPath, HTTPIngressRuleValue, Ingress, IngressClass, IngressSpec,
        },
    },
    apimachinery::pkg::apis::meta::v1::OwnerReference,
};
use kube::{
    Api, Client, Resource, ResourceExt as _,
    api::{ListParams, ObjectMeta, PartialObjectMeta, PartialObjectMetaExt, Patch, PatchParams},
    runtime::{
        Controller, WatchStreamExt as _,
        controller::Action,
        metadata_watcher,
        reflector::{self, ObjectRef},
        watcher::{Config, watcher},
    },
};
use serde::de::DeserializeOwned;
use tracing::{info, warn};

use crate::{
    Error, Result,
    cli::ControllerArgs,
    controllers::cloudflared::{
        CloudflaredTunnelAccess, CloudflaredTunnelIngress, CloudflaredTunnelOriginRequest,
    },
};

use super::cloudflared::{CloudflaredTunnel, CloudflaredTunnelSpec};

const PATCH_PARAMS_APPLY_NAME: &str = "cloudflared-ingress.chalharu.top";

/// Initialize the controller and shared state (given the crd is installed)
pub async fn run_controllers(args: ControllerArgs) -> Result<()> {
    let client = Client::try_default().await?;
    let context = Arc::new(Context {
        client: client.clone(),
        args,
        target_ingressclass: Arc::new(Mutex::new(HashMap::new())),
    });
    run_controller(client, context).await;

    Ok(())
}

async fn get_ingress_classes(client: &Client, args: &ControllerArgs) -> Result<Vec<IngressClass>> {
    let ingress_class_api = Api::<IngressClass>::all(client.clone());
    let ingress_class = if let Some(ingress_class) = args.ingress_class() {
        ingress_class_api
            .get(ingress_class)
            .await
            .ok()
            .filter(|ic| matches_requested_ingress_class(ic, args))
            .into_iter()
            .collect()
    } else {
        ingress_class_api
            .list(&ListParams::default())
            .await?
            .items
            .into_iter()
            .filter(|ic| matches_requested_ingress_class(ic, args))
            .collect()
    };
    Ok(ingress_class)
}

async fn get_ingresses(
    client: &Client,
    ingress_class: &str,
    include_default: bool,
) -> Result<Vec<Ingress>> {
    let ingress_api = Api::<Ingress>::all(client.clone());
    let ingresses = ingress_api
        .list(&ListParams::default())
        .await?
        .items
        .into_iter()
        .filter(|ing| {
            ing.spec
                .as_ref()
                .and_then(|s| s.ingress_class_name.as_ref())
                .map_or(include_default, |c| c == ingress_class)
        })
        .collect::<Vec<_>>();
    Ok(ingresses)
}

async fn get_services(client: &Client) -> Result<Vec<Service>> {
    let service_api = Api::<Service>::all(client.clone());
    let services = service_api
        .list(&ListParams::default())
        .await?
        .items
        .into_iter()
        .collect::<Vec<_>>();
    Ok(services)
}

type PartialIngressClass = PartialObjectMeta<IngressClass>;
type ServicePortMap = HashMap<String, HashMap<String, i32>>;
type TargetIngressClassMap = HashMap<Option<String>, ObjectRef<PartialIngressClass>>;

fn lock_target_ingressclass(
    target_ingressclass: &Mutex<TargetIngressClassMap>,
) -> MutexGuard<'_, TargetIngressClassMap> {
    target_ingressclass
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn ingress_class_matches_controller(ic: &IngressClass, controller: &str) -> bool {
    ic.spec
        .as_ref()
        .and_then(|spec| spec.controller.as_ref())
        .is_some_and(|current| current == controller)
}

fn matches_requested_ingress_class(ic: &IngressClass, args: &ControllerArgs) -> bool {
    if !ingress_class_matches_controller(ic, args.ingress_controller()) {
        return false;
    }

    args.ingress_class()
        .map(|target| ic.name_any() == *target)
        .unwrap_or(true)
}

fn is_default_ingress_class(ic: &IngressClass) -> bool {
    ic.meta()
        .annotations
        .as_ref()
        .and_then(|annotations| annotations.get("ingressclass.kubernetes.io/is-default-class"))
        .is_some_and(|value| value.eq_ignore_ascii_case("true"))
}

fn build_target_ingress_class_map(ingress_classes: &[IngressClass]) -> TargetIngressClassMap {
    let mut targets = HashMap::new();

    for ingress_class in ingress_classes {
        let obj_ref = reflector::Lookup::to_object_ref(
            &ingress_class.metadata.clone().into_request_partial(),
            (),
        );
        if is_default_ingress_class(ingress_class) {
            targets.insert(None, obj_ref.clone());
        }
        targets.insert(Some(ingress_class.name_any()), obj_ref);
    }

    targets
}

fn build_service_port_map(services: Vec<Service>) -> ServicePortMap {
    services
        .into_iter()
        .filter_map(|service| {
            let namespace = service.namespace()?;
            let ports = service
                .spec
                .iter()
                .flat_map(|spec| {
                    spec.ports.iter().flat_map(|ports| {
                        ports.iter().filter_map(|port| {
                            port.name.as_ref().map(|name| (name.clone(), port.port))
                        })
                    })
                })
                .collect();

            Some((format!("{}.{}.svc", service.name_any(), namespace), ports))
        })
        .collect()
}

fn resolve_service_port(
    service: &k8s_openapi::api::networking::v1::IngressServiceBackend,
    namespace: &str,
    services: &ServicePortMap,
) -> Option<i32> {
    service.port.as_ref().and_then(|port| {
        port.number.or_else(|| {
            let service_name = format!("{}.{}.svc", service.name.as_str(), namespace);
            port.name.as_ref().and_then(|name| {
                services
                    .get(&service_name)
                    .and_then(|service_ports| service_ports.get(name).copied())
            })
        })
    })
}

fn build_service_url(scheme: &str, service_name: &str, port: Option<i32>) -> String {
    match port
        .filter(|port| !(*port == 80 && scheme == "http" || *port == 443 && scheme == "https"))
    {
        Some(port) => format!("{scheme}://{service_name}:{port}"),
        None => format!("{scheme}://{service_name}"),
    }
}

fn ingress_service_scheme(ingress: &Ingress) -> String {
    ingress
        .annotations()
        .get("cloudflared-ingress.ingress.kubernetes.io/service.serversscheme")
        .map(String::as_str)
        .unwrap_or("http")
        .to_lowercase()
}

fn split_annotation_csv(value: Option<&String>) -> Vec<String> {
    value
        .map(String::as_str)
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn build_origin_request(
    team_name: Option<&str>,
    aud_tags: &[String],
) -> CloudflaredTunnelOriginRequest {
    CloudflaredTunnelOriginRequest {
        access: team_name.map(|team_name| CloudflaredTunnelAccess {
            required: true,
            team_name: team_name.to_string(),
            aud_tag: aud_tags.to_vec(),
        }),
        no_tls_verify: Some(true),
        ..Default::default()
    }
}

#[allow(clippy::result_large_err)]
fn path_to_regex(path_type: &str, path: Option<&str>) -> Result<Option<String>> {
    match path_type {
        "Exact" => Ok(Some(format!("^{}$", regex_escape(path.unwrap_or("/"))))),
        "Prefix" => Ok(path.filter(|path| *path != "/").map(|path| {
            let normalized = path.trim_end_matches('/');
            let normalized = if normalized.is_empty() {
                "/"
            } else {
                normalized
            };
            format!("^{}(?:/|$)", regex_escape(normalized))
        })),
        "ImplementationSpecific" => Ok(path
            .filter(|path| *path != "/")
            .map(|path| format!("^{}", regex_escape(path)))),
        _ => Err(Error::illegal_document()),
    }
}

#[allow(clippy::result_large_err)]
fn ingress_rule_values(spec: &IngressSpec) -> Result<Vec<(Option<String>, HTTPIngressRuleValue)>> {
    let default_backend = spec
        .default_backend
        .as_ref()
        .map(|backend| HTTPIngressRuleValue {
            paths: vec![HTTPIngressPath {
                backend: backend.clone(),
                path: None,
                path_type: "ImplementationSpecific".to_string(),
            }],
        });
    let rules = spec.rules.as_deref().unwrap_or_default();

    if rules.is_empty() {
        return Ok(Vec::new());
    }

    let mut values = Vec::with_capacity(rules.len());
    for rule in rules {
        values.push((
            rule.host.clone(),
            rule.http
                .clone()
                .or_else(|| default_backend.clone())
                .ok_or_else(Error::illegal_document)?,
        ));
    }

    Ok(values)
}

#[allow(clippy::result_large_err)]
fn build_tunnel_ingresses_for_ingress(
    ingress: Ingress,
    services: &ServicePortMap,
) -> Result<Vec<CloudflaredTunnelIngress>> {
    let scheme = ingress_service_scheme(&ingress);
    let aud_tags = split_annotation_csv(
        ingress
            .annotations()
            .get("cloudflared-ingress.ingress.kubernetes.io/service.aud"),
    );
    let team_name = ingress
        .annotations()
        .get("cloudflared-ingress.ingress.kubernetes.io/service.team")
        .cloned();
    let namespace = ingress.namespace().ok_or_else(Error::illegal_document)?;
    let Some(spec) = ingress.spec else {
        return Ok(Vec::new());
    };
    let origin_request = build_origin_request(team_name.as_deref(), &aud_tags);
    let mut rendered_ingresses = Vec::new();

    for (host, rule_value) in ingress_rule_values(&spec)? {
        for path in rule_value.paths {
            if path.backend.resource.is_some() {
                return Err(Error::illegal_document());
            }
            let service = path.backend.service.ok_or_else(Error::illegal_document)?;
            let service_name = format!("{}.{}.svc", service.name.as_str(), namespace);
            let service_url = build_service_url(
                &scheme,
                &service_name,
                resolve_service_port(&service, &namespace, services),
            );

            rendered_ingresses.push(CloudflaredTunnelIngress {
                hostname: host.clone().ok_or_else(Error::illegal_document)?,
                service: service_url,
                path: path_to_regex(path.path_type.as_str(), path.path.as_deref())?,
                origin_request: Some(origin_request.clone()),
            });
        }
    }

    Ok(rendered_ingresses)
}

#[allow(clippy::result_large_err)]
fn collect_tunnel_ingresses(
    ingresses: Vec<Ingress>,
    services: &ServicePortMap,
) -> Result<Vec<CloudflaredTunnelIngress>> {
    let mut rendered_ingresses = Vec::new();

    for ingress in ingresses {
        rendered_ingresses.extend(build_tunnel_ingresses_for_ingress(ingress, services)?);
    }

    Ok(rendered_ingresses)
}

fn build_cloudflared_tunnel(
    name: String,
    owner_references: Vec<OwnerReference>,
    ingress: Vec<CloudflaredTunnelIngress>,
) -> CloudflaredTunnel {
    CloudflaredTunnel {
        metadata: ObjectMeta {
            name: Some(name),
            owner_references: Some(owner_references),
            ..Default::default()
        },
        spec: CloudflaredTunnelSpec {
            ingress: Some(ingress),
            default_ingress_service: "http_status:404".to_string(),
            ..Default::default()
        },
        status: None,
    }
}

// Context for our reconciler
#[derive(Clone)]
struct Context {
    /// Kubernetes client
    client: Client,
    args: ControllerArgs,
    target_ingressclass: Arc<Mutex<TargetIngressClassMap>>,
}

async fn run_controller(client: Client, context: Arc<Context>) {
    info!("Starting controller for Ingress");

    let api_ingressclass = Api::<IngressClass>::all(client.clone());
    let api_ingress = Api::<Ingress>::all(client);
    let (reader_ingressclass, writer_ingressclass) = reflector::store();

    // controller main stream from metadata_watcher
    let stream_ingressclass = metadata_watcher(api_ingressclass, Config::default())
        .default_backoff()
        .reflect(writer_ingressclass)
        .applied_objects();

    let stream_ingress = watcher(api_ingress, Config::default()).touched_objects();

    let target_ingressclass = context.target_ingressclass.clone();
    Controller::for_stream(stream_ingressclass, reader_ingressclass)
        .watches_stream(stream_ingress, move |i| {
            let target_ingressclass = target_ingressclass.clone();
            i.spec.and_then(|is| {
                lock_target_ingressclass(&target_ingressclass)
                    .get(&is.ingress_class_name)
                    .cloned()
            })
        })
        .shutdown_on_signal()
        .run(reconcile, error_policy, context)
        .for_each(|_| futures::future::ready(()))
        .await;

    info!("controller for Ingress shutdown");
}

async fn reconcile<K>(res: Arc<PartialObjectMeta<K>>, ctx: Arc<Context>) -> Result<Action>
where
    K: Resource<DynamicType = ()> + Clone + DeserializeOwned + Debug,
{
    let kind = K::kind(&()).to_string();
    let name = res.name_any();
    if let Some(ns) = res.namespace() {
        info!("Reconciling {kind} \"{name}\" in {ns}");
    } else {
        info!("Reconciling {kind} \"{name}\"");
    }
    ctx.reconcile().await?;
    Ok(Action::requeue(Duration::from_secs(60 * 60)))
}

fn error_policy<K>(_: Arc<K>, error: &Error, _ctx: Arc<Context>) -> Action {
    warn!("reconcile failed: {error:?}");
    Action::requeue(Duration::from_secs(5 * 60))
}

impl Context {
    async fn reconcile(&self) -> Result<()> {
        let ingress_class = get_ingress_classes(&self.client, &self.args).await?;

        *lock_target_ingressclass(&self.target_ingressclass) =
            build_target_ingress_class_map(&ingress_class);

        for ic in ingress_class {
            self.reconcile_for_ingressclass(ic.clone(), is_default_ingress_class(&ic))
                .await?;
        }
        Ok(())
    }

    async fn reconcile_for_ingressclass(
        &self,
        ic: IngressClass,
        is_default_class: bool,
    ) -> Result<()> {
        let ingresses = get_ingresses(&self.client, &ic.name_any(), is_default_class).await?;
        let name = ic.name_any();
        let owner_ref = ic.controller_owner_ref(&());

        let cfdt_api = Api::<CloudflaredTunnel>::namespaced(
            self.client.clone(),
            self.args.cloudflare_tunnel_namespace(),
        );
        let services = build_service_port_map(get_services(&self.client).await?);
        let cfdt_ingress = collect_tunnel_ingresses(ingresses, &services)?;
        let cfd =
            build_cloudflared_tunnel(name.clone(), owner_ref.into_iter().collect(), cfdt_ingress);

        cfdt_api
            .patch(
                name.as_str(),
                &PatchParams::apply(PATCH_PARAMS_APPLY_NAME).force(),
                &Patch::Apply(cfd),
            )
            .await?;
        Ok(())
    }
}

fn regex_escape(s: &str) -> String {
    s.replace("\\", "\\\\")
        .replace("*", "\\*")
        .replace("+", "\\+")
        .replace("?", "\\?")
        .replace("{", "\\{")
        .replace("}", "\\}")
        .replace("(", "\\(")
        .replace(")", "\\)")
        .replace("[", "\\[")
        .replace("]", "\\]")
        .replace("^", "\\^")
        .replace("$", "\\$")
        .replace("-", "\\-")
        .replace("|", "\\|")
        .replace(".", "\\.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controllers::test_support;
    use k8s_openapi::api::networking::v1::{
        IngressBackend, IngressClassSpec, IngressRule, IngressServiceBackend, IngressSpec,
        ServiceBackendPort,
    };
    use kube::api::{DeleteParams, PostParams};

    fn controller_args(ingress_class: Option<&str>) -> ControllerArgs {
        ControllerArgs::new_for_test(
            ingress_class.map(str::to_string),
            "chalharu.top/cloudflared-ingress-controller",
        )
    }

    fn ingress_class(name: &str, controller: &str) -> IngressClass {
        IngressClass {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: Some(IngressClassSpec {
                controller: Some(controller.to_string()),
                parameters: None,
            }),
        }
    }

    fn ingress_with_rules(
        host: Option<&str>,
        namespace: &str,
        annotations: Vec<(&str, &str)>,
        rules: Option<Vec<IngressRule>>,
        default_backend: Option<IngressBackend>,
    ) -> Ingress {
        Ingress {
            metadata: ObjectMeta {
                name: Some("demo".to_string()),
                namespace: Some(namespace.to_string()),
                annotations: Some(
                    annotations
                        .into_iter()
                        .map(|(key, value)| (key.to_string(), value.to_string()))
                        .collect(),
                ),
                ..Default::default()
            },
            spec: Some(IngressSpec {
                ingress_class_name: Some("public".to_string()),
                rules: rules.or_else(|| {
                    host.map(|host| {
                        vec![IngressRule {
                            host: Some(host.to_string()),
                            http: Some(HTTPIngressRuleValue {
                                paths: vec![HTTPIngressPath {
                                    backend: IngressBackend {
                                        service: Some(IngressServiceBackend {
                                            name: "api".to_string(),
                                            port: Some(ServiceBackendPort {
                                                number: Some(80),
                                                name: None,
                                            }),
                                        }),
                                        resource: None,
                                    },
                                    path: Some("/".to_string()),
                                    path_type: "Prefix".to_string(),
                                }],
                            }),
                        }]
                    })
                }),
                default_backend,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn matches_requested_ingress_class_checks_the_controller_value() {
        let args = controller_args(Some("public"));

        let matching = ingress_class("public", args.ingress_controller());
        let wrong_controller = ingress_class("public", "public");

        assert!(matches_requested_ingress_class(&matching, &args));
        assert!(!matches_requested_ingress_class(&wrong_controller, &args));
    }

    #[test]
    fn ingress_class_matches_controller_requires_an_exact_controller_match() {
        let ingress_class = ingress_class("public", "example.com/controller");

        assert!(ingress_class_matches_controller(
            &ingress_class,
            "example.com/controller"
        ));
        assert!(!ingress_class_matches_controller(
            &ingress_class,
            "example.com/other"
        ));
    }

    #[test]
    fn is_default_ingress_class_uses_case_insensitive_annotation() {
        let mut default_class = ingress_class("public", "controller");
        default_class.metadata.annotations = Some(std::collections::BTreeMap::from([(
            "ingressclass.kubernetes.io/is-default-class".to_string(),
            "TrUe".to_string(),
        )]));

        let non_default_class = ingress_class("private", "controller");

        assert!(is_default_ingress_class(&default_class));
        assert!(!is_default_ingress_class(&non_default_class));
    }

    #[test]
    fn lock_target_ingressclass_recovers_after_mutex_poisoning() {
        let target_ingressclass = Arc::new(Mutex::new(TargetIngressClassMap::new()));
        let poisoned = target_ingressclass.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoned.lock().expect("mutex should lock");
            panic!("poison the mutex");
        })
        .join();

        let mut guard = lock_target_ingressclass(&target_ingressclass);
        guard.insert(None, ObjectRef::new("public"));

        assert!(guard.contains_key(&None));
    }

    #[test]
    fn resolve_service_port_supports_named_and_numeric_ports() {
        let services = HashMap::from([(
            "api.default.svc".to_string(),
            HashMap::from([("https".to_string(), 8443)]),
        )]);
        let named = IngressServiceBackend {
            name: "api".to_string(),
            port: Some(ServiceBackendPort {
                name: Some("https".to_string()),
                number: None,
            }),
        };
        let numeric = IngressServiceBackend {
            name: "api".to_string(),
            port: Some(ServiceBackendPort {
                name: None,
                number: Some(8080),
            }),
        };

        assert_eq!(
            resolve_service_port(&named, "default", &services),
            Some(8443)
        );
        assert_eq!(
            resolve_service_port(&numeric, "default", &services),
            Some(8080)
        );
    }

    #[test]
    fn build_service_url_omits_default_ports() {
        assert_eq!(
            build_service_url("http", "api.default.svc", Some(80)),
            "http://api.default.svc"
        );
        assert_eq!(
            build_service_url("https", "api.default.svc", Some(443)),
            "https://api.default.svc"
        );
        assert_eq!(
            build_service_url("https", "api.default.svc", Some(8443)),
            "https://api.default.svc:8443"
        );
    }

    #[test]
    fn path_to_regex_formats_exact_and_prefix_matches() {
        assert_eq!(
            path_to_regex("Exact", Some("/api/v1/users[0]")).unwrap(),
            Some("^/api/v1/users\\[0\\]$".to_string())
        );
        assert_eq!(
            path_to_regex("Prefix", Some("/v1+beta")).unwrap(),
            Some("^/v1\\+beta(?:/|$)".to_string())
        );
        assert_eq!(
            path_to_regex("ImplementationSpecific", Some("/v1+beta")).unwrap(),
            Some("^/v1\\+beta".to_string())
        );
        assert_eq!(path_to_regex("Prefix", Some("/")).unwrap(), None);
        assert!(path_to_regex("Unknown", Some("/")).is_err());
    }

    #[test]
    fn build_service_port_map_keeps_only_named_ports_for_namespaced_services() {
        let services = vec![
            Service {
                metadata: ObjectMeta {
                    name: Some("api".to_string()),
                    namespace: Some("default".to_string()),
                    ..Default::default()
                },
                spec: Some(k8s_openapi::api::core::v1::ServiceSpec {
                    ports: Some(vec![
                        k8s_openapi::api::core::v1::ServicePort {
                            name: Some("http".to_string()),
                            port: 80,
                            ..Default::default()
                        },
                        k8s_openapi::api::core::v1::ServicePort {
                            name: None,
                            port: 443,
                            ..Default::default()
                        },
                    ]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Service {
                metadata: ObjectMeta {
                    name: Some("missing-ns".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
        ];

        let map = build_service_port_map(services);

        assert_eq!(map.len(), 1);
        assert_eq!(map["api.default.svc"]["http"], 80);
        assert!(!map["api.default.svc"].contains_key("443"));
    }

    #[test]
    fn build_target_ingress_class_map_tracks_default_and_named_classes() {
        let public = ingress_class("public", "controller");
        let mut private = ingress_class("private", "controller");
        private.metadata.annotations = Some(std::collections::BTreeMap::from([(
            "ingressclass.kubernetes.io/is-default-class".to_string(),
            "TRUE".to_string(),
        )]));

        let map = build_target_ingress_class_map(&[public, private]);

        assert!(map.contains_key(&Some("public".to_string())));
        assert!(map.contains_key(&Some("private".to_string())));
        assert!(map.contains_key(&None));
    }

    #[test]
    fn split_annotation_csv_trims_and_discards_empty_values() {
        let values = split_annotation_csv(Some(&"  aud-a, ,aud-b  ".to_string()));

        assert_eq!(values, vec!["aud-a".to_string(), "aud-b".to_string()]);
    }

    #[test]
    fn split_annotation_csv_returns_empty_when_annotation_is_missing() {
        assert!(split_annotation_csv(None).is_empty());
    }

    #[test]
    fn build_origin_request_enables_access_when_team_is_present() {
        let aud_tags = vec!["aud-a".to_string(), "aud-b".to_string()];

        let origin_request = build_origin_request(Some("team"), &aud_tags);
        let default_request = build_origin_request(None, &aud_tags);

        assert_eq!(origin_request.no_tls_verify, Some(true));
        assert_eq!(
            origin_request
                .access
                .as_ref()
                .expect("access should exist")
                .team_name,
            "team"
        );
        assert_eq!(default_request.no_tls_verify, Some(true));
        assert_eq!(default_request.access, None);
    }

    #[test]
    fn ingress_service_scheme_defaults_to_http_and_normalizes_case() {
        let default_ingress =
            ingress_with_rules(Some("example.com"), "default", Vec::new(), None, None);
        let https_ingress = ingress_with_rules(
            Some("example.com"),
            "default",
            vec![(
                "cloudflared-ingress.ingress.kubernetes.io/service.serversscheme",
                "HTTPS",
            )],
            None,
            None,
        );

        assert_eq!(ingress_service_scheme(&default_ingress), "http");
        assert_eq!(ingress_service_scheme(&https_ingress), "https");
    }

    #[test]
    fn ingress_rule_values_without_explicit_rules_returns_no_entries() {
        let values = ingress_rule_values(&IngressSpec {
            default_backend: Some(IngressBackend {
                service: Some(IngressServiceBackend {
                    name: "api".to_string(),
                    port: Some(ServiceBackendPort {
                        name: Some("https".to_string()),
                        number: None,
                    }),
                }),
                resource: None,
            }),
            ..Default::default()
        })
        .expect("missing rules should be ignored");

        assert!(values.is_empty());
    }

    #[test]
    fn ingress_rule_values_reuses_the_default_backend_when_http_rule_is_missing() {
        let values = ingress_rule_values(&IngressSpec {
            default_backend: Some(IngressBackend {
                service: Some(IngressServiceBackend {
                    name: "api".to_string(),
                    port: Some(ServiceBackendPort {
                        number: Some(8080),
                        name: None,
                    }),
                }),
                resource: None,
            }),
            rules: Some(vec![IngressRule {
                host: Some("example.com".to_string()),
                http: None,
            }]),
            ..Default::default()
        })
        .expect("default backend should be reused");

        assert_eq!(values.len(), 1);
        assert_eq!(values[0].0.as_deref(), Some("example.com"));
        assert_eq!(
            values[0].1.paths[0]
                .backend
                .service
                .as_ref()
                .map(|service| service.name.as_str()),
            Some("api")
        );
    }

    #[test]
    fn ingress_rule_values_rejects_rules_without_http_or_default_backend() {
        let result = ingress_rule_values(&IngressSpec {
            rules: Some(vec![IngressRule {
                host: Some("example.com".to_string()),
                http: None,
            }]),
            ..Default::default()
        });

        assert!(matches!(result, Err(Error::IllegalDocument { .. })));
    }

    #[test]
    fn build_tunnel_ingresses_for_ingress_skips_default_backend_without_host_rules() {
        let services = HashMap::from([(
            "api.default.svc".to_string(),
            HashMap::from([("https".to_string(), 8443)]),
        )]);
        let ingress = Ingress {
            metadata: ObjectMeta {
                name: Some("default-backend-only".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            spec: Some(IngressSpec {
                default_backend: Some(IngressBackend {
                    service: Some(IngressServiceBackend {
                        name: "api".to_string(),
                        port: Some(ServiceBackendPort {
                            name: Some("https".to_string()),
                            number: None,
                        }),
                    }),
                    resource: None,
                }),
                ..Default::default()
            }),
            ..Default::default()
        };

        let rendered = build_tunnel_ingresses_for_ingress(ingress, &services)
            .expect("default-backend-only ingress should be skipped");

        assert!(rendered.is_empty());
    }

    #[test]
    fn build_tunnel_ingresses_for_ingress_returns_empty_when_spec_is_missing() {
        let ingress = Ingress {
            metadata: ObjectMeta {
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            spec: None,
            ..Default::default()
        };

        let rendered = build_tunnel_ingresses_for_ingress(ingress, &HashMap::new())
            .expect("missing spec should be ignored");

        assert!(rendered.is_empty());
    }

    #[test]
    fn build_tunnel_ingresses_for_ingress_requires_a_namespace() {
        let ingress = ingress_with_rules(Some("example.com"), "default", Vec::new(), None, None);
        let mut ingress_without_namespace = ingress.clone();
        ingress_without_namespace.metadata.namespace = None;

        let result = build_tunnel_ingresses_for_ingress(ingress_without_namespace, &HashMap::new());

        assert!(matches!(result, Err(Error::IllegalDocument { .. })));
    }

    #[test]
    fn build_tunnel_ingresses_for_ingress_builds_expected_entries() {
        let services = HashMap::from([(
            "api.default.svc".to_string(),
            HashMap::from([("https".to_string(), 8443)]),
        )]);
        let ingress = ingress_with_rules(
            Some("example.com"),
            "default",
            vec![
                (
                    "cloudflared-ingress.ingress.kubernetes.io/service.serversscheme",
                    "HTTPS",
                ),
                (
                    "cloudflared-ingress.ingress.kubernetes.io/service.aud",
                    "aud-a, aud-b",
                ),
                (
                    "cloudflared-ingress.ingress.kubernetes.io/service.team",
                    "team",
                ),
            ],
            Some(vec![IngressRule {
                host: Some("example.com".to_string()),
                http: Some(HTTPIngressRuleValue {
                    paths: vec![HTTPIngressPath {
                        backend: IngressBackend {
                            service: Some(IngressServiceBackend {
                                name: "api".to_string(),
                                port: Some(ServiceBackendPort {
                                    name: Some("https".to_string()),
                                    number: None,
                                }),
                            }),
                            resource: None,
                        },
                        path: Some("/api".to_string()),
                        path_type: "Prefix".to_string(),
                    }],
                }),
            }]),
            None,
        );

        let rendered = build_tunnel_ingresses_for_ingress(ingress, &services)
            .expect("ingress should render successfully");

        assert_eq!(rendered.len(), 1);
        assert_eq!(rendered[0].hostname, "example.com");
        assert_eq!(rendered[0].service, "https://api.default.svc:8443");
        assert_eq!(rendered[0].path.as_deref(), Some("^/api(?:/|$)"));
        assert_eq!(
            rendered[0]
                .origin_request
                .as_ref()
                .and_then(|origin_request| origin_request.access.as_ref())
                .expect("access should exist")
                .aud_tag,
            vec!["aud-a".to_string(), "aud-b".to_string()]
        );
    }

    #[test]
    fn build_tunnel_ingresses_for_ingress_requires_a_host() {
        let services = HashMap::new();
        let ingress = ingress_with_rules(
            None,
            "default",
            Vec::new(),
            Some(vec![IngressRule {
                host: None,
                http: Some(HTTPIngressRuleValue {
                    paths: vec![HTTPIngressPath {
                        backend: IngressBackend {
                            service: Some(IngressServiceBackend {
                                name: "api".to_string(),
                                port: Some(ServiceBackendPort {
                                    number: Some(80),
                                    name: None,
                                }),
                            }),
                            resource: None,
                        },
                        path: Some("/".to_string()),
                        path_type: "Prefix".to_string(),
                    }],
                }),
            }]),
            None,
        );

        let result = build_tunnel_ingresses_for_ingress(ingress, &services);

        assert!(matches!(result, Err(Error::IllegalDocument { .. })));
    }

    #[test]
    fn build_tunnel_ingresses_for_ingress_requires_service_backends() {
        let ingress = ingress_with_rules(
            Some("example.com"),
            "default",
            Vec::new(),
            Some(vec![IngressRule {
                host: Some("example.com".to_string()),
                http: Some(HTTPIngressRuleValue {
                    paths: vec![HTTPIngressPath {
                        backend: IngressBackend {
                            service: None,
                            resource: None,
                        },
                        path: Some("/".to_string()),
                        path_type: "Prefix".to_string(),
                    }],
                }),
            }]),
            None,
        );

        let result = build_tunnel_ingresses_for_ingress(ingress, &HashMap::new());

        assert!(matches!(result, Err(Error::IllegalDocument { .. })));
    }

    #[test]
    fn build_tunnel_ingresses_for_ingress_rejects_resource_backends() {
        let ingress = ingress_with_rules(
            Some("example.com"),
            "default",
            Vec::new(),
            Some(vec![IngressRule {
                host: Some("example.com".to_string()),
                http: Some(HTTPIngressRuleValue {
                    paths: vec![HTTPIngressPath {
                        backend: IngressBackend {
                            service: None,
                            resource: Some(Default::default()),
                        },
                        path: Some("/".to_string()),
                        path_type: "ImplementationSpecific".to_string(),
                    }],
                }),
            }]),
            None,
        );

        let result = build_tunnel_ingresses_for_ingress(ingress, &HashMap::new());

        assert!(matches!(result, Err(Error::IllegalDocument { .. })));
    }

    #[test]
    fn collect_tunnel_ingresses_flattens_rendered_entries_from_all_ingresses() {
        let services = HashMap::from([(
            "api.default.svc".to_string(),
            HashMap::from([("https".to_string(), 8443)]),
        )]);
        let ingresses = vec![
            ingress_with_rules(Some("one.example.com"), "default", Vec::new(), None, None),
            ingress_with_rules(Some("two.example.com"), "default", Vec::new(), None, None),
        ];

        let rendered =
            collect_tunnel_ingresses(ingresses, &services).expect("all ingresses should render");

        assert_eq!(rendered.len(), 2);
        assert_eq!(rendered[0].hostname, "one.example.com");
        assert_eq!(rendered[1].hostname, "two.example.com");
    }

    #[test]
    fn build_cloudflared_tunnel_uses_expected_defaults() {
        let owner_references = vec![OwnerReference {
            name: "public".to_string(),
            ..Default::default()
        }];
        let ingress = vec![CloudflaredTunnelIngress {
            hostname: "example.com".to_string(),
            service: "https://api.default.svc".to_string(),
            path: None,
            origin_request: None,
        }];

        let tunnel = build_cloudflared_tunnel("public".to_string(), owner_references, ingress);

        assert_eq!(tunnel.metadata.name.as_deref(), Some("public"));
        assert_eq!(
            tunnel
                .metadata
                .owner_references
                .as_ref()
                .expect("owner references should exist")[0]
                .name,
            "public"
        );
        assert_eq!(
            tunnel.spec.default_ingress_service,
            "http_status:404".to_string()
        );
        assert_eq!(
            tunnel
                .spec
                .ingress
                .as_ref()
                .expect("ingress should exist")
                .len(),
            1
        );
    }

    #[test]
    fn regex_escape_escapes_special_characters() {
        assert_eq!(
            regex_escape(r"\*+?{}()[]^$|."),
            r"\\\*\+\?\{\}\(\)\[\]\^\$\|\."
        );
    }

    #[tokio::test]
    async fn error_policy_requeues_after_five_minutes() {
        let action = error_policy(
            Arc::new(PartialObjectMeta::<IngressClass>::default()),
            &Error::illegal_document(),
            Arc::new(Context {
                client: test_client(),
                args: controller_args(None),
                target_ingressclass: Arc::new(Mutex::new(HashMap::new())),
            }),
        );

        assert_eq!(action, Action::requeue(Duration::from_secs(5 * 60)));
    }

    #[tokio::test]
    async fn kind_reconcile_creates_cloudflared_tunnel_from_cluster_resources() {
        let Some(client) = test_support::kind_client().await else {
            return;
        };

        test_support::ensure_cloudflared_crd(&client).await;
        let app_namespace = test_support::unique_name("ingress-app");
        let tunnel_namespace = test_support::unique_name("ingress-tunnel");
        let ingress_class_name = test_support::unique_name("public");
        test_support::ensure_namespace(&client, &app_namespace).await;
        test_support::ensure_namespace(&client, &tunnel_namespace).await;

        let args = ControllerArgs::new_for_test_with_namespace(
            Some(ingress_class_name.clone()),
            "chalharu.top/cloudflared-ingress-controller",
            tunnel_namespace.clone(),
        );

        let ingress_class_api = Api::<IngressClass>::all(client.clone());
        let created_ingress_class = ingress_class_api
            .create(
                &PostParams::default(),
                &ingress_class(&ingress_class_name, args.ingress_controller()),
            )
            .await
            .expect("ingress class should create");

        let service_api = Api::<Service>::namespaced(client.clone(), &app_namespace);
        service_api
            .create(
                &PostParams::default(),
                &Service {
                    metadata: ObjectMeta {
                        name: Some("api".to_string()),
                        namespace: Some(app_namespace.clone()),
                        ..Default::default()
                    },
                    spec: Some(k8s_openapi::api::core::v1::ServiceSpec {
                        ports: Some(vec![k8s_openapi::api::core::v1::ServicePort {
                            name: Some("https".to_string()),
                            port: 8443,
                            ..Default::default()
                        }]),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await
            .expect("service should create");

        let ingress_api = Api::<Ingress>::namespaced(client.clone(), &app_namespace);
        let mut ingress = ingress_with_rules(
            Some("app.example.com"),
            &app_namespace,
            vec![
                (
                    "cloudflared-ingress.ingress.kubernetes.io/service.serversscheme",
                    "HTTPS",
                ),
                (
                    "cloudflared-ingress.ingress.kubernetes.io/service.aud",
                    "aud-a, aud-b",
                ),
                (
                    "cloudflared-ingress.ingress.kubernetes.io/service.team",
                    "team",
                ),
            ],
            Some(vec![IngressRule {
                host: Some("app.example.com".to_string()),
                http: Some(HTTPIngressRuleValue {
                    paths: vec![HTTPIngressPath {
                        backend: IngressBackend {
                            service: Some(IngressServiceBackend {
                                name: "api".to_string(),
                                port: Some(ServiceBackendPort {
                                    name: Some("https".to_string()),
                                    number: None,
                                }),
                            }),
                            resource: None,
                        },
                        path: Some("/api".to_string()),
                        path_type: "Prefix".to_string(),
                    }],
                }),
            }]),
            None,
        );
        ingress
            .spec
            .as_mut()
            .expect("spec should exist")
            .ingress_class_name = Some(ingress_class_name.clone());
        ingress_api
            .create(&PostParams::default(), &ingress)
            .await
            .expect("ingress should create");

        let context = Arc::new(Context {
            client: client.clone(),
            args,
            target_ingressclass: Arc::new(Mutex::new(HashMap::new())),
        });

        context
            .reconcile()
            .await
            .expect("reconcile should translate ingress resources");

        let cfdt_api = Api::<CloudflaredTunnel>::namespaced(client.clone(), &tunnel_namespace);
        let cfdt_name = ingress_class_name.clone();
        let cloudflared_tunnel = test_support::wait_for(
            "CloudflaredTunnel creation from ingress reconciliation",
            Duration::from_secs(10),
            || {
                let cfdt_api = cfdt_api.clone();
                let cfdt_name = cfdt_name.clone();
                async move {
                    cfdt_api
                        .get_opt(&cfdt_name)
                        .await
                        .expect("CloudflaredTunnel lookup should succeed")
                }
            },
        )
        .await;

        assert!(
            lock_target_ingressclass(&context.target_ingressclass)
                .contains_key(&Some(ingress_class_name.clone()))
        );
        assert_eq!(
            cloudflared_tunnel.metadata.name.as_deref(),
            Some(cfdt_name.as_str())
        );
        assert_eq!(
            cloudflared_tunnel
                .metadata
                .owner_references
                .as_ref()
                .map(|refs| {
                    refs.iter()
                        .map(|owner_ref| owner_ref.name.clone())
                        .collect::<Vec<_>>()
                }),
            Some(vec![created_ingress_class.name_any()])
        );
        let rendered_ingress = cloudflared_tunnel
            .spec
            .ingress
            .as_ref()
            .expect("ingress rules should exist");
        assert_eq!(rendered_ingress.len(), 1);
        assert_eq!(rendered_ingress[0].hostname, "app.example.com");
        assert_eq!(
            rendered_ingress[0].service,
            format!("https://api.{app_namespace}.svc:8443")
        );
        assert_eq!(rendered_ingress[0].path.as_deref(), Some("^/api(?:/|$)"));
        assert_eq!(
            rendered_ingress[0]
                .origin_request
                .as_ref()
                .and_then(|origin_request| origin_request.access.as_ref())
                .map(|access| access.aud_tag.clone()),
            Some(vec!["aud-a".to_string(), "aud-b".to_string()])
        );

        ingress_class_api
            .delete(&ingress_class_name, &DeleteParams::background())
            .await
            .expect("ingress class should delete");
        test_support::cleanup_namespace(&client, &app_namespace).await;
        test_support::cleanup_namespace(&client, &tunnel_namespace).await;
    }

    fn test_client() -> Client {
        Client::try_from(kube::Config::new(
            "http://127.0.0.1:1"
                .parse()
                .expect("test server URI should parse"),
        ))
        .expect("client should build")
    }
}
