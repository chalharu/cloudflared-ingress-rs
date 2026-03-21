FROM alpine:3.22 AS build

ARG RUST_VERSION=1.94.0

RUN apk -U --no-cache add rustup gcc musl-dev

RUN rustup-init -y --profile minimal --default-toolchain ${RUST_VERSION}
ENV PATH="/root/.cargo/bin:$PATH" \
    RUSTUP_TOOLCHAIN="${RUST_VERSION}"

ARG BUILDDIR=/source
WORKDIR ${BUILDDIR}

ARG CARGO_HOME=/var/cache/cargo
ENV CARGO_HOME=${CARGO_HOME}


RUN --mount=type=bind,source=src,target=src \
    --mount=type=cache,target=${CARGO_HOME} \
    --mount=type=bind,source=Cargo.toml,target=Cargo.toml \
    cargo build --release && cp ${BUILDDIR}/target/release/cloudflared-ingress-rs /

RUN strip /cloudflared-ingress-rs

FROM scratch
WORKDIR /
COPY --from=build /cloudflared-ingress-rs ./
ENTRYPOINT [ "/cloudflared-ingress-rs" ]
