FROM rust:1.99.0-slim-trixie AS wheel
RUN apt-get update \
    && apt-get install --yes --no-install-recommends autoconf automake cmake g++ libtool make perl pkg-config python3 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=ghcr.io/astral-sh/uv:0.12.23 /uv /usr/local/bin/uv
ENV UV_PYTHON_INSTALL_DIR=/opt/python
RUN uv python install 3.14.8
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
    --mount=type=cache,target=/root/.cache/uv \
    test -d vendor/capnproto/c++ || { echo "vendor/capnproto is empty: run git submodule update --init vendor/capnproto before docker build" >&2; exit 1; }; \
    test -n "$GIT_REV" || { echo "pass --build-arg GIT_REV=\$(git rev-parse HEAD)" >&2; exit 1; }; \
    ${CPUSET:+taskset --cpu-list "$CPUSET"} chrt -i 0 nice -n 19 uv tool run --python 3.14.8 maturin@1.15.0 build --release --locked --compatibility linux \
        --interpreter "$(uv python find 3.14.8)" --out /out

FROM python:3.14.8-slim-trixie AS environment
COPY --from=ghcr.io/astral-sh/uv:0.12.23 /uv /usr/local/bin/uv
ENV UV_PYTHON=/usr/local/bin/python3.14 \
    UV_PYTHON_DOWNLOADS=never \
    UV_COMPILE_BYTECODE=1 \
    UV_LINK_MODE=copy
WORKDIR /build
COPY services/dawn/pyproject.toml services/dawn/uv.lock services/dawn/
RUN --mount=type=cache,target=/root/.cache/uv \
    uv export --project services/dawn --frozen --no-dev --no-emit-project --no-emit-package dusk --output-file /build/requirements.txt \
    && uv venv /opt/dawn \
    && uv pip install --python /opt/dawn/bin/python --require-hashes --requirement /build/requirements.txt
COPY --from=wheel /out /build/wheels
COPY services/dawn services/dawn
RUN --mount=type=cache,target=/root/.cache/uv \
    uv pip install --python /opt/dawn/bin/python --no-deps /build/wheels/*.whl \
    && uv pip install --python /opt/dawn/bin/python --no-deps ./services/dawn

FROM python:3.14.8-slim-trixie
RUN useradd --system --uid 10001 --user-group --no-create-home --shell /usr/sbin/nologin dawn
COPY --from=environment /opt/dawn /opt/dawn
ENV PATH=/opt/dawn/bin:$PATH \
    PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1
USER 10001:10001
EXPOSE 8443 9101
ENTRYPOINT ["/opt/dawn/bin/dawn"]
