# The context holds the release's binary for each architecture, and BuildKit
# resolves TARGETARCH to the platform it is building, so nothing is compiled
# here.

# A carrier image: no runtime, the binary is taken out with `COPY --from` or
# `docker cp`.
FROM scratch AS bin
ARG TARGETARCH
COPY linux/$TARGETARCH/sbxtkt /usr/local/bin/sbxtkt
# `docker create` takes its command from the image, and a scratch one brings none.
CMD ["/usr/local/bin/sbxtkt"]

# fd, rg, uv, deno and tusd come out of the binary itself, so only the rest of a
# workable environment is installed here.
FROM debian:bookworm-slim AS latest

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        curl \
        git \
        jq \
        less \
        openssh-client \
        procps \
        unzip \
        vim-tiny \
        xz-utils \
        zip \
    && rm -rf /var/lib/apt/lists/*

COPY --from=bin /usr/local/bin/sbxtkt /usr/local/bin/sbxtkt

EXPOSE 3000

ENTRYPOINT ["sbxtkt"]
# The CLI binds loopback by default, which a published port cannot reach.
CMD ["serve", "--host", "0.0.0.0"]
