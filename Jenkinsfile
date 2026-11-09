// main 배포 — 파이프라인은 Jenkins 공용 라이브러리 sandcorp(sandcorporation/jenkins-shared)의 sandcorpDeploy에 있다.
// 여기에는 이 저장소가 넘기는 값만 둔다. 동작과 인자 목록은 jenkins-shared README, 이 저장소 사정은 docs/deploy.md.
//
// 운영 .env 는 여기서 만들지 않는다. oracle 잡 워크스페이스(deployDir 기본값)에 상주한다.
@Library('sandcorp') _

sandcorpDeploy(
  name: 'Promptlab',
  // 이미지 셋: 서버·워커(바이너리 둘), 판정기(judge + docker CLI), 본문 검사 docker 소켓 프록시(#95).
  // 러너 이미지(RUNNER_IMAGES)는 여기서 굽지 않는다 — Jenkinsfile.runners 가 굽고, oracle 에서는 docker-proxy 가 시작할 때 받는다.
  images: [
    [repo: 'ghcr.io/sandcorporation/sandcorp-promptlab', envVar: 'PROMPTLAB_IMAGE', dockerfile: 'docker/server.Dockerfile', context: '.'],
    [repo: 'ghcr.io/sandcorporation/sandcorp-promptlab-judge', envVar: 'PROMPTLAB_JUDGE_IMAGE', dockerfile: 'docker/judge.Dockerfile', context: '.'],
    [repo: 'ghcr.io/sandcorporation/sandcorp-promptlab-docker-proxy', envVar: 'PROMPTLAB_PROXY_IMAGE', dockerfile: 'docker/docker-proxy.Dockerfile', context: '.'],
  ],
  sourceUrl: 'https://github.com/sandcorporation/sandcorp-promptlab',
  buildSsh: false,                      // 비공개 git 의존성 없음
  // 배포 전 check는 fmt·.ps1 BOM만(#140) — 시험 전부는 PR(Actions)과 main push(Actions)가 돈다. 컴파일은 이미지 굽기가 한다.
  check: 'bash scripts/ci/check.sh --quick',
  composeProject: 'promptlab-prod',     // compose.oracle.yaml 의 name: 과 같아야 한다
  composeFiles: ['compose.oracle.yaml'],
  service: 'server',
  preServices: ['db'],                  // 서버 교체(--no-deps) 전에 DB 를 올린다(서버·워커는 시작 때 마이그레이션을 돈다)
  // 서버가 healthy 해진 뒤 새 이미지로 다시 만든다 — 프록시 → 판정기 → 워커 차례(워커가 judge 를, judge 가 프록시를 부른다).
  // docker-proxy 는 시작할 때 러너 이미지(RUNNER_IMAGES 참조)가 없으면 받는다(docs/deploy.md 「러너 이미지」).
  extraServices: ['docker-proxy', 'judge', 'worker'],
  // 실시간 감독 TURN(#14, coturn 고정 태그). .env 에 비밀·공인 IP가 없으면 컨테이너가 바로 exit 0 — 배포는 깨지지 않는다.
  keepServices: ['turn'],
  // Jenkins 에이전트 컨테이너는 호스트 루프백에 못 닿는다 — 헬스는 server 컨테이너 안에서 본다.
  healthCmd: 'curl -fsS http://127.0.0.1:8080/health',
  // 문제은행(#138): 운영 문항은 비공개 데이터 저장소에서 deploy/problems.ref 의 커밋으로 problems-data/ 에 받는다
  // (jenkins-shared extraCheckouts — 배포(oracle) 단계에서만, 이미지·check에는 안 들어간다). compose.oracle.yaml 이
  // ./problems-data 를 server·worker 에 ro 로 붙인다(docs/deploy.md 「문제은행 디렉터리」).
  extraCheckouts: [[url: 'git@github.com:sandcorporation/sandcorp-promptlab-problems.git',
                    refFile: 'deploy/problems.ref', dir: 'problems-data',
                    credentialsId: 'github-farrar142']],
)
