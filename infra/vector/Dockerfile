FROM python:3.14.8-slim-trixie AS contracts
WORKDIR /build
COPY services/contracts/kafka contracts
COPY infra/vector/lake lake
COPY infra/vector/bundle_schemas.py infra/vector/tests/generate_tests.py ./
RUN python bundle_schemas.py contracts /out/contracts \
    && python generate_tests.py contracts lake /out/tests

FROM timberio/vector:0.59.0-debian AS configured
COPY infra/vector/vector.yaml infra/vector/transforms.yaml /etc/vector/
COPY infra/vector/remaps /etc/vector/remaps
COPY infra/vector/lake /etc/vector/lake
COPY --from=contracts /out/contracts /etc/vector/contracts

FROM configured AS test
COPY infra/vector/tests/sources.yaml infra/vector/tests/contracts.yaml infra/vector/tests/otel.yaml /etc/vector/tests/
COPY --from=contracts /out/tests /etc/vector/tests/generated
RUN vector test /etc/vector/transforms.yaml /etc/vector/tests/*.yaml /etc/vector/tests/generated/*.json \
    && touch /etc/vector/tested

FROM configured
COPY --from=test /etc/vector/tested /etc/vector/tested
RUN mkdir -p /var/lib/vector && chown 65534:65534 /var/lib/vector
USER 65534:65534
ENV VECTOR_LOG_FORMAT=json \
    VECTOR_DANGEROUSLY_ALLOW_ENV_VAR_INTERPOLATION=true
EXPOSE 8686 9598
ENTRYPOINT ["/usr/bin/vector"]
CMD ["--config", "/etc/vector/vector.yaml", "--config", "/etc/vector/transforms.yaml"]
