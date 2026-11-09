# 서버·채점 워커 이미지(바이너리 둘, 같은 이미지). 판정기·소켓 프록시는 따로(docker/judge.Dockerfile·docker-proxy.Dockerfile). 개발 스택(compose.dev.yaml)이 굽고, 운영 배포(#5)도 이것을 쓴다.
# 빌드 문맥은 저장소 루트(.dockerignore 가 target·모델·.git 을 뺀다). BuildKit 캐시로 두 번째부터 빠르다.
# cargo 레지스트리 캐시(id=promptlab-cargo-registry)는 이 파일의 build·dashboard 단계와 judge·docker-proxy 이미지가 함께 쓴다 —
# BuildKit이 단계를 병렬로 돌리면 같은 크레이트 압축 풀기가 겹쳐 「failed to unpack package … File exists」로 깨졌다(#140).
# 그래서 레지스트리 마운트는 모두 sharing=locked(한 번에 한 RUN만): 단계별 id로 나누면 병렬은 되지만 레지스트리를
# 단계마다 따로 받고, 동시 컴파일이 빌드 노드(mac Docker 12.5GB) 메모리를 더 먹는다. target 캐시는 단계마다 따로다.
ARG RUST_VERSION=1.98.1
FROM rust:${RUST_VERSION}-slim-bookworm AS build
ARG RUST_VERSION
# cmake·make·g++·clang·libclang: 네이티브 의존성(C 빌드·bindgen) 빌드.
RUN apt-get update \
 && apt-get install -y --no-install-recommends pkg-config cmake make g++ clang libclang-dev \
 && rm -rf /var/lib/apt/lists/*
# rust-toolchain.toml 의 clippy·rustfmt·wasm32 는 이미지 빌드에 필요 없다 — 이미 깔린 툴체인을 쓴다.
ENV RUSTUP_TOOLCHAIN=${RUST_VERSION} CARGO_TARGET_DIR=/target CARGO_TERM_COLOR=never
WORKDIR /src
COPY . .
RUN --mount=type=cache,id=promptlab-cargo-registry,sharing=locked,target=/usr/local/cargo/registry \
    --mount=type=cache,id=promptlab-server-target,target=/target \
    cargo build --release --locked -p promptlab-server -p promptlab-worker \
 && mkdir -p /out \
 && cp /target/release/promptlab-server /target/release/promptlab-worker /out/

# 대시보드(Dioxus 웹, wasm32). dx 없이 cargo + wasm-bindgen(--target web)으로 굽고 crates/dashboard/index.html 을 곁들인다.
# sandcorp-design CSS는 wasm 안에 구워져 있고(include_str!), 로고·파비콘 svg만 /design/ 에 둔다(#41).
# 서버가 PROMPTLAB_DASHBOARD_DIR 에서 서빙한다. wasm-bindgen-cli 판은 Cargo.lock 의 wasm-bindgen 과 같아야 한다
# (다르면 wasm-bindgen 이 스키마 판 불일치로 멈춘다 — 그때 아래 ARG 를 올린다).
FROM rust:${RUST_VERSION}-slim-bookworm AS dashboard
ARG RUST_VERSION
ARG WASM_BINDGEN_VERSION=0.2.130
ENV RUSTUP_TOOLCHAIN=${RUST_VERSION} CARGO_TERM_COLOR=never
RUN rustup target add wasm32-unknown-unknown
RUN --mount=type=cache,id=promptlab-cargo-registry,sharing=locked,target=/usr/local/cargo/registry \
    --mount=type=cache,id=promptlab-wasm-bindgen-target,target=/wbg-target \
    CARGO_TARGET_DIR=/wbg-target cargo install --locked wasm-bindgen-cli --version ${WASM_BINDGEN_VERSION}
WORKDIR /src
COPY . .
RUN --mount=type=cache,id=promptlab-cargo-registry,sharing=locked,target=/usr/local/cargo/registry \
    --mount=type=cache,id=promptlab-dashboard-target,target=/target \
    CARGO_TARGET_DIR=/target cargo build --release --locked -p promptlab-dashboard --target wasm32-unknown-unknown \
 && mkdir -p /dist \
 && wasm-bindgen --target web --no-typescript --out-dir /dist \
      /target/wasm32-unknown-unknown/release/promptlab-dashboard.wasm \
 && cp crates/dashboard/index.html /dist/index.html \
 && mkdir -p /dist/design && cp design/*.svg design/VERSION /dist/design/

FROM debian:bookworm-slim
# curl: 배포 헬스 확인(Jenkinsfile healthCmd 가 서버 컨테이너 안에서 부른다).
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 promptlab
COPY --from=build /out/ /usr/local/bin/
# 대시보드 정적 파일(서버가 / 에서 서빙).
COPY --from=dashboard /dist/ /opt/promptlab/dashboard/
USER promptlab
ENV PROMPTLAB_ADDR=0.0.0.0:8080 PROMPTLAB_DASHBOARD_DIR=/opt/promptlab/dashboard
EXPOSE 8080
CMD ["promptlab-server"]
