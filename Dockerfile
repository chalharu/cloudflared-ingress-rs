FROM alpine:3.23 AS rust-base

ARG RUST_VERSION=1.94.0

RUN apk -U --no-cache add cargo-chef gcc musl-dev rustup sccache

RUN rustup-init -y --profile minimal --default-toolchain ${RUST_VERSION}
ENV PATH="/root/.cargo/bin:$PATH" \
    RUSTUP_TOOLCHAIN="${RUST_VERSION}" \
    CARGO_HOME=/var/cache/cargo \
    SCCACHE_DIR=/var/cache/sccache \
    RUSTC_WRAPPER=sccache \
    CARGO_INCREMENTAL=0 \
    BUILDDIR=/source

WORKDIR ${BUILDDIR}

FROM rust-base AS planner

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo chef prepare --recipe-path recipe.json

FROM rust-base AS dependencies

COPY --from=planner ${BUILDDIR}/recipe.json recipe.json

RUN --mount=type=cache,target=${CARGO_HOME} \
    --mount=type=cache,target=${SCCACHE_DIR} \
    --mount=type=cache,target=${BUILDDIR}/target \
    cargo chef cook --release --recipe-path recipe.json

FROM dependencies AS build

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN --mount=type=cache,target=${CARGO_HOME} \
    --mount=type=cache,target=${SCCACHE_DIR} \
    --mount=type=cache,target=${BUILDDIR}/target \
    cargo build --release && \
    sccache --show-stats && \
    cp ${BUILDDIR}/target/release/cloudflared-ingress-rs /

RUN strip /cloudflared-ingress-rs

FROM scratch
WORKDIR /
COPY --from=build /cloudflared-ingress-rs ./
ENTRYPOINT [ "/cloudflared-ingress-rs" ]
