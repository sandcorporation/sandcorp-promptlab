# 시험 이음매 ④: 끝에서 끝 스모크

Windows 호스트(Jenkins 서비스 에이전트, 세션 0)에서 설치 파일로 설치 → Docker 확인 → 앱 창 확인 → 응시(모범 답안·로컬 Docker 예제 테스트) → 제출 → 서버 히든 채점까지 하나를 돈다. `scripts/ci/check.sh`(PR 검사)는 이 시험을 돌리지 않는다.

- `smoke.ps1` — 진입점. 단계: 무인 설치 → Docker 확인(`docker version` + 앱 CLI `--check-docker`) → 앱 창 기동 확인(세션 0이면 경고로 건너뜀, 화면 캡처 없음) → 서버 주소 기본값 → 가상 카메라(자리) → 기계 토큰(`scripts/windows/smoke-token.ps1`, client_credentials) → 「스모크」 조직 배정을 `promptlab-app-cli`로 응시(스모크 문항의 모범 답안, 로컬 Docker 예제 테스트 전부 통과·서버 히든 채점 전부 통과 확인) → 제거. 종료 0 = 채점 끝.
- `smoke.sh` — 리눅스에서는 하지 않는다는 안내(종료 2).

계정·토큰·노드 구성은 `docs/windows-host.md`, 잡은 `Jenkinsfile.windows`, 사람이 하는 끝에서 끝은 `docs/e2e-acceptance.md`.
