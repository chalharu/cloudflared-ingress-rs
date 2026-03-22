//! Kubernetes controllers managed by this binary.

pub mod cloudflared;
pub mod ingress;

#[cfg(test)]
pub(crate) mod test_support;
