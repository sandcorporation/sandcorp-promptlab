#!/usr/bin/env bash
# 후보 모델(GGUF)을 내려받아 sha256을 검증하고 캐시 디렉터리에 둔다. 이미 있고 해시가 맞으면 받지 않는다.
#
# 모델 정본은 crates/shared/src/inference.rs 의 `MODELS` 후보 표(#131, 큰 것부터) — 이 스크립트는 거기서 값을 읽는다.
# 캐시 디렉터리: $PROMPTLAB_MODEL_DIR → $XDG_CACHE_HOME/promptlab/models → ~/.cache/promptlab/models
# (promptlab_inference::model_dir() 와 같은 순서). 공개 모델이라 토큰이 필요 없다.
#
# 쓰기: bash scripts/fetch-model.sh [--model <id> | --all]   (기본 = 표의 첫 줄(가장 큰 후보), id 예: 7b · 1.5b)
#       → 받은 모델마다 마지막에 파일 경로를 한 줄씩 찍는다.
set -euo pipefail

cd "$(dirname "$0")/.."
pin=crates/shared/src/inference.rs

want=""
all=0
while [ $# -gt 0 ]; do
  case "$1" in
    --model) want=${2:?--model 뒤에 id}; shift 2 ;;
    --model=*) want=${1#--model=}; shift ;;
    --all) all=1; shift ;;
    *) echo "fetch-model: 모르는 인자 $1 (--model <id> | --all)" >&2; exit 2 ;;
  esac
done

# 표의 줄마다 "id repo file revision sha256" 한 줄.
rows=$(sed -n '/^pub const MODELS: \[ModelPin; [0-9]*\] = \[/,/^\];/p' "$pin" | awk '
  /ModelPin \{/ { id=repo=file=rev=sha="" }
  /^ *[a-z_0-9]+: "/ {
      key=$1; sub(/:$/, "", key); val=$0; sub(/^[^"]*"/, "", val); sub(/",$/, "", val)
      if (key=="id") id=val; else if (key=="hf_repo") repo=val; else if (key=="file") file=val
      else if (key=="revision") rev=val; else if (key=="sha256") sha=val }
  /^ *},$/ { print id, repo, file, rev, sha }')
[ -n "$rows" ] || { echo "fetch-model: $pin 에서 MODELS 를 못 읽었다" >&2; exit 1; }
ids=$(echo "$rows" | awk '{print $1}' | paste -sd' ')

if [ "$all" = 1 ]; then
  picked=$rows
elif [ -n "$want" ]; then
  picked=$(echo "$rows" | awk -v w="$want" '$1==w')
  [ -n "$picked" ] || { echo "fetch-model: 후보에 없는 모델 $want (후보: $ids)" >&2; exit 2; }
else
  picked=$(echo "$rows" | head -n1)
fi

dir=${PROMPTLAB_MODEL_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/promptlab/models}
mkdir -p "$dir"

fetch_one() {
  local id=$1 repo=$2 file=$3 revision=$4 sha256=$5
  for v in repo file revision sha256; do
    [ -n "${!v}" ] || { echo "fetch-model: $id 의 $v 를 못 읽었다" >&2; exit 1; }
  done
  local dest="$dir/$file"
  check() { echo "$sha256  $1" | sha256sum -c --status; }
  if [ -f "$dest" ] && check "$dest"; then
    echo "fetch-model: 캐시에 있음 ($id $file)" >&2
    echo "$dest"
    return 0
  fi
  local url="https://huggingface.co/$repo/resolve/$revision/$file"
  local part="$dest.part"
  echo "fetch-model: 내려받기 $id $repo@${revision:0:12} $file" >&2
  # 끊겼으면 이어 받는다(-C -). 실패하면 재시도한다.
  curl -fsSL --retry 5 --retry-delay 3 -C - -o "$part" "$url"
  if ! check "$part"; then
    rm -f "$part"
    echo "fetch-model: sha256 불일치 — 받은 파일을 버렸다 ($file)" >&2
    exit 1
  fi
  mv "$part" "$dest"
  echo "fetch-model: 검증 끝 ($id $file)" >&2
  echo "$dest"
}

while read -r id repo file revision sha256; do
  fetch_one "$id" "$repo" "$file" "$revision" "$sha256"
done <<< "$picked"
