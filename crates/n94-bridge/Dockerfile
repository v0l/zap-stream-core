FROM rust:bookworm AS build
WORKDIR /app/src
COPY . .
RUN cargo install --path ./crates/n94-bridge --root /app/build

FROM rust:bookworm
WORKDIR /app
COPY --from=build /app/build .
ENTRYPOINT ["/app/bin/n94-bridge"]