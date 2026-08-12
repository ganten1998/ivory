# Build context MUST be the repo root — this crate depends on ../../ivory-core
# by path, so a context of tools/ivory-fulfil cannot see it:
#   docker build -f tools/ivory-fulfil/Dockerfile -t ivory-fulfil .
FROM rust:1-slim AS build
WORKDIR /src
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY ivory-core ./ivory-core
COPY tools/ivory-fulfil ./tools/ivory-fulfil
WORKDIR /src/tools/ivory-fulfil
RUN cargo build --release

FROM debian:stable-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/tools/ivory-fulfil/target/release/ivory-fulfil /usr/local/bin/ivory-fulfil
# Persist this: it is the record of every key ever issued.
VOLUME ["/data"]
ENV LEDGER_PATH=/data/ledger.jsonl PORT=8080
EXPOSE 8080
CMD ["ivory-fulfil"]
