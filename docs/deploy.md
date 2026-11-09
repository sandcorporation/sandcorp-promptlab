# 운영 배포 (oracle)

promptlab.sandcorp.co.kr — 다른 서비스와 같은 틀(GitHub Actions가 검사·이미지 굽기·`production` 승격, Jenkins `sandcorpDeploy`가 production만 oracle에 pull 배포 — 아래 「배포 흐름·되돌리기」).
파이프라인은 공용 라이브러리 `sandcorp`(jenkins-shared README)에 있고 이 저장소는 값만 넘긴다(`Jenkinsfile`).

## 배포 흐름·되돌리기(#8)

```
PR → Actions PR 검사 → main 병합
   → release.yml(push main): check(pr-check.yml workflow_call) → image(image.yml, 이미지 셋 matrix) → promote(production fast-forward)
   → Jenkins sandcorp-promptlab(*/production): 이미지 셋 <sha>가 ghcr에 있는지 확인(5분) → extraCheckouts(문제은행) → oracle 배포
```

- **운영 판 = `production` 브랜치 머리.** 사람이 직접 push하지 않는다 — `release.yml`의 승격 잡(`contents: write`는 이 잡만)이 `scripts/ci/promote.sh ff <sha>`로 앞으로만 옮긴다(강제 없음, 이미 더 새 판이면 건너뜀). 저장소 ruleset이 production의 강제 push·삭제를 막는다.
- 검사나 이미지 하나라도 실패하면 승격이 없고 운영은 그대로다. 실패한 잡을 다시 돌리면(같은 sha) 이어서 승격한다.
- `GITHUB_TOKEN`의 production push는 Actions를 다시 깨우지 않고 저장소 웹훅으로 Jenkins만 깨운다. main push도 Jenkins를 깨우지만 production이 그대로면 「이미 배포한 sha」로 건너뛴다.
- **되돌리기 — 정석**: main에 revert PR을 내 병합한다. 검사·이미지·승격·배포를 그대로 탄다(production이 앞으로 간다).
- **되돌리기 — 급할 때**: Jenkins 「Build with Parameters」에 `ROLLBACK_SHA=<옛 sha 40자>`. 빌드 없이 그 sha의 이미지 셋으로 배포한다(이미지가 ghcr에 남아 있어야 한다). production은 그대로라 다음 승격이 다시 앞으로 가니, 이어서 revert PR을 낸다. production을 옛 sha로 강제로 옮기는 워크플로는 두지 않는다(ruleset이 Actions를 예외로 받지 않는다).
- **Actions가 못 구울 때**: Jenkins `BUILD_ON=jenkins`(production 머리를 Jenkins 빌드 노드에서 굽고 배포).
- 숨은 문항 데이터는 Actions에 없다 — 검사는 공개 픽스처로 돌고, 운영 문항은 Jenkins 배포 단계에서만 받는다(아래 「문제은행 디렉터리」).

## 구성

| 파일 | 역할 |
|---|---|
| `.github/workflows/release.yml` | main push → 검사(`pr-check.yml` 재사용) → 이미지 셋(`image.yml`) → `production` fast-forward(`scripts/ci/promote.sh`) |
| `Jenkinsfile` | production 배포(`deployFrom: 'production'`). 잡 이름 `sandcorp-promptlab`, Script Path `Jenkinsfile`, 브랜치 지정자 `*/production`. 굽지도 검사하지도 않는다 — 비상 길 `BUILD_ON=jenkins`에서만 굽고 `check`(`scripts/ci/check.sh --quick`, fmt·.ps1 BOM만, #140)를 돈다 |
| `compose.oracle.yaml` | 프로젝트 `promptlab-prod`: `db`(promptlab 전용 Postgres 17) · `server` · `worker` · `judge` · `docker-proxy` · `turn` |
| `docker/server.Dockerfile` | 서버·워커 이미지(바이너리 둘) + 대시보드 정적 파일 |
| `docker/judge.Dockerfile` | 판정기 이미지(`promptlab-judge` + docker CLI 정적 바이너리, nobody) |
| `docker/docker-proxy.Dockerfile` | 본문 검사 docker 소켓 프록시 이미지(`promptlab-docker-proxy`) |

- 이미지 셋: `ghcr.io/sandcorporation/sandcorp-promptlab:<sha>`(`PROMPTLAB_IMAGE`) · `…/sandcorp-promptlab-judge:<sha>`(`PROMPTLAB_JUDGE_IMAGE`) · `…/sandcorp-promptlab-docker-proxy:<sha>`(`PROMPTLAB_PROXY_IMAGE`), **linux/arm64** 하나(세 Dockerfile 모두 amd64로도 구워진다). GitHub Actions `image.yml`이 `ubuntu-24.04-arm` 실행기에서 네이티브로 굽고 `GITHUB_TOKEN`으로 올린다(#8, 라벨 `org.opencontainers.image.source`·`revision`, 빌드 캐시 `<이미지>:buildcache`). 비공개 git 의존성·빌드 인자 비밀은 없다. oracle은 pull만 한다. 비상 길 `BUILD_ON=jenkins`는 옛 Jenkins 빌드(macos 먼저, 못 잡으면 home QEMU)로 같은 태그를 굽는다.
- **비상 길 check를 가볍게 둔 까닭(#140)**: 빌드 노드(mac Docker 12.5GB)에서 `check.sh` 전부의 시험 링크가 OOM(`ld … signal 9`)으로 죽어 배포가 막혔다. 병합 조건은 GitHub Actions `PR 검사`(`.github/workflows/pr-check.yml`) 초록이고 main push도 `release.yml`이 같은 검사를 다시 돈다. 컴파일 오류는 이미지 굽기가 잡는다.
- **server 이미지 의존성 층 분리(#10)**: `docker/server.Dockerfile`은 cargo-chef(`prepare`→`cook`)로 의존성 컴파일을 BuildKit 층에 둔다. 소스만 바뀌면 recipe.json이 같아 cook 층(서버·워커, 대시보드 wasm32)이 `:buildcache`(mode=max)로 적중한다. 캐시 마운트(`--mount=type=cache`)는 registry 캐시에 실리지 않아 새 실행기마다 비었기에 server Dockerfile에서는 걷었고, 그래서 레지스트리 공유 잠금(#140)도 이 이미지에는 필요 없다(build·dashboard 단계는 병렬). 로컬 amd64 측정: 빈 캐시 3m51s → 소스 한 줄 변경 51s. Actions(arm64) 첫 병합 실행(#11, 캐시 비어 있음): server 잡 6m24s(check 5m24s·judge 1m08s·docker-proxy 55s·promote 7s, 병합→승격 12m08s). judge·docker-proxy Dockerfile은 그대로라 `id=promptlab-cargo-registry`·`sharing=locked`(#140: 병렬 압축 풀기 경합 「File exists」 방지)를 유지한다. 러너 Dockerfile도 같다.
- 러너 이미지 4개(`RUNNER_IMAGES`)는 배포 잡이 굽지 않는다 — `Jenkinsfile.runners`가 굽고 GHCR에 올린다(아래 「러너 이미지」).
- 공개 진입: NPM(host 네트워크) → `127.0.0.1:18096`(server). 호스트 포트는 루프백에만 열리고 `.env`의 `PROMPTLAB_HOST_PORT`로 바꾼다.
- 서버는 같은 API를 `/`와 `/api` 두 곳에서 받는다. 계정(#7) 경로: `/api/auth/login`·`/api/auth/callback`(대시보드 로그인), `/api/oauth/app-client.json`(응시 앱 CIMD — auth가 이 URL을 받아 간다), `/.well-known/oauth-protected-resource/api`(RFC 9728).
- 대시보드(Dioxus 웹)는 로그인·조직·구성원 화면이 생겼지만 정적 서빙은 아직 없다(후속). 그래서 로그인 뒤 돌아가는 `https://promptlab.sandcorp.co.kr/…`는 지금 서버 404다.
- auth 쪽 등록(대장 몫): 리소스 `https://promptlab.sandcorp.co.kr/api`(스코프 `promptlab` 일반 사용자 허용, `login`), 기밀 클라이언트 「promptlab 대시보드」(allows_members). 응시 앱 공개 클라이언트(CIMD)는 배포 뒤 첫 `/authorize`에서 생기므로 그 뒤 `clients members --client-id https://promptlab.sandcorp.co.kr/api/oauth/app-client.json --allow`.
- 배포 순서(`deploy.sh`): pull(이미지 셋) → `db` 올림 → server 교체 → healthy → `docker-proxy` → `judge` → `worker` 교체 → `/health`(server 컨테이너 안에서 `healthCmd`로 `curl`). `healthUrl`(호스트 루프백 18096)은 쓰지 않는다 — Jenkins 에이전트가 호스트 네트워크가 아니라 루프백에 못 닿아 빌드 #3 헬스 확인이 실패했다(`curl: (7)`). 서버·워커는 시작 때 DB 마이그레이션을 돈다. 실패하면 직전 sha 이미지로 자동 롤백(스키마는 되돌리지 않는다 — 마이그레이션은 하위 호환으로 쓴다).

### 운영 `.env` (oracle 배포 디렉터리 `/var/jenkins_home/workspace/sandcorp-promptlab/.env`, chmod 600, 저장소에 없다)

| 키 | 뜻 |
|---|---|
| `PROMPTLAB_AUTH_CLIENT_ID` · `PROMPTLAB_AUTH_CLIENT_SECRET` | 대시보드 기밀 클라이언트(sandcorp-auth 「promptlab 대시보드」, 필수). 시크릿은 auth가 한 번만 보여 준다 — 잃으면 `clients rotate-secret` |
| `PROMPTLAB_PUBLIC_URL` | 선택, 기본 `https://promptlab.sandcorp.co.kr`. 리소스(토큰 `aud`) `…/api`, 대시보드 콜백 `…/api/auth/callback`, 앱 클라이언트 id `…/api/oauth/app-client.json`이 여기서 나온다 |
| `PROMPTLAB_AUTH_URL` | 선택, 기본 `https://auth.sandcorp.co.kr`(`iss`·JWKS·`/authorize`·`/token`) |
| `PROMPTLAB_AUTH_OWNER_SUB` | 선택, 기본 `farrar142`(대표 토큰 규칙 A1) |
| `PROMPTLAB_SMOKE_CLIENT_IDS` | 선택(#28). 받을 스모크 기계 클라이언트의 `client_id`(sandcorp-auth 「promptlab 스모크」), 쉼표 구분·앞뒤 공백 무시. 비거나 없으면 기계 토큰(`acct=machine`)은 전부 401. 이 주체는 「스모크」 조직의 응시자로만 행동한다(아래 「스모크 조직」) |
| `PROMPTLAB_ANTHROPIC_API_KEY` | 선택(#15 AI 리포트). 없으면 리포트만 꺼진다(서버·워커·채점은 그대로, 리포트 화면은 「꺼짐」, 만들기 요청 503). 서버만 읽고 `x-api-key` 헤더로만 보낸다 — 로그·응답·DB에 싣지 않는다 |
| `PROMPTLAB_AUTH_WEBHOOK_SECRETS` | 선택(#54 탈퇴 웹훅). sandcorp-auth `clients webhook`이 출력한 서명 비밀, 클라이언트(대시보드 기밀·응시 앱 CIMD)마다 하나를 쉼표로 잇는다. 웹훅 URL은 `https://promptlab.sandcorp.co.kr/api/auth/webhook`. 없으면 웹훅만 503(탈퇴자의 연습 기록은 1년 무활동 삭제로만 지워진다) |
| `POSTGRES_PASSWORD` | promptlab 전용 DB 비밀번호(필수). `DATABASE_URL`에 그대로 들어가므로 URL에 안전한 문자(영숫자)만 |
| `PROMPTLAB_HOST_PORT` | 선택, 기본 18096 |
| `RUST_LOG` | 선택, 기본 `info` |
| `PROMPTLAB_PROBLEMS_HOST_DIR` | 선택(#138). 서버·워커에 ro로 마운트할 문제 디렉터리의 호스트 경로. 비우면 compose 기본값 `./problems-data`를 쓴다 — 운영 `.env`에는 두지 않는다. 아래 「문제은행 디렉터리」 |
| `SENTRY_DSN_BACKEND` | 선택(#79 GlitchTip). compose가 서버·워커의 컨테이너 env `SENTRY_DSN`으로 넘긴다. 없으면 오류 수집이 꺼진다. 값은 비밀이라 저장소·로그에 없다(운영 `.env`는 대장 몫) |
| `PROMPTLAB_SMTP_HOST` · `PROMPTLAB_SMTP_PORT` · `PROMPTLAB_SMTP_USERNAME` · `PROMPTLAB_SMTP_PASSWORD` · `PROMPTLAB_MAIL_FROM` | 선택(#8 응시 초대 메일, SMTP). HOST가 없으면 발송이 꺼지고 초대는 대시보드에서 링크 복사로 전한다. PORT 기본 465(암묵 TLS), 587이면 STARTTLS. FROM은 `promptlab <no-reply@도메인>` 꼴이고 그 도메인의 SPF/DKIM이 SMTP 제공자로 잡혀 있어야 한다. 설명은 `crates/server/src/mail.rs` |

### 문제은행 디렉터리(#138)

히든 테스트·모범 답안은 이미지에 굽지 않는다(GHCR 이미지가 공개돼도 새지 않게, `.dockerignore`가 `problems-data`를 뺀다). 배포는
oracle 잡 워크스페이스의 체크아웃에서 compose를 돌리므로, `compose.oracle.yaml`이 `${PROMPTLAB_PROBLEMS_HOST_DIR:-./problems-data}`를
server·worker의 `/opt/promptlab/problems`에 **ro bind**로 붙이고 `PROMPTLAB_PROBLEMS_DIR`로 알린다. 서버·워커는 시작할 때 전부 읽어
`validate()`하고 하나라도 깨지면 뜨지 않는다(헬스 실패 → 자동 롤백). 시작 로그 `문제은행 실음 source=/opt/promptlab/problems practice=4 exam=8 smoke=1`.
운영 문항은 비공개 데이터 저장소 `sandcorp-promptlab-problems`에 있고, 고정 커밋은 이 저장소의 `deploy/problems.ref`(40자 sha 한 줄)다.
배포 잡(`Jenkinsfile`의 `extraCheckouts`, jenkins-shared README 「다른 저장소 받기」)이 **배포(oracle) 단계에서만** `checkout scm` 뒤에
그 커밋을 잡 워크스페이스의 `problems-data/`에 얕게 받고(매번 지우고 새로, 받은 HEAD = sha 확인, 자격 `github-farrar142` = 조직 저장소를 읽을 수 있는
SSH Username with private key, 호스트 키는 GitHub 공개 키 고정), compose 기본값
`./problems-data`를 마운트한다. 빌드 노드·이미지에는 들어가지 않는다(`.dockerignore`에도 `problems-data`). 형식이 틀린 ref·없는 sha·
자격 실패는 교체 전에 배포를 멈춘다(운영은 옛 그대로). 문항 바꾸는 절차는 `docs/problem-bank.md`.

옛 비공개 저장소(`sandcorp-promptlab-archive`) 시절의 배포 체크아웃에 남은 `crates/store/problems/`는 더 이상 마운트하지 않는다
(compose 기본값이 `./problems-data`다).

### 러너 이미지

- 판정기는 `--pull never`이고 프록시는 클라이언트의 pull(`POST /images/create`)을 막는다. 그래서 **docker 소켓을 가진 `docker-proxy`가 시작할 때** `RUNNER_IMAGES`의 참조 4개(`RunnerImage::reference()` — digest가 있으면 `name@sha256:…`, 아직 없으면 `name:태그`, 판정기가 `docker run`에 넘기는 것과 같은 문자열)를 소켓에 직접 받는다(`crates/docker-proxy/src/pull.rs`). 이미 있으면 받지 않고, 실패하면 10초부터 두 배씩(최대 10분) 다시 한다. 받는 동안에도 판정 요청은 넘긴다 — 이미지가 없으면 만들기가 실패 → 판정기 503 → 워커가 큐로 되돌린다.
- 이 방법을 고른 까닭: 참조의 정본이 Rust 상수 한 곳이라 compose·Jenkinsfile에 digest를 옮겨 적으면 어긋날 수 있다(프록시는 같은 상수로 허용 목록과 받을 목록을 함께 만든다). 공용 라이브러리 `deploy.sh`의 pull 단계는 배포 잡이 구운 `images`만 받고, `migrate` 칸은 소켓 없는 서비스 컨테이너 안에서 돌아 이미지를 받을 수 없다 — 공용 라이브러리를 고치지 않고 되는 길이 이것이다.
- GHCR 패키지는 공개(승인됨, 첫 push 뒤 전환)라 oracle에 로그인이 필요 없다. 공개 전에는 프록시 로그에 `러너 이미지 받기 실패 — 다시 한다`가 찍히고 채점은 503으로 큐에 남는다.
- 판(태그·digest)을 바꾸면(`crates/shared/src/coding/mod.rs`) 다음 배포의 새 프록시가 새 참조를 받는다. 옛 러너 이미지는 남는다(필요하면 사람이 `docker image rm`).

### docker 소켓 프록시

- 소켓을 마운트하는 서비스는 `docker-proxy` 하나다. 허용: `HEAD|GET /_ping`, `GET /version`, `POST /containers/create`(본문 검사), `POST /containers/{id}/attach|start|wait|kill`, `DELETE /containers/{id}`, `GET /containers/{id}/json` — `{id}`는 판정기 이름(`promptlab-run-<32 hex>`)·64자 id 꼴이고 **이 프록시로 만든 컨테이너**만(서버·DB 컨테이너 조회·kill 불가). 나머지(이미지 pull·build, 볼륨, 네트워크, exec, 로그, `info`, 목록 …)는 403. `/v1.NN/` 접두는 떼고 판단한다(1.40 이상만).
- 만들기 본문은 판정기 상수(`promptlab_judge::limits`)와 대조한다: 이미지 = `RUNNER_IMAGES` 참조, `NetworkMode none`, `Privileged`·`CapAdd`·`Binds`·`Mounts`·`VolumesFrom`·`Devices`·`PidMode`·`IpcMode` … 비거나 기본값, `CapDrop [ALL]`, `SecurityOpt [no-new-privileges]`, `ReadonlyRootfs`, `User 65534:65534`, `Tmpfs` = `/work`·`/tmp` 상수, CPU·메모리(스왑 없음)·pids·nofile 상수, 모르는 키는 기본값만. Go JSON과 다르게 읽히는 본문(대소문자만 다른 겹친 키, ASCII 아닌 키)은 거부.
- 권한: root로 시작해 소켓 gid를 보조 그룹으로 두고 nobody로 내린다(`cap_add: [SETUID, SETGID]`만) — 호스트 docker 그룹 gid를 `.env`에 적을 필요가 없다.
- 로그: 거부마다 한 줄(메서드·경로·까닭). 본문(코드·케이스가 지나간다)은 남기지 않는다. 시험 `crates/docker-proxy/tests/rules.rs`(거부 사례마다 하나), 끝에서 끝 `tests/e2e.rs`(`--ignored`, 호스트 docker).

## 판정기 예산·compose 상한

spec #93 3절: 동시 실행 2개(judge 안 세마포어) × 러너 컨테이너 CPU 1·메모리 512m → 최대 **2코어·1GB**(oracle 4코어 Ampere A1, 메모리 22GB). 옛 채점 워커의 모델 예산(2코어·3GB)을 이것이 대신한다. 러너 컨테이너는 데몬이 바로 만드는 형제 컨테이너라 compose 상한 밖이다 — 상한은 판정기 상수가 컨테이너마다 건다.

| 서비스 | cpus | mem_limit | 네트워크 | 그 밖 |
|---|---|---|---|---|
| `server` | — | 256m | default(루프백 18096) | |
| `worker` | 0.5 | 256m | default + judge | `PROMPTLAB_JUDGE_URL=http://judge:8090`, 모델 없음 |
| `judge` | 0.25 | 128m | judge(internal) | `DOCKER_HOST=tcp://docker-proxy:2375`, `PROMPTLAB_JUDGE_CONCURRENCY=2`, read_only + tmpfs `/tmp`, cap_drop ALL, no-new-privileges, init |
| `docker-proxy` | 0.25 | 64m | judge(internal) | 소켓 마운트, read_only, cap_drop ALL + SETUID·SETGID, no-new-privileges |
| 러너 컨테이너(최대 2) | 1씩 | 512m씩(스왑 없음) | none | pids 64, 읽기 전용 + tmpfs `/work` 64m·`/tmp` 16m, nobody, cap_drop ALL, nofile 64, 제출 벽시계 최대 60초 |
| `db` | — | 384m | default | |

- `judge` 네트워크는 `internal: true` — judge·프록시는 밖으로 나가는 길이 없고 포트도 열지 않는다. 러너 이미지 pull은 데몬이 하므로 상관없다.
- 워커는 하나만 돈다는 전제(시작 때 `grading`에 남은 제출을 큐로 되돌린다). compose에서 `worker`를 늘리지 않는다.

## 스모크 조직(#28)

Windows 끝에서 끝 스모크(`docs/windows-host.md`)는 sandcorp-auth 기계 클라이언트 토큰으로 운영에 응시·제출한다.

- 서버는 `PROMPTLAB_SMOKE_CLIENT_IDS`에 든 `client_id`의 기계 토큰만 받는다(auth 문서 12절 M1~M3, `crates/server/src/auth/token.rs`).
- 기계 주체가 할 수 있는 것: 배정 목록(`GET /me/assignments`)·문항 시작·제출·자기 제출 조회. 조직 경로는 404, 그 밖(조직 만들기·초대 받기·연습 제출·`/me`·대시보드 로그인)은 403(`crates/server/src/auth/mod.rs` `machine_may`).
- **준비는 멱등 자동**: 기계 주체가 배정 목록을 부르면 `promptlab_store::smoke::prepare`가 「스모크」 조직(`orgs.is_smoke`, 하나뿐)·스모크 시험(사내 진단·점수 공개, 시범 문제 1문항, 30분·10턴)·시작 전 배정 하나를 없을 때만 만든다. 문항마다 제출은 한 번이라 스모크 한 번에 배정 하나를 새로 쓴다. 관리 명령을 두지 않은 까닭: 스모크 조직은 대시보드에 보이지 않아 사람이 만들 자리가 없고, env 키 하나로 준비가 끝나게 하려고.
- **격리**: 스모크 조직은 구성원 조회에서 빠진다 — 대시보드 조직 목록(`/me`)·조직 화면·결과 목록·제출 조회(조직 권한)에 나오지 않는다(DB로 구성원을 넣어도). 집계를 새로 만들 때도 `NOT orgs.is_smoke`로 거른다. 스모크 제출의 결과는 그 기계 주체만 본다.
- 스모크 결과를 운영 DB에서 보려면(읽기): `SELECT s.id, s.status, s.created_at FROM submissions s JOIN orgs o ON o.id = s.org_id WHERE o.is_smoke ORDER BY s.created_at DESC LIMIT 10;`

## 실시간 감독 TURN(#14)

- 서비스 `turn`(`coturn/coturn:4.18.0-alpine`, host 네트워크, `cpus: 0.5`·`mem_limit: 128m`). 배포 잡이 `keepServices`로 `up -d --no-deps turn`(고정 태그라 설정이 같으면 그대로).
- 인증은 TURN REST API(`use-auth-secret`): 서버와 coturn이 공유 비밀 하나를 나눠 갖고, 서버가 세션마다 1시간짜리 HMAC 자격을 준다. 장기 비밀번호는 없다. 사설·루프백 대역으로는 relay하지 않는다(`denied-peer-ip`), 사용자당 할당 4개·전체 40개(relay 포트 하나가 할당 하나라 포트 수와 맞춘다. 영상 한 세션 = 앱 1·감독관 1 할당이라 동시 약 20세션. 포트 범위는 `PROMPTLAB_TURN_MIN_PORT`·`PROMPTLAB_TURN_MAX_PORT`로 바꾼다 — 바꾸면 방화벽·`total-quota`도 맞춘다)·세션당 1Mbps.
- 운영 `.env`(대장 몫): `PROMPTLAB_TURN_SECRET`(서버·coturn 같은 값, 예: `openssl rand -hex 32`), `PROMPTLAB_TURN_EXTERNAL_IP`(oracle 공인 IP — 호스트는 사설 IP라 relay 주소로 공인 IP를 알려야 한다). 선택: `PROMPTLAB_TURN_URLS`, `PROMPTLAB_TURN_TTL_SECS`.
- 방화벽(대장 몫, OCI 보안 목록 + 호스트 iptables): 들어오는 UDP 3478, TCP 3478, UDP 49160-49199(40개). TLS(5349)는 쓰지 않는다(443은 NPM).
- DNS: 따로 없음 — `promptlab.sandcorp.co.kr`이 oracle로 바로 간다(프록시 아님). URL 기본은 `turn:promptlab.sandcorp.co.kr:3478?transport=udp`·`tcp`.
- 값이 없을 때: turn 컨테이너는 안내 한 줄을 찍고 exit 0(`restart: on-failure`라 다시 뜨지 않는다), 서버는 실시간 보기만 503. 값을 넣은 뒤 다음 배포(또는 `docker compose -p promptlab-prod -f compose.oracle.yaml up -d --no-deps turn server`)로 켜진다.
- 시그널링 세션은 서버 메모리에만 있다 — 서버가 다시 뜨면 열린 실시간 보기는 닫힌다(감독관이 다시 연다).

## 오류 수집 — GlitchTip(#79)

- 서버·워커가 sentry SDK(`crates/observability`, rustls 전송 — OpenSSL 없음)로 GlitchTip 프로젝트 `promptlab-backend`에 보낸다. 컨테이너 env `SENTRY_DSN`(운영 `.env`의 `SENTRY_DSN_BACKEND`)이 없으면 SDK를 켜지 않는다 — dev·CI·시험은 조용하다.
- `release`는 컨테이너 env `SENTRY_RELEASE` = 배포 커밋 sha. compose가 배포 잡이 `deploy.sh`에 넘기는 `SANDCORP_IMAGE_SHA`(이미지 태그 sha, 자동·수동 되돌리기면 되돌린 sha)를 받는다. `environment`는 `production`, PII·요청 본문·성능 추적 없음.
- 이벤트가 되는 것: 패닉, `tracing::error!`(서버 요청 안의 것은 요청 URL·메서드가 붙고 query·쿠키·인증 헤더는 지운다). warn·info는 breadcrumb로도 보내지 않는다. 걸러내기 규칙은 `crates/observability/src/lib.rs`의 `scrub_event`(시험 `crates/observability/tests`).
- 연동 확인(대장): `docker exec <server 컨테이너> promptlab-server glitchtip-test` — 서버를 띄우지 않고 `GlitchTipOnboardingTest("연동 시험 promptlab <KST 날짜>")` 오류 하나를 보내고 5초 flush한다. DSN이 없으면 「DSN 없음」·종료 코드 1, 보내지 못했으면 2.
- 코드에 error 수준 로그를 더할 때: 숨은 입력·검사 규칙·모델 출력·`failure_detail`·대화 원문·이메일·이름을 싣지 않는다(id·종류·건수만) — 그대로 GlitchTip에 나간다.

## 남은 한계

- 프록시가 아는 「만든 컨테이너」는 메모리에만 있다(15분). 프록시가 판정 도중 재시작하면 그 판정의 kill이 거부되고(컨테이너는 `--rm`·러너 시간 상한으로 스스로 끝난다) 판정기가 503을 줄 수 있다 — 워커가 다시 넣는다.
- judge 이미지의 docker CLI 판을 올리면 만들기 본문 꼴이 바뀔 수 있다 — 프록시 픽스처(`crates/docker-proxy/tests/fixtures`)를 다시 잡는다.
- Gatus: 저장소 밖(모니터링 설정)이라 이 PR에는 없다. 조각은 PR 본문 참고.
