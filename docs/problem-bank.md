# 문제은행

코딩 문제은행(#95, spec #93). 문항은 `<id>.json`(`shared::coding::CodingProblem` 형식 — 설명·서명·예제·히든 케이스·모범
답안·제한)이고, 시험을 만들 때 출제자가 여기서 문항을 고른다.

## 어디에 있나(#138)

히든 테스트·모범 답안은 **공개 저장소·이미지에 두지 않는다.** 서버·워커는 바이너리에 굽지 않고 **시작할 때 디렉터리에서
읽는다**(`crates/store/src/catalog.rs`).

| 환경 | 싣는 것 |
|---|---|
| `PROMPTLAB_PROBLEMS_DIR` 있음(운영) | 그 디렉터리. 운영 compose가 `/opt/promptlab/problems`에 ro로 마운트한다(호스트 경로 `PROMPTLAB_PROBLEMS_HOST_DIR` — 기본 `./problems-data`, 아래) |
| 없음(개발 스택·CI·시험) | 저장소에 구운 **공개 픽스처** `crates/store/fixtures/problems/`(연습 2·시험 2 — 운영 문제은행과 다른, 공개돼도 되는 문항) |
| 언제나 | **스모크 전용 공개 문항** `crates/store/fixtures/smoke/smoke-running-sum.json` — 스모크 조직 시험만 쓰고 연습·시험 문제은행·운영자 목록에 안 나온다. Windows 스모크가 이 파일의 모범 답안으로 응시한다 |

픽스처와 운영 문항은 섞지 않는다. 디렉터리 꼴은 `index.json`(`{"practice": [id…], "exam": [id…]}` — 목록 순서 = 화면 순서)
+ 문항마다 `<id>.json`이다(사람용 표 `INDEX.md`는 같이 두되 읽지 않는다). 시작할 때 전부 읽어 `validate()`하고, 파일
이름 ≠ id·id 중복·목록에 없는 `*.json`·없는 파일·스모크 id 겹침 중 하나라도 있으면 이유를 찍고 **뜨지 않는다**(배포
헬스가 잡는다). 시작 로그 `문제은행 실음 source=… practice=… exam=… smoke=1`.

운영 문항(연습 4·시험 8, 목록 `INDEX.md`)은 **비공개 데이터 저장소 `sandcorp-promptlab-problems`**에 있고, 운영은 이 저장소의
**`deploy/problems.ref`**(40자 커밋 sha 한 줄)가 고정한 커밋을 쓴다. 배포 잡이 배포(oracle) 단계에서 그 커밋을 잡 워크스페이스의
`problems-data/`에 얕게 받는다(jenkins-shared `extraCheckouts`, 자격 `github-farrar142` — `Jenkinsfile`). 이미지·
빌드 노드에는 들어가지 않는다.

## 연습 문항과 시험 문항(#18)

`index.json`의 `practice` 문항은 개인 연습에만, `exam` 문항은 시험에만 쓴다. 연습에서는 같은 히든 케이스에 몇 번이고
제출해 통과 수를 볼 수 있으므로, 시험 문항을 연습에 열면 시험 유출 통로가 된다. 시험 문제은행과 시험 만들기는 연습
문항을 받지 않고, 연습(`GET /problems/{id}`·`POST /submissions`)은 시험 문항에 404다. 연습 문항도 히든 케이스와 모범
답안은 밖으로 내보내지 않는다(통과 수만).

## 문항 형식과 검증

- 답이 하나로 정해지는 문제만 둔다(순서가 여럿 가능한 답은 설명에서 정렬 기준을 못 박는다).
- 기대값은 **모범 답안(Python 3.12)을 실제로 돌린 값**이다. 예제 2~3개, 히든 8~15개(경계값·큰 입력 포함), 케이스 입력
  64KB·합계 1MB 이하, `int`는 ±(2^53−1) 안.
- 불변식은 시험이 건다. 대상은 `PROMPTLAB_PROBLEMS_DIR`, 없으면 공개 픽스처(+ 스모크 문항).
  - 싣기(`crates/store/tests/bank.rs`, 항상): 픽스처 = 구운 표, 연습·시험 안 겹침, 스모크는 문제은행 밖, 깨진 디렉터리는 거부.
    `PROMPTLAB_PROBLEMS_DIR`가 있으면 그 디렉터리가 실리는지도 본다.
  - 형식 시험(`crates/judge/tests/bank.rs`, 항상): `index.json` 목록 = 파일, `CodingProblem`으로 읽히고 `validate()` 통과,
    파일 이름 = id.
  - docker 시험(`#[ignore]`): 문항마다 모범 답안을 판정기로 예제 + 히든 전부 돌려 모두 통과. 출력은 id·통과 수·실패 케이스
    번호뿐이다(히든 입력·기대값을 찍지 않는다).

  ```
  bash scripts/build-runners.sh --local
  PROMPTLAB_PROBLEMS_DIR=<데이터 저장소 체크아웃> cargo test -p promptlab-store -p promptlab-judge --test bank -- --include-ignored --test-threads=1
  ```

## 문항을 더하는 절차

운영 문항(데이터 저장소 `sandcorp-promptlab-problems` — 그 저장소 README와 같다):

1. `<id>.json`을 `CodingProblem` 형식으로 쓴다(`id` 소문자-하이픈, `reference_solution`은 틀 모양
   `def 이름(인자: 타입) -> 타입:` 그대로, `"reference_language": "python"`). 기대값은 모범 답안을 로컬에서 돌려 넣는다.
2. `index.json`의 `practice` 또는 `exam`에 id를, `INDEX.md` 표에 한 줄(난이도·주제·타입·연습/시험)을 더한다.
3. 위 docker 시험을 `PROMPTLAB_PROBLEMS_DIR`로 돌려 모범 답안이 모두 통과하는지 본다. 코드는 고치지 않는다.
4. 데이터 저장소에 PR → 그 저장소 CI(문항 검사 — 이 저장소 main의 형식·싣기 시험) 초록 → 병합.
5. **이 저장소**에서 `deploy/problems.ref`를 방금 병합한 데이터 저장소 main 커밋의 40자 sha로 바꾸는 PR을 낸다. 병합하면 배포가
   돌고 운영이 새 문항을 쓴다. 4만 하고 5를 잊으면 운영은 바뀌지 않는다(의도 — 어느 데이터로 도는지가 이 저장소 이력에 sha로
   남고 내용은 남지 않는다). 형식이 틀린 ref·없는 sha는 배포가 받기 단계(교체 전)에서 실패하고 운영은 옛 그대로다.
6. 배포 뒤 시작 로그의 `practice=… exam=…` 수를 본다.

공개 픽스처(`crates/store/fixtures/problems/`)는 같은 꼴에 더해 `catalog.rs`의 `FIXTURES` 표에도 더한다(구워 둔다 — 표와
디렉터리가 같은지 시험이 본다). 서버 시험의 가짜 판정기는 인자 JSON으로 답을 찾으므로 문항끼리 인자가 겹치면 안 되고,
숨은 입력 불변식이 16자 이상 인자만 찾으니 히든 인자는 길게 둔다.
