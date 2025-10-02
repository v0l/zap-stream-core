FROM rust:bookworm AS build
WORKDIR /app/src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=shared \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/app/src/target,sharing=shared \
    cargo install --path ./crates/n94-bridge --root /app/build

FROM rust:bookworm
WORKDIR /app
COPY --from=build /app/build .
ENTRYPOINT ["/app/bin/n94-bridge"]