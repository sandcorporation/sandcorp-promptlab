#!/usr/bin/env bash
# PR 검사 = 로컬 검사. fmt → clippy → 빌드 → 시험(시험 DB 포함)을 docker 안에서 돌린다.
# 빌드 노드(home·macos)에 Rust·webkit2gtk 가 없어도 docker 만 있으면 된다.
#
# - home 노드는 Jenkins 컨테이너가 호스트 docker 소켓을 쓰므로 작업 공간을 bind mount 하지 않는다.
#   소스는 tar 로 컨테이너 stdin 에 흘려 넣는다.
# - cargo 레지스트리·target 은 named volume(promptlab-ci-cargo, promptlab-ci-target)에 남겨 두 번째부터 빠르다.
# - 실제 모델 시험(#[ignore] — 재현성·AI 끝에서 끝)은 CI에서 돌리지 않는다(#98): 7B 모델이 4.47GiB 받기·
#   시험 60초·최대 RSS 7.7GiB라 빌드 노드에 무겁다. 모델·inference·llama.cpp 판을 바꾸는 PR은 로컬에서
#   `bash scripts/fetch-model.sh && cargo test --release -p promptlab-inference -- --ignored`를 돌려 PR에 적는다.
#   캐시를 비우려면: docker volume rm promptlab-ci-cargo promptlab-ci-target
# - 시험 DB: 실행마다 Postgres 컨테이너(promptlab-ci-pg-<pid>-<rand>)를 전용 네트워크에 띄우고 포트는 열지 않는다
#   (다른 저장소·동시 실행과 이름·포트가 겹치지 않는다). 데이터는 tmpfs, 끝나면 컨테이너·네트워크를 지운다.
#   CI 컨테이너는 DATABASE_URL 로 닿고, #[sqlx::test]가 시험마다 새 DB를 만든다.
set -euo pipefail

# - `--quick`(배포 잡 `Jenkinsfile`의 check, #140): fmt·.ps1 BOM만 본다 — 시험 DB를 띄우지 않고 컴파일하지 않는다.
#   main은 GitHub Actions(push: main)가 전부 검사하고, 컴파일은 바로 뒤 이미지 굽기가 한다. 빌드 노드(mac Docker 12.5GB)에서
#   전체 시험의 링크가 OOM(signal 9)으로 죽어 배포가 막혔다. PR 검사(`Jenkinsfile.pr`·Actions)는 인자 없이 전부 돈다.
# Windows PowerShell 5.1은 BOM 없는 UTF-8을 CP949로 읽어 한글 문자열 파서 오류가 난다 — 모든 .ps1은 BOM으로 시작해야 한다.
check_ps1_bom() {
  local f bad=0
  while IFS= read -r f; do
    [ "$(head -c3 "$f" | od -An -tx1 | tr -d ' \n')" = efbbbf ] || { echo "BOM 없음: $f" >&2; bad=1; }
  done < <(find . -name '*.ps1' -not -path './target/*' -not -path './.git/*')
  return $bad
}

quick=${PROMPTLAB_CI_QUICK:-0}
[ "${1:-}" = --quick ] && quick=1

steps() {
  local t0 t
  step() {
    local name=$1; shift
    t=$(date +%s)
    echo "== [$name] $*"
    "$@"
    echo "== [$name] 통과 ($(( $(date +%s) - t ))s)"
  }
  t0=$(date +%s)
  step fmt cargo fmt --all --check
  step ps1-bom check_ps1_bom
  if [ "$quick" = 1 ]; then echo "== 빠른 검사 통과 ($(( $(date +%s) - t0 ))s)"; return; fi
  step clippy cargo clippy --workspace --all-targets --locked -- -D warnings
  step build cargo build --workspace --locked
  step build-wasm cargo build -p promptlab-dashboard --target wasm32-unknown-unknown --locked
  step test cargo test --workspace --locked
  echo "== 전부 통과 ($(( $(date +%s) - t0 ))s)"
}

if [ "${PROMPTLAB_CI_INSIDE:-}" = 1 ]; then
  steps
  exit 0
fi

cd "$(dirname "$0")/../.."
rust_version=$(sed -n 's/^channel *= *"\(.*\)"/\1/p' rust-toolchain.toml)
image="promptlab-ci:rust-${rust_version}-$(cksum docker/ci.Dockerfile | cut -d' ' -f1)"
run_id="$$-$RANDOM"
container="promptlab-ci-$run_id"
pg="promptlab-ci-pg-$run_id"
net="promptlab-ci-net-$run_id"
pg_image="postgres:17-alpine"

if ! docker image inspect "$image" >/dev/null 2>&1; then
  echo "== CI 이미지 굽기: $image"
  docker build -q --build-arg RUST_VERSION="$rust_version" -t "$image" -f docker/ci.Dockerfile docker
fi

if [ "$quick" = 1 ]; then
  git ls-files -z --cached --others --exclude-standard \
    | while IFS= read -r -d '' f; do [ -e "$f" ] && printf '%s\0' "$f"; done \
    | COPYFILE_DISABLE=1 tar --null -T - -cf - \
    | docker run --rm -i --name "$container" -e PROMPTLAB_CI_INSIDE=1 -e PROMPTLAB_CI_QUICK=1 "$image" \
        bash -c 'tar -xf - -C /src && exec bash scripts/ci/check.sh'
  exit $?
fi

cleanup() {
  docker rm -f "$container" "$pg" >/dev/null 2>&1 || true
  docker network rm "$net" >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM

echo "== 시험 DB 띄우기: $pg ($pg_image)"
docker network create "$net" >/dev/null
# 시험 DB 비밀번호는 이 실행에만 쓰는 시험용 값이다(포트를 열지 않고 끝나면 지운다) — 운영 비밀이 아니다.
docker run -d --name "$pg" --network "$net" \
  --tmpfs /var/lib/postgresql/data \
  -e POSTGRES_PASSWORD=promptlab-ci \
  "$pg_image" -c fsync=off -c full_page_writes=off >/dev/null
# 초기화 중의 임시 서버는 TCP를 듣지 않는다 — TCP로 응답해야 진짜 준비된 것이다.
for i in $(seq 1 60); do
  docker exec "$pg" pg_isready -h 127.0.0.1 -U postgres -q && break
  [ "$i" = 60 ] && { echo "시험 DB가 60초 안에 뜨지 않았다"; docker logs "$pg" | tail -20; exit 1; }
  sleep 1
done

# 추적 중이거나 무시되지 않은 새 파일만 넣는다(target·.env 는 빠진다). 지운 추적 파일은 건너뛴다.
git ls-files -z --cached --others --exclude-standard \
  | while IFS= read -r -d '' f; do [ -e "$f" ] && printf '%s\0' "$f"; done \
  | COPYFILE_DISABLE=1 tar --null -T - -cf - \
  | docker run --rm -i --name "$container" \
      -v promptlab-ci-cargo:/usr/local/cargo/registry \
      -v promptlab-ci-target:/target \
      --network "$net" \
      -e DATABASE_URL="postgres://postgres:promptlab-ci@$pg:5432/postgres" \
      -e PROMPTLAB_CI_INSIDE=1 \
      "$image" \
      bash -c 'tar -xf - -C /src && exec bash scripts/ci/check.sh'
