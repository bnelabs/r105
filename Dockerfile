# r105 native Rust image
# Usage:
#   docker build -t r105 .
#   docker run -it --rm r105 chat

FROM rust:1.88-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates bubblewrap \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/r105 /usr/local/bin/r105
RUN useradd --create-home --shell /bin/sh r105
USER r105
WORKDIR /home/r105
ENTRYPOINT ["r105"]
CMD ["chat"]
