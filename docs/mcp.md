# MCP 출제 서버 연결하기 (#17)

출제자(조직의 관리자·출제자 역할)가 Claude 같은 AI 도구에서 조직 문제를 만들고 고치고, 풀림을 미리 돌려 본다.

- 주소: `https://promptlab.sandcorp.co.kr/api/mcp` (전송 Streamable HTTP)
- 인증: sandcorp-auth OAuth(카카오 로그인). 클라이언트가 처음 부르면 `401`과 함께
  `WWW-Authenticate: Bearer resource_metadata="https://promptlab.sandcorp.co.kr/.well-known/oauth-protected-resource/api"`를 받고,
  그 문서(리소스 `https://promptlab.sandcorp.co.kr/api`, 스코프 `promptlab`, 권한 서버 `https://auth.sandcorp.co.kr`)를 따라 로그인한다.
  토큰은 응시 앱·대시보드와 같은 리소스·스코프다 — auth에 따로 등록할 리소스는 없다.

## Claude Code

```
claude mcp add --transport http promptlab https://promptlab.sandcorp.co.kr/api/mcp
```

그다음 Claude Code에서 `/mcp` → `promptlab` → 「Authenticate」를 고르면 브라우저에서 sandcorp-auth 로그인이 열린다.

## Claude 데스크톱·claude.ai

설정 → 커넥터 → 「사용자 지정 커넥터 추가」에 이름 `promptlab`, URL `https://promptlab.sandcorp.co.kr/api/mcp`를 넣고 「연결」 → 로그인.

## 일반 사용자(member) 연결 — auth 결정 대기

Claude는 auth에 클라이언트를 새로 등록한다(DCR `/register` 또는 CIMD). auth는 새 클라이언트의 「일반 사용자 허용」(`allows_members`)이
기본으로 꺼져 있어, 지금은 **대표 계정만** 연결된다. 출제자(일반 사용자)가 쓰려면 auth 쪽에서 그 클라이언트에 일반 사용자 허용을 켜거나
정책을 바꿔야 한다(조율자 결정 대기). promptlab 서버 쪽은 바꿀 것이 없다.

## 문제 원본(`ProblemSource`)

`create_problem`·`update_problem`은 코딩 문제 하나를 이 모양으로 받는다(JSON Schema는 `tools/list`가 준다 — shared `ProblemSource`에서 나온다).

```json
{
  "title": "두 수 더하기",
  "description": "정수 `a`와 `b`의 합을 돌려준다. (마크다운, 응시자에게 보인다)",
  "signature": {"name": "add", "params": [{"name": "a", "type": "int"}, {"name": "b", "type": "int"}], "returns": "int"},
  "examples": [{"args": [1, 2], "expected": 3}],
  "hidden": [{"args": [0, 0], "expected": 0}, "… 5~50개"],
  "reference_solution": "def add(a, b):\n    return a + b\n",
  "reference_language": "python",
  "limits": {"case_time_ms": 2000}
}
```

- **서명**: 함수 이름 + 인자(이름·타입) + 반환 타입. 타입은 `int`(±(2^53−1) 안)·`double`·`bool`·`string`과 그 1차원 배열, `int[][]`·`string[][]`뿐이다.
- **케이스** `{args, expected}`: 예제 1~10개(응시자에게 보인다), 히든 5~50개(밖으로 나가지 않는다). 입력 하나 64KB·합계 1MB. `double`은 상대·절대 1e-6.
- 4개 언어(Python 3.12·JavaScript Node 22·Java 21·C++17)의 빈 틀과 실행 하네스는 서명에서 **자동으로 만든다** — 출제자는 쓰지 않는다.
- **모범 답안** 하나(`reference_language`, 기본 python)를 서명대로 쓴다. 발행 전 점검이 예제 + 히든을 모두 통과하는지 판정기로 본다.
- 제목·설명·서명·케이스가 규칙에 맞지 않으면 `422`와 까닭(HTTP 편집 API와 같은 검증).

## 도구

| 도구 | 하는 일 | 결과 |
|---|---|---|
| `list_orgs` | 내 조직과 역할 | `{orgs: [OrgView]}` |
| `list_problems` | 조직 문제 목록(숨은 입력 없음) | `{problems: [OrgProblemSummary]}` |
| `get_problem` | 조직 문제 원본 전체(히든 테스트·모범 답안 포함) | `OrgProblem` |
| `create_problem` | 검증하고 저장(새 문제는 발행 전이라 시험에 못 넣는다) | `OrgProblem` |
| `update_problem` | 원본을 통째로 바꿈(원본이 바뀌면 발행이 풀린다) | `OrgProblem` |
| `start_preview` | 저장된 문제(`problem_id`)의 **발행 전 점검**을 큐에 넣음(#73·#96): ① 모범 답안이 예제 + 히든을 모두 통과 ② 4개 언어 빈 틀이 모두 컴파일 | `PreviewView` |
| `get_preview` | 점검 상태·결과(`done`·`failed`까지 몇 초 간격으로) — `result`의 통과 수(`passed`·`total`·`compiled`)와 언어별 틀 컴파일(`templates`), 발행 가능·안내. 케이스 입력·출력은 없다 | `PreviewView` |
| `publish_problem` | 지금 원본의 마지막 점검이 모두 통과면 발행(그래야 시험에 넣는다), 아니면 422 + 까닭·안내 | `Publication` |

타입은 HTTP API와 같다(`crates/shared/src/authoring.rs`). 실패는 도구 결과 `isError: true`에
`{"status": 403|404|422|429, "error": "…"}` — HTTP와 같은 뜻이다(구성원이 아닌 조직·다른 조직 문제 404, 역할 부족 403,
검증 실패·점검 미통과 발행 422, 점검 한도(조직마다 진행 중 3개) 429). 점검·발행 규칙의 정본은
`crates/shared/src/publication.rs`.

## 직접 확인(토큰 없이)

```
curl -si -X POST https://promptlab.sandcorp.co.kr/api/mcp -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"ping"}' | grep -i -E '^HTTP|www-authenticate'
curl -s https://promptlab.sandcorp.co.kr/.well-known/oauth-protected-resource/api
```
