FROM rust:1.99.0-slim-trixie AS builder
RUN apt-get update \
    && apt-get install --yes --no-install-recommends autoconf automake cmake g++ libtool make perl pkg-config python3 \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY rust-toolchain.toml ./
RUN rustup toolchain install
COPY . .
ARG GIT_REV
ARG CARGO_BUILD_JOBS=default
ARG CPUSET=
ARG FLEET
ARG PROVISION
ENV DUSK_GIT_REV=$GIT_REV \
    DUSK_NODE_INIT_SCRIPT="nightfall -c ${FLEET} --provision ${PROVISION} --ca /etc/dusk/fleet-server-ca.pem" \
    DUSK_NODE_KVS_PERSISTENT=/var/lib/dusk/kvs
RUN --mount=type=secret,id=fleet-token,env=DUSK_FLEET_TOKEN \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    test -d vendor/capnproto/c++ || { echo "vendor/capnproto is empty: run git submodule update --init vendor/capnproto before docker build" >&2; exit 1; }; \
    test -n "$GIT_REV" || { echo "pass --build-arg GIT_REV=\$(git rev-parse HEAD)" >&2; exit 1; }; \
    test -n "$FLEET" && test -n "$PROVISION" || { echo "pass --build-arg FLEET=<host:port> and --build-arg PROVISION=<host:port>" >&2; exit 1; }; \
    test -n "$DUSK_FLEET_TOKEN" || { echo "pass the fleet token as a build secret: --secret id=fleet-token,env=DUSK_FLEET_TOKEN" >&2; exit 1; }; \
    ${CPUSET:+taskset --cpu-list "$CPUSET"} chrt -i 0 nice -n 19 cargo build --locked --release --package dusk_node_bin --bin dusk_node \
    && cp target/release/dusk_node /dusk_node

FROM debian:trixie-20261005-slim
RUN useradd --system --uid 10001 --user-group --no-create-home --shell /usr/sbin/nologin dusk \
    && install --directory --owner 10001 --group 10001 --mode 0700 /var/lib/dusk
COPY --from=builder /dusk_node /usr/local/bin/dusk_node
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/dusk_node"]
