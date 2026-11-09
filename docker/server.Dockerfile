# 서버·채점 워커 이미지(바이너리 둘, 같은 이미지). 판정기·소켓 프록시는 따로(docker/judge.Dockerfile·docker-proxy.Dockerfile). 개발 스택(compose.dev.yaml)이 굽고, 운영 배포(#5)도 이것을 쓴다.
# 빌드 문맥은 저장소 루트(.dockerignore 가 target·모델·.git 을 뺀다).
#
# 의존성 층 분리(#10): cargo-chef 로 `prepare`(매니페스트·Cargo.lock 만 담은 recipe.json) → `cook`(의존성만 컴파일) → 소스 복사 → 워크스페이스 크레이트만 컴파일.
# recipe.json 은 소스만 바뀌면 그대로라 cook 층이 BuildKit 층 캐시에 그대로 적중한다. 층 캐시는 image.yml 의 registry 캐시(:buildcache, mode=max)로 실행기 사이에 이어진다.
# 예전에는 cargo 레지스트리·target 을 `--mount=type=cache` 에 뒀는데, 그 마운트는 registry 캐시에 실리지 않아 새 실행기마다 비어 있었다 —
# 그래서 마운트를 모두 걷고 레지스트리·target 이 층 안에 남게 했다. 레지스트리 마운트가 없으니 build·dashboard 단계가 같은 마운트를 나눠 쓰다
# 압축 풀기가 겹치던 문제(#140)와 그 때문의 sharing=locked 도 사라지고, 두 단계는 서로 기다리지 않고 병렬로 돈다.
# (judge·docker-proxy 이미지는 의존성이 작아 이번에는 그대로 둔다 — 그쪽 마운트는 sharing=locked 를 유지한다.)
ARG RUST_VERSION=1.98.1
FROM rust:${RUST_VERSION}-slim-bookworm AS chef
ARG RUST_VERSION
# cmake·make·g++·clang·libclang: 네이티브 의존성(C 빌드·bindgen) 빌드.
RUN apt-get update \
 && apt-get install -y --no-install-recommends pkg-config cmake make g++ clang libclang-dev \
 && rm -rf /var/lib/apt/lists/*
# rust-toolchain.toml 의 clippy·rustfmt·wasm32 는 이미지 빌드에 필요 없다 — 이미 깔린 툴체인을 쓴다.
ENV RUSTUP_TOOLCHAIN=${RUST_VERSION} CARGO_TARGET_DIR=/target CARGO_TERM_COLOR=never
ARG CARGO_CHEF_VERSION=0.1.78
RUN cargo install --locked cargo-chef --version ${CARGO_CHEF_VERSION}
WORKDIR /src

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path /recipe.json

FROM chef AS build
COPY --from=planner /recipe.json /recipe.json
RUN cargo chef cook --release --locked -p promptlab-server -p promptlab-worker --recipe-path /recipe.json
COPY . .
RUN cargo build --release --locked -p promptlab-server -p promptlab-worker \
 && mkdir -p /out \
 && cp /target/release/promptlab-server /target/release/promptlab-worker /out/

# 대시보드(Dioxus 웹, wasm32). dx 없이 cargo + wasm-bindgen(--target web)으로 굽고 crates/dashboard/index.html 을 곁들인다.
# sandcorp-design CSS는 wasm 안에 구워져 있고(include_str!), 로고·파비콘 svg만 /design/ 에 둔다(#41).
# 서버가 PROMPTLAB_DASHBOARD_DIR 에서 서빙한다. wasm-bindgen-cli 판은 Cargo.lock 의 wasm-bindgen 과 같아야 한다
# (다르면 wasm-bindgen 이 스키마 판 불일치로 멈춘다 — 그때 아래 ARG 를 올린다).
FROM chef AS wbg
ARG WASM_BINDGEN_VERSION=0.2.130
RUN cargo install --locked wasm-bindgen-cli --version ${WASM_BINDGEN_VERSION} --root /wbg

FROM chef AS dashboard-cook
RUN rustup target add wasm32-unknown-unknown
COPY --from=planner /recipe.json /recipe.json
RUN cargo chef cook --release --locked --target wasm32-unknown-unknown -p promptlab-dashboard --recipe-path /recipe.json

FROM dashboard-cook AS dashboard
COPY --from=wbg /wbg/bin/wasm-bindgen /usr/local/bin/wasm-bindgen
COPY . .
RUN cargo build --release --locked -p promptlab-dashboard --target wasm32-unknown-unknown \
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
