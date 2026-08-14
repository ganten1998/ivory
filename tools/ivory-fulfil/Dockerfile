# Build context MUST be the repo root — this crate depends on ../../ivory-core
# by path, so a context of tools/ivory-fulfil cannot see it:
#   docker build -f tools/ivory-fulfil/Dockerfile -t ivory-fulfil .
FROM rust:1-slim AS build
WORKDIR /src
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*
# ivory-core inherits version/edition/license/rust-version/lints from the root
# workspace manifest, so that manifest must be here or cargo fails with
# "failed to find a workspace root". A workspace will not load with a member
# missing, so EVERY member the root lists has to be copied — hence the GUI
# crates, purely so the manifest parses. They are never built: tools/ivory-fulfil
# declares its own empty [workspace], which is what keeps it standalone.
#
# If you add a workspace member, add it here. The failure is at the cargo step,
# not the COPY, and it reads as a workspace error rather than a missing file.
COPY Cargo.toml ./
COPY ivory-core ./ivory-core
COPY ivory-ui ./ivory-ui
COPY ivory ./ivory
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
