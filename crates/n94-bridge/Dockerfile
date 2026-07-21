FROM rust:trixie AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app/src

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
# Share the target dir between `cargo chef cook` and `cargo install` so the
# cooked dependency artifacts are reused for the final build.
ENV CARGO_TARGET_DIR=/app/src/target
COPY --from=planner /app/src/recipe.json recipe.json
# Build dependencies only. This layer is keyed on recipe.json (Cargo.toml +
# Cargo.lock), so CI layer caching reuses it for any commit that only touches
# source code. Cook only n94-bridge's deps: other workspace members need
# ffmpeg headers which this image doesn't have.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=shared \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo chef cook --release -p n94-bridge --recipe-path recipe.json
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=shared \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo install --path ./crates/n94-bridge --root /app/build

FROM rust:trixie
WORKDIR /app
COPY --from=build /app/build .
ENTRYPOINT ["/app/bin/n94-bridge"]
