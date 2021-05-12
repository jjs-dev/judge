FROM lukemathwalker/cargo-chef as build-plan
WORKDIR /app
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM lukemathwalker/cargo-chef as cache
WORKDIR /app
COPY --from=build-plan /app/recipe.json recipe.json
ARG RELEASE=""
RUN cargo chef cook ${RELEASE} --recipe-path recipe.json

FROM rust as build
WORKDIR /app
COPY . .
COPY --from=cache /app/target target
COPY --from=cache $CARGO_HOME $CARGO_HOME
ARG RELEASE=""
ENV RUSTC_BOOTSTRAP=1
RUN cargo build ${RELEASE} -Zunstable-options --out-dir ./out

FROM gcr.io/distroless/cc
COPY --from=build /app/out/judge /usr/local/bin/judge
ENV RUST_LOG=info
ENTRYPOINT ["/usr/local/bin/judge"]