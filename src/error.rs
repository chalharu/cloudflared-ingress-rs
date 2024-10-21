use std::num::TryFromIntError;

use snafu::{
    AsBacktrace, AsErrorSource, Backtrace, Error, ErrorCompat, GenerateImplicitData, IntoError,
    NoneError, Snafu,
};

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

    #[snafu(display("Rand error: {source}"))]
    RandError {
        #[snafu(source)]
        source: rand::Error,
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

impl From<rand::Error> for ControllerError {
    fn from(value: rand::Error) -> Self {
        RandSnafu.into_error(value)
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
}

pub type Result<T, E = ControllerError> = std::result::Result<T, E>;

impl ControllerError {
    pub fn metric_label(&self) -> String {
        format!("{self:?}").to_lowercase()
    }
}
