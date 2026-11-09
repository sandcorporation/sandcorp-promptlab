# syntax=docker/dockerfile:1
# 러너 이미지(cpp) — 하네스 실행기(promptlab-runner, musl 정적)와 언어 런타임만 든다. 비밀 없음(GHCR 공개).
# 빌드: bash scripts/build-runners.sh (저장소 루트가 문맥). 판은 crates/shared/src/coding/mod.rs 의 RUNNER_TAG.
# 판정기·앱은 --read-only·--tmpfs /work·--user 65534:65534·--network none 으로 띄운다(promptlab_judge::docker_run_args).

FROM rust:1.98-alpine AS runner
RUN apk add --no-cache musl-dev
WORKDIR /src
# 실행기는 워크스페이스 상속 없이 따로 구워진다 — 판은 저장소 Cargo.lock 그대로.
COPY Cargo.lock ./
COPY crates/runner/ ./
RUN --mount=type=cache,sharing=locked,target=/usr/local/cargo/registry \
    cargo build --release && strip target/release/promptlab-runner

FROM alpine:3.22
RUN apk add --no-cache g++
COPY --from=runner /src/target/release/promptlab-runner /usr/local/bin/promptlab-runner
ENV PROMPTLAB_RUNNER_LANG=cpp
WORKDIR /work
USER 65534:65534
ENTRYPOINT ["/usr/local/bin/promptlab-runner"]
