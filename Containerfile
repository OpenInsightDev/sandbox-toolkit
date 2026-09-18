# syntax=docker/dockerfile:1

ARG DEBIAN_VERSION=13

FROM scratch AS bin

ARG TARGETARCH
ARG OCI_SOURCE=https://github.com/OpenInsightDev/sandbox-toolkit

LABEL org.opencontainers.image.title="sandbox-toolkit" \
    org.opencontainers.image.source="${OCI_SOURCE}" \
    org.opencontainers.image.description="Sandbox toolkit server (binaries only, for COPY --from)"

COPY --chmod=0755 dist/linux/${TARGETARCH}/sandbox-toolkit /sandbox-toolkit

FROM debian:${DEBIAN_VERSION}-slim AS runtime

ARG OCI_SOURCE=https://github.com/OpenInsightDev/sandbox-toolkit

LABEL org.opencontainers.image.title="sandbox-toolkit" \
    org.opencontainers.image.source="${OCI_SOURCE}" \
    org.opencontainers.image.description="Sandbox toolkit server with fd, ripgrep, jaq, uv and deno bundled"

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --user-group --shell /bin/bash --uid 1000 sandbox

COPY --from=bin /sandbox-toolkit /usr/local/bin/sandbox-toolkit

ENV HOME=/home/sandbox

WORKDIR /home/sandbox
USER sandbox

EXPOSE 8000
STOPSIGNAL SIGINT
ENTRYPOINT ["/usr/local/bin/sandbox-toolkit"]
CMD ["--bind", "0.0.0.0:8000"]
