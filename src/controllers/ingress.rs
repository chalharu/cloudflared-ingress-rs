use std::{
    collections::{HashMap, HashSet},
    fmt::Debug,
    sync::{Arc, Mutex},
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
    cli::ControllerArgs, controllers::cloudflared::CloudflaredTunnelIngress, Error, Result,
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

    // tokio::join!(
    //     run_controller::<Ingress>(client.clone(), context.clone()),
    //     run_controller::<IngressClass>(client.clone(), context.clone()),
    // );
    Ok(())
}

async fn get_ingress_classes(client: &Client, args: &ControllerArgs) -> Result<Vec<IngressClass>> {
    let ingress_class_api = Api::<IngressClass>::all(client.clone());
    let ingress_class = if let Some(ingress_class) = args.ingress_class() {
        ingress_class_api
            .get(ingress_class)
            .await
            .ok()
            .filter(|ic| {
                ic.spec.as_ref().map_or(false, |s| {
                    s.controller.as_ref().map_or(false, |c| c == ingress_class)
                })
            })
            .into_iter()
            .collect()
    } else {
        ingress_class_api
            .list(&ListParams::default())
            .await?
            .items
            .into_iter()
            .filter(|ic| {
                ic.spec
                    .as_ref()
                    .and_then(|s| s.controller.as_ref())
                    .map_or(false, |c| c == args.ingress_controller())
            })
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

// Context for our reconciler
#[derive(Clone)]
struct Context {
    /// Kubernetes client
    client: Client,
    args: ControllerArgs,
    target_ingressclass: Arc<Mutex<HashMap<Option<String>, ObjectRef<PartialIngressClass>>>>,
}

async fn run_controller(client: Client, context: Arc<Context>) {
    info!("Starting controller for Ingress");

    let api_ingressclass = Api::<IngressClass>::all(client.clone());
    let api_ingress = Api::<Ingress>::all(client);
    let (reader_ingressclass, writer_ingressclass) = reflector::store();
    let (_reader_ingress, writer_ingress) = reflector::store();

    // controller main stream from metadata_watcher
    let stream_ingressclass = metadata_watcher(api_ingressclass, Config::default())
        .default_backoff()
        .reflect(writer_ingressclass)
        .applied_objects();

    let stream_ingress = watcher(api_ingress, Config::default())
        .default_backoff()
        .reflect(writer_ingress)
        .touched_objects();

    let context_in = context.clone();
    Controller::for_stream(stream_ingressclass, reader_ingressclass)
        .watches_stream(stream_ingress, move |i| {
            let context = context_in.clone();
            i.spec.and_then(|is| {
                context
                    .target_ingressclass
                    .lock()
                    .unwrap()
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

        let mut current_ic: HashSet<_> = self
            .target_ingressclass
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect();

        for ic in ingress_class.iter() {
            let is_default_class = ic
                .meta()
                .annotations
                .as_ref()
                .and_then(|a| a.get("ingressclass.kubernetes.io/is-default-class"))
                .map_or(false, |x| x.to_lowercase() == "true");
            let name = ic.name_any();

            let obj_ref =
                reflector::Lookup::to_object_ref(&ic.metadata.clone().into_request_partial(), ());
            if is_default_class {
                current_ic.remove(&None);
                self.target_ingressclass
                    .lock()
                    .unwrap()
                    .insert(None, obj_ref.clone());
            }
            current_ic.remove(&Some(name.clone()));
            self.target_ingressclass
                .lock()
                .unwrap()
                .insert(Some(name.clone()), obj_ref.clone());
        }

        for ic in current_ic {
            self.target_ingressclass.lock().unwrap().remove(&ic);
        }

        for ic in ingress_class {
            let is_default_class = ic
                .meta()
                .annotations
                .as_ref()
                .and_then(|a| a.get("ingressclass.kubernetes.io/is-default-class"))
                .map_or(false, |x| x.to_lowercase() == "true");

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

        let ingresses = get_ingresses(&self.client, &ic.name_any(), is_default_class).await?;
        let name = ic.name_any();
        let owner_ref = ic.controller_owner_ref(&());

        let mut cfdt_ingress = Vec::new();

        let cfdt_api = Api::<CloudflaredTunnel>::namespaced(
            self.client.clone(),
            self.args.cloudflare_tunnel_namespace(),
        );
        let services: HashMap<_, _> = get_services(&self.client)
            .await?
            .into_iter()
            .map(|s| {
                let svc_name = format!("{}.{}.svc", s.name_any(), s.namespace().unwrap());
                let ports: HashMap<_, _> = s
                    .spec
                    .iter()
                    .flat_map(|s| {
                        s.ports.iter().flat_map(|p| {
                            p.iter()
                                .flat_map(|p| p.name.as_ref().map(|n| (n.clone(), p.port)))
                        })
                    })
                    .collect();
                (svc_name, ports)
            })
            .collect();

        for i in ingresses.into_iter() {
            let scheme = i
                .annotations()
                .get(SERVERSSCHEME_ANNOTATION)
                .map(String::as_str)
                .unwrap_or("http")
                .to_lowercase();

            let ns = i.namespace().unwrap();

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
                    let svc_name = format!("{}.{}.svc", service.name, ns);
                    let port = service
                        .port
                        .as_ref()
                        .and_then(|p| {
                            p.number.or_else(|| {
                                p.name.as_ref().and_then(|p_name| {
                                    services
                                        .get(&svc_name)
                                        .and_then(|svc| svc.get(p_name).cloned())
                                })
                            })
                        })
                        .filter(|&x| {
                            !(x == 80 && scheme == "http" || x == 443 && scheme == "https")
                        });
                    let cfdt_service = if let Some(port) = port {
                        format!("{}://{}:{}", scheme, svc_name, port)
                    } else {
                        format!("{}://{}", scheme, svc_name)
                    };

                    let path = match p.path_type.as_str() {
                        "Exact" => Some(format!(
                            "^{}$",
                            p.path
                                .as_ref()
                                .map(|x| regex_escape(x.to_string()))
                                .unwrap_or_else(|| "/".to_string())
                        )),
                        "Prefix" | "ImplementationSpecific" => p
                            .path
                            .as_ref()
                            .filter(|x| x.as_str() != "/")
                            .map(|x| format!("^{}", regex_escape(x.to_string()))),
                        _ => return Err(Error::illegal_document()),
                    };

                    cfdt_ingress.push(CloudflaredTunnelIngress {
                        // Hostなしは最終的にCNAMEが振れないことからエラーとする
                        hostname: r.host.clone().ok_or_else(Error::illegal_document)?,
                        service: cfdt_service,
                        path,
                        origin_request: None,
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
                &PatchParams::apply(PATCH_PARAMS_APPLY_NAME),
                &Patch::Apply(cfd),
            )
            .await?;
        Ok(())
    }
}

fn regex_escape(s: String) -> String {
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
