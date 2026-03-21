use std::{
    collections::{HashMap, HashSet},
    fmt::Debug,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use futures::StreamExt as _;
use k8s_openapi::api::{
    core::v1::Service,
    networking::v1::{HTTPIngressPath, HTTPIngressRuleValue, Ingress, IngressClass},
};
use kube::{
    api::{ListParams, ObjectMeta, PartialObjectMeta, PartialObjectMetaExt, Patch, PatchParams},
    runtime::{
        controller::Action,
        metadata_watcher,
        reflector::{self, ObjectRef},
        watcher::{watcher, Config},
        Controller, WatchStreamExt as _,
    },
    Api, Client, Resource, ResourceExt as _,
};
use serde::de::DeserializeOwned;
use tracing::{info, warn};

use crate::{
    cli::ControllerArgs,
    controllers::cloudflared::{
        CloudflaredTunnelAccess, CloudflaredTunnelIngress, CloudflaredTunnelOriginRequest,
    },
    Error, Result,
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

#[allow(clippy::result_large_err)]
fn path_to_regex(path_type: &str, path: Option<&str>) -> Result<Option<String>> {
    match path_type {
        "Exact" => Ok(Some(format!("^{}$", regex_escape(path.unwrap_or("/"))))),
        "Prefix" | "ImplementationSpecific" => Ok(path
            .filter(|path| *path != "/")
            .map(|path| format!("^{}", regex_escape(path)))),
        _ => Err(Error::illegal_document()),
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

        let mut current_ic: HashSet<_> = lock_target_ingressclass(&self.target_ingressclass)
            .keys()
            .cloned()
            .collect();

        for ic in ingress_class.iter() {
            let is_default_class = ic
                .meta()
                .annotations
                .as_ref()
                .and_then(|a| a.get("ingressclass.kubernetes.io/is-default-class"))
                .is_some_and(|x| x.eq_ignore_ascii_case("true"));
            let name = ic.name_any();

            let obj_ref =
                reflector::Lookup::to_object_ref(&ic.metadata.clone().into_request_partial(), ());
            if is_default_class {
                current_ic.remove(&None);
                lock_target_ingressclass(&self.target_ingressclass).insert(None, obj_ref.clone());
            }
            current_ic.remove(&Some(name.clone()));
            lock_target_ingressclass(&self.target_ingressclass)
                .insert(Some(name.clone()), obj_ref.clone());
        }

        for ic in current_ic {
            lock_target_ingressclass(&self.target_ingressclass).remove(&ic);
        }

        for ic in ingress_class {
            let is_default_class = ic
                .meta()
                .annotations
                .as_ref()
                .and_then(|a| a.get("ingressclass.kubernetes.io/is-default-class"))
                .is_some_and(|x| x.eq_ignore_ascii_case("true"));

            self.reconcile_for_ingressclass(ic, is_default_class)
                .await?;
        }
        Ok(())
    }

    async fn reconcile_for_ingressclass(
        &self,
        ic: IngressClass,
        is_default_class: bool,
    ) -> Result<()> {
        const SERVERSSCHEME_ANNOTATION: &str =
            "cloudflared-ingress.ingress.kubernetes.io/service.serversscheme";
        const ACCESS_AUD_ANNOTATION: &str = "cloudflared-ingress.ingress.kubernetes.io/service.aud";
        const ACCESS_TEAM_ANNOTATION: &str =
            "cloudflared-ingress.ingress.kubernetes.io/service.team";

        let ingresses = get_ingresses(&self.client, &ic.name_any(), is_default_class).await?;
        let name = ic.name_any();
        let owner_ref = ic.controller_owner_ref(&());

        let mut cfdt_ingress = Vec::new();

        let cfdt_api = Api::<CloudflaredTunnel>::namespaced(
            self.client.clone(),
            self.args.cloudflare_tunnel_namespace(),
        );
        let services = build_service_port_map(get_services(&self.client).await?);

        for i in ingresses.into_iter() {
            let scheme = i
                .annotations()
                .get(SERVERSSCHEME_ANNOTATION)
                .map(String::as_str)
                .unwrap_or("http")
                .to_lowercase();

            let aud_tags = i
                .annotations()
                .get(ACCESS_AUD_ANNOTATION)
                .map(String::as_str)
                .map(|s| {
                    s.split(',')
                        .map(str::trim)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            let team_name = i.annotations().get(ACCESS_TEAM_ANNOTATION).cloned();

            let ns = i.namespace().ok_or_else(Error::illegal_document)?;

            let Some(spec) = i.spec else {
                continue;
            };

            let default_backend =
                spec.default_backend
                    .as_ref()
                    .map(|backend| HTTPIngressRuleValue {
                        paths: vec![HTTPIngressPath {
                            backend: backend.clone(),
                            path: None,
                            path_type: "ImplementationSpecific".to_string(),
                        }],
                    });

            let origin_request = team_name
                .map(|t| CloudflaredTunnelOriginRequest {
                    access: Some(CloudflaredTunnelAccess {
                        required: true,
                        team_name: t.to_string(),
                        aud_tag: aud_tags,
                    }),
                    no_tls_verify: Some(true),
                    ..Default::default()
                })
                .or(Some(CloudflaredTunnelOriginRequest {
                    no_tls_verify: Some(true),
                    ..Default::default()
                }));

            for r in spec.rules.iter().flat_map(|r| r.iter()) {
                for p in r
                    .http
                    .as_ref()
                    .or(default_backend.as_ref())
                    .ok_or_else(Error::illegal_document)?
                    .paths
                    .iter()
                {
                    if p.backend.resource.is_some() {
                        return Err(Error::illegal_document());
                    }
                    let Some(ref service) = p.backend.service else {
                        return Err(Error::illegal_document());
                    };
                    let svc_name = format!("{}.{}.svc", service.name.as_str(), ns);
                    let cfdt_service = build_service_url(
                        &scheme,
                        &svc_name,
                        resolve_service_port(service, &ns, &services),
                    );
                    let path = path_to_regex(p.path_type.as_str(), p.path.as_deref())?;

                    cfdt_ingress.push(CloudflaredTunnelIngress {
                        // Hostなしは最終的にCNAMEが振れないことからエラーとする
                        hostname: r.host.clone().ok_or_else(Error::illegal_document)?,
                        service: cfdt_service,
                        path,
                        origin_request: origin_request.clone(),
                    });
                }
            }
        }
        let cfd = CloudflaredTunnel {
            metadata: ObjectMeta {
                name: Some(name.clone()),
                owner_references: Some(owner_ref.into_iter().collect()),
                ..Default::default()
            },
            spec: CloudflaredTunnelSpec {
                ingress: Some(cfdt_ingress),
                default_ingress_service: "http_status:404".to_string(),
                ..Default::default()
            },
            status: None,
        };

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
    use k8s_openapi::api::networking::v1::{
        IngressClassSpec, IngressServiceBackend, ServiceBackendPort,
    };

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

    #[test]
    fn matches_requested_ingress_class_checks_the_controller_value() {
        let args = controller_args(Some("public"));

        let matching = ingress_class("public", args.ingress_controller());
        let wrong_controller = ingress_class("public", "public");

        assert!(matches_requested_ingress_class(&matching, &args));
        assert!(!matches_requested_ingress_class(&wrong_controller, &args));
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
            Some("^/v1\\+beta".to_string())
        );
        assert_eq!(path_to_regex("Prefix", Some("/")).unwrap(), None);
        assert!(path_to_regex("Unknown", Some("/")).is_err());
    }
}
