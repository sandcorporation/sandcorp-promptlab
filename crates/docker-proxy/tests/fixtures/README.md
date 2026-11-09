# 픽스처

`create-docker-cli-29.8.1.json` — docker CLI 29.8.1(API 1.56)이 판정기 인자
(`promptlab_judge::docker_run_args(<러너 이미지>, <이름>)`)로 `docker run`할 때 보낸 `POST /containers/create`
본문 원문. 로깅 중계로 잡았다(2026-11-07). 같은 실행에서 CLI가 부른 호출은 차례로
`HEAD /_ping` → `POST /v1.56/containers/create?name=…` → `POST …/attach?stderr=1&stdin=1&stdout=1&stream=1`(101)
→ `POST …/wait?condition=removed` → `POST …/start`, 벽시계 `docker kill <이름>`은 `HEAD /_ping` →
`POST …/<이름>/kill`이다(이미지 inspect·`info` 없음).

judge 이미지의 docker CLI 판(`docker/judge.Dockerfile`)을 올리면 다시 잡아 바꾼다 — 본문 꼴이 바뀌면
`tests/rules.rs`의 허용 시험이 먼저 깨진다.
