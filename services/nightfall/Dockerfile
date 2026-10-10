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
ENV DUSK_GIT_REV=$GIT_REV
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    test -d vendor/capnproto/c++ || { echo "vendor/capnproto is empty: run git submodule update --init vendor/capnproto before docker build" >&2; exit 1; }; \
    test -n "$GIT_REV" || { echo "pass --build-arg GIT_REV=\$(git rev-parse HEAD)" >&2; exit 1; }; \
    ${CPUSET:+taskset --cpu-list "$CPUSET"} chrt -i 0 nice -n 19 cargo build --locked --release --package nightfall --bin nightfall \
    && mkdir -p /out/schemas \
    && cp target/release/nightfall /out/nightfall \
    && find services/nightfall/schemas -maxdepth 1 -type f \( -name '*.capnp.bin' -o -name '*.json' \) -exec cp {} /out/schemas/ \; \
    && CAPNP=$(find target/release/build -path '*/out/capnproto/bin/capnp' -type f -printf '%T@ %p\n' | sort -rn | head -n 1 | cut -d ' ' -f 2-) \
       sh services/nightfall/schemas/build.sh /out/schemas

FROM debian:trixie-20261005-slim
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --user-group --no-create-home --shell /usr/sbin/nologin nightfall
COPY --from=builder /out/nightfall /usr/local/bin/nightfall
COPY --from=builder /out/schemas /usr/share/nightfall/schemas
USER 10001:10001
EXPOSE 8443 8444 8445 9100
ENTRYPOINT ["/usr/local/bin/nightfall"]
