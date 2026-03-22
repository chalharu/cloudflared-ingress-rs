//! Shared error types used across the controllers and HTTP server.

use std::{convert::Infallible, num::TryFromIntError};

use snafu::{
    AsBacktrace, AsErrorSource, Backtrace, Error, ErrorCompat, GenerateImplicitData, IntoError,
    NoneError, Snafu,
};

/// Controller and runtime errors surfaced by the binary.
#[derive(Snafu, Debug)]
#[snafu(crate_root(crate::error))]
#[allow(clippy::enum_variant_names)]
pub enum ControllerError {
    #[snafu(display("SerializationError: {source}"))]
    SerializationError {
        #[snafu(source)]
        source: serde_json::Error,
        #[snafu(backtrace)]
        backtrace: Backtrace,
    },

    #[snafu(display("SerializationError: {source}"))]
    SerializationYamlError {
        #[snafu(source)]
        source: serde_yaml::Error,
        #[snafu(backtrace)]
        backtrace: Backtrace,
    },

    #[snafu(display("Kube Error: {source}"))]
    KubeError {
        #[snafu(source)]
        source: kube::Error,
        #[snafu(backtrace)]
        backtrace: Backtrace,
    },

    #[snafu(display("Finalizer Error: {source}"))]
    // NB: awkward type because finalizer::Error embeds the reconciler error (which is this)
    // so boxing this error to break cycles
    FinalizerError {
        #[snafu(source)]
        source: Box<kube::runtime::finalizer::Error<ControllerError>>,
        #[snafu(backtrace)]
        backtrace: Backtrace,
    },

    #[snafu(display("IllegalDocument"))]
    IllegalDocument {
        #[snafu(backtrace)]
        backtrace: Backtrace,
    },

    #[snafu(display("Cloudflare zone not found for hostname: {hostname}"))]
    CloudflareZoneNotFound {
        hostname: String,
        #[snafu(backtrace)]
        backtrace: Backtrace,
    },

    #[snafu(display("I/O Error: {source}"))]
    IoError {
        #[snafu(source)]
        source: std::io::Error,
        #[snafu(backtrace)]
        backtrace: Backtrace,
    },

    #[snafu(display("Cloudflare Framework Error: {source}"))]
    CloudflareFrameworkError {
        #[snafu(source)]
        source: cloudflare::framework::Error,
        #[snafu(backtrace)]
        backtrace: Backtrace,
    },

    #[snafu(display("Cloudflare API Error: {source}"))]
    CloudflareApiFailure {
        #[snafu(source)]
        source: Box<cloudflare::framework::response::ApiFailure>,
        #[snafu(backtrace)]
        backtrace: Backtrace,
    },

    #[snafu(display("Tokio join error: {source}"))]
    TokioJoinError {
        #[snafu(source)]
        source: tokio::task::JoinError,
        #[snafu(backtrace)]
        backtrace: Backtrace,
    },

    #[snafu(display("Base64 decode error: {source}"))]
    Base64DecodeError {
        #[snafu(source)]
        source: base64::DecodeError,
        #[snafu(backtrace)]
        backtrace: Backtrace,
    },

    #[snafu(display("Utf8 error: {source}"))]
    Utf8Error {
        #[snafu(source)]
        source: std::str::Utf8Error,
        #[snafu(backtrace)]
        backtrace: Backtrace,
    },

    #[snafu(display("From utf8 error: {source}"))]
    FromUtf8Error {
        #[snafu(source)]
        source: std::string::FromUtf8Error,
        #[snafu(backtrace)]
        backtrace: Backtrace,
    },

    #[snafu(display("Infallible error"))]
    InfallibleError {
        #[snafu(backtrace)]
        backtrace: Backtrace,
    },

    #[snafu(display("Convert from int error: {source}"))]
    TryFromIntError {
        #[snafu(source)]
        source: TryFromIntError,
        #[snafu(backtrace)]
        backtrace: Backtrace,
    },
}

impl From<serde_json::Error> for ControllerError {
    fn from(value: serde_json::Error) -> Self {
        SerializationSnafu.into_error(value)
    }
}

impl From<serde_yaml::Error> for ControllerError {
    fn from(value: serde_yaml::Error) -> Self {
        SerializationYamlSnafu.into_error(value)
    }
}

impl From<kube::Error> for ControllerError {
    fn from(value: kube::Error) -> Self {
        KubeSnafu.into_error(value)
    }
}

impl From<Box<kube::runtime::finalizer::Error<ControllerError>>> for ControllerError {
    fn from(value: Box<kube::runtime::finalizer::Error<ControllerError>>) -> Self {
        FinalizerSnafu.into_error(value)
    }
}

impl From<std::io::Error> for ControllerError {
    fn from(value: std::io::Error) -> Self {
        IoSnafu.into_error(value)
    }
}

impl From<cloudflare::framework::Error> for ControllerError {
    fn from(value: cloudflare::framework::Error) -> Self {
        CloudflareFrameworkSnafu.into_error(value)
    }
}

impl From<cloudflare::framework::response::ApiFailure> for ControllerError {
    fn from(value: cloudflare::framework::response::ApiFailure) -> Self {
        CloudflareApiFailureSnafu.into_error(Box::new(value))
    }
}

impl From<tokio::task::JoinError> for ControllerError {
    fn from(value: tokio::task::JoinError) -> Self {
        TokioJoinSnafu.into_error(value)
    }
}

impl From<base64::DecodeError> for ControllerError {
    fn from(value: base64::DecodeError) -> Self {
        Base64DecodeSnafu.into_error(value)
    }
}

impl From<std::str::Utf8Error> for ControllerError {
    fn from(value: std::str::Utf8Error) -> Self {
        Utf8Snafu.into_error(value)
    }
}

impl From<std::string::FromUtf8Error> for ControllerError {
    fn from(value: std::string::FromUtf8Error) -> Self {
        FromUtf8Snafu.into_error(value)
    }
}

impl From<Infallible> for ControllerError {
    fn from(_: Infallible) -> Self {
        InfallibleSnafu.into_error(NoneError)
    }
}

impl From<TryFromIntError> for ControllerError {
    fn from(value: TryFromIntError) -> Self {
        TryFromIntSnafu.into_error(value)
    }
}

impl ControllerError {
    pub fn illegal_document() -> Self {
        IllegalDocumentSnafu.build()
    }

    pub fn cloudflare_zone_not_found(hostname: impl Into<String>) -> Self {
        CloudflareZoneNotFoundSnafu {
            hostname: hostname.into(),
        }
        .build()
    }

    pub fn metric_label(&self) -> &'static str {
        match self {
            Self::SerializationError { .. } => "serialization_error",
            Self::SerializationYamlError { .. } => "serialization_yaml_error",
            Self::KubeError { .. } => "kube_error",
            Self::FinalizerError { .. } => "finalizer_error",
            Self::IllegalDocument { .. } => "illegal_document",
            Self::CloudflareZoneNotFound { .. } => "cloudflare_zone_not_found",
            Self::IoError { .. } => "io_error",
            Self::CloudflareFrameworkError { .. } => "cloudflare_framework_error",
            Self::CloudflareApiFailure { .. } => "cloudflare_api_failure",
            Self::TokioJoinError { .. } => "tokio_join_error",
            Self::Base64DecodeError { .. } => "base64_decode_error",
            Self::Utf8Error { .. } => "utf8_error",
            Self::FromUtf8Error { .. } => "from_utf8_error",
            Self::InfallibleError { .. } => "infallible_error",
            Self::TryFromIntError { .. } => "try_from_int_error",
        }
    }
}

pub type Result<T, E = ControllerError> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use std::convert::TryFrom as _;

    use base64::Engine as _;

    use super::*;

    #[test]
    fn illegal_document_constructor_returns_expected_variant_and_label() {
        let error = ControllerError::illegal_document();

        assert!(matches!(error, ControllerError::IllegalDocument { .. }));
        assert_eq!(error.metric_label(), "illegal_document");
    }

    #[test]
    fn cloudflare_zone_not_found_constructor_returns_expected_variant_and_label() {
        let error = ControllerError::cloudflare_zone_not_found("api.other.com");

        assert!(matches!(
            error,
            ControllerError::CloudflareZoneNotFound { ref hostname, .. } if hostname == "api.other.com"
        ));
        assert_eq!(error.metric_label(), "cloudflare_zone_not_found");
    }

    #[test]
    fn conversion_impls_wrap_common_source_errors() {
        let json_error = ControllerError::from(
            serde_json::from_str::<serde_json::Value>("{").expect_err("json should fail"),
        );
        let yaml_error = ControllerError::from(
            serde_yaml::from_str::<serde_yaml::Value>("value: [").expect_err("yaml should fail"),
        );
        let io_error = ControllerError::from(std::io::Error::other("io"));
        let decode_error = ControllerError::from(
            base64::engine::general_purpose::STANDARD
                .decode("%%%")
                .expect_err("base64 should fail"),
        );
        let invalid_utf8 = vec![0xff];
        let utf8_error = ControllerError::from(
            std::str::from_utf8(&invalid_utf8).expect_err("utf8 should fail"),
        );
        let from_utf8_error =
            ControllerError::from(String::from_utf8(vec![0xff]).expect_err("utf8 should fail"));
        let int_error =
            ControllerError::from(u8::try_from(1_000_i32).expect_err("conversion should fail"));

        assert!(matches!(
            json_error,
            ControllerError::SerializationError { .. }
        ));
        assert!(matches!(
            yaml_error,
            ControllerError::SerializationYamlError { .. }
        ));
        assert!(matches!(io_error, ControllerError::IoError { .. }));
        assert!(matches!(
            decode_error,
            ControllerError::Base64DecodeError { .. }
        ));
        assert!(matches!(utf8_error, ControllerError::Utf8Error { .. }));
        assert!(matches!(
            from_utf8_error,
            ControllerError::FromUtf8Error { .. }
        ));
        assert!(matches!(int_error, ControllerError::TryFromIntError { .. }));
    }

    #[test]
    fn infallible_metric_label_is_stable() {
        let error = ControllerError::InfallibleError {
            backtrace: Backtrace::generate(),
        };

        assert!(matches!(error, ControllerError::InfallibleError { .. }));
        assert_eq!(error.metric_label(), "infallible_error");
    }

    #[test]
    fn metric_label_uses_stable_variant_names() {
        let errors = [
            ControllerError::IllegalDocument {
                backtrace: Backtrace::generate(),
            },
            ControllerError::CloudflareZoneNotFound {
                hostname: "api.other.com".to_string(),
                backtrace: Backtrace::generate(),
            },
            ControllerError::IoError {
                source: std::io::Error::other("io"),
                backtrace: Backtrace::generate(),
            },
            ControllerError::InfallibleError {
                backtrace: Backtrace::generate(),
            },
            ControllerError::TryFromIntError {
                source: u8::try_from(1_000_i32).expect_err("conversion should fail"),
                backtrace: Backtrace::generate(),
            },
        ];

        let labels = errors
            .into_iter()
            .map(|error| error.metric_label())
            .collect::<Vec<_>>();

        assert_eq!(
            labels,
            vec![
                "illegal_document",
                "cloudflare_zone_not_found",
                "io_error",
                "infallible_error",
                "try_from_int_error",
            ]
        );
    }

    #[test]
    fn tokio_join_error_conversion_wraps_join_failures() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime should build");

        let join_error = runtime.block_on(async {
            tokio::spawn(async move {
                panic!("boom");
            })
            .await
            .expect_err("task should panic")
        });

        let error = ControllerError::from(join_error);

        assert!(matches!(error, ControllerError::TokioJoinError { .. }));
        assert_eq!(error.metric_label(), "tokio_join_error");
    }
}
