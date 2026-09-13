FROM docker.io/library/rust:nightly-trixie AS builder

WORKDIR /app/

COPY ./rust-toolchain.toml /app/rust-toolchain.toml
COPY ./.cargo/ /app/.cargo/

COPY ./benches/ /app/benches/
COPY ./src/ /app/src/
COPY ./Cargo.toml /app/Cargo.toml
COPY ./Cargo.lock /app/Cargo.lock
COPY ./LICENSE /app/LICENSE
COPY ./README.md /app/README.md

ARG FEATURES="default"
RUN cargo build --features "${FEATURES}" --locked --release && \
    cargo install \
        --features "${FEATURES}" --locked \
        --path . --root /app/ \
        --bin sbx

FROM gcr.io/distroless/cc-debian13

COPY --from=builder /app/bin/sbx /usr/bin/sbx
COPY --from=builder /app/LICENSE /usr/share/doc/sbx/LICENSE
COPY --from=builder /app/README.md /usr/share/doc/sbx/README.md

WORKDIR /mnt/data/

ENV SBX_ROOT_DIR=/mnt/data/
ENV SBX_SHADOW_FILE=/etc/sbx/shadow:rw
ENV SBX_LOG=info

EXPOSE 8080/tcp 8443/tcp
VOLUME /mnt/data/

ENTRYPOINT ["/usr/bin/sbx"]
