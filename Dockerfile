# syntax=docker/dockerfile:1.10
FROM rust:1.97.1-bookworm AS build
WORKDIR /source
COPY . .
RUN cargo build --profile dist --locked --package sandbox-cli --package sandbox-mcp --package sandboxd

FROM docker:29.6.2-cli AS docker-cli

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /var/lib/sandbox --create-home sandbox
COPY --from=docker-cli /usr/local/bin/docker /usr/local/bin/docker
COPY --from=build /source/target/dist/sandbox /usr/local/bin/sandbox
COPY --from=build /source/target/dist/sandbox-mcp /usr/local/bin/sandbox-mcp
COPY --from=build /source/target/dist/sandboxd /usr/local/bin/sandboxd
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/sandboxd"]
CMD ["--role", "all"]
