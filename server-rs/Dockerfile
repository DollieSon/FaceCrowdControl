# Build stage: compile a release binary.
FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src src
COPY migrations migrations
COPY games.toml games.toml
RUN cargo build --release --locked

# Runtime stage: just the binary (SQLite is compiled in; no TLS needed for a local Postgres).
FROM debian:bookworm-slim
RUN useradd --system --create-home fcc
WORKDIR /app
COPY --from=build /src/target/release/fcc-server /usr/local/bin/fcc-server
COPY games.toml /app/games.toml
RUN mkdir -p /app/data && chown fcc /app/data
USER fcc

ENV BIND_ADDRESS=0.0.0.0 \
    PORT=3000 \
    DATABASE_URL=sqlite://data/fcc.db
VOLUME /app/data
EXPOSE 3000
CMD ["fcc-server"]
