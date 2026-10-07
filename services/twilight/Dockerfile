FROM node:22.23.3-trixie-slim AS web
WORKDIR /build/web
COPY services/twilight/web/package.json services/twilight/web/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm npm ci --no-audit --no-fund
COPY services/twilight/web ./
ARG CPUSET=
RUN rm -rf dist && ${CPUSET:+taskset --cpu-list "$CPUSET"} chrt -i 0 nice -n 19 npm run build

FROM golang:1.27.1-trixie AS build
WORKDIR /build/twilight
COPY services/twilight/go.mod services/twilight/go.sum ./
RUN --mount=type=cache,target=/go/pkg/mod go mod download
COPY services/twilight ./
COPY --from=web /build/web/dist ./web/dist
ARG GIT_REV
ARG VERSION=dev
ARG CPUSET=
RUN --mount=type=cache,target=/go/pkg/mod \
    --mount=type=cache,target=/root/.cache/go-build \
    test -n "$GIT_REV" || { echo "pass --build-arg GIT_REV=\$(git rev-parse HEAD)" >&2; exit 1; }; \
    CGO_ENABLED=0 ${CPUSET:+taskset --cpu-list "$CPUSET"} chrt -i 0 nice -n 19 go build -trimpath -tags ui \
        -ldflags "-s -w -X main.version=${VERSION} -X main.revision=${GIT_REV}" \
        -o /out/twilight ./cmd/twilight

FROM gcr.io/distroless/static-debian13:nonroot@sha256:e2e927ec666bae08560abb3c55d0659eceabb657f56b6782ab500a9fc7f555e3
COPY --from=build /out/twilight /usr/local/bin/twilight
USER 65532:65532
EXPOSE 8080 9102
ENTRYPOINT ["/usr/local/bin/twilight"]
CMD ["serve"]
