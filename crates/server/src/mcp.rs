//! MCP 출제 서버(#17): 출제자가 AI 도구(Claude 등)에서 조직 코딩 문제를 만들고 고치고 발행 전 점검을 돌린다.
//!
//! - 전송: MCP Streamable HTTP의 상태 없는 꼴. `POST /api/mcp`(같은 경로를 `/mcp`에서도)에 JSON-RPC 메시지 하나,
//!   응답은 `application/json` 한 덩이(SSE 흐름·세션 id 없음). 알림·응답 메시지는 `202`. `GET`·`DELETE`는 `405`.
//! - 인증: `Authorization: Bearer`만(세션 쿠키는 보지 않는다 — 쿠키로 도구를 부르는 교차 출처 요청이 생기지 않는다).
//!   토큰 규칙은 API와 같다(리소스 `…/api`, 스코프 `promptlab`). 실패는 `401`/`403` + `WWW-Authenticate`의
//!   `resource_metadata`(보호 리소스 메타데이터 `/.well-known/oauth-protected-resource/api`) — 클라이언트가 이걸 보고
//!   sandcorp-auth OAuth를 시작한다. 기계 주체(스모크)는 `403`.
//! - 권한: 도구는 [`crate::problems`]의 서비스 함수를 그대로 부른다 — HTTP API와 권한(시험 관리 역할)·검증·큐 한도가
//!   같다. HTTP 상태(404·403·422·429)는 도구 결과 `isError: true` + 본문 `{"status": …, "error": …}`로 돌려준다.
//! - 스키마: 도구 입력·출력 JSON Schema는 HTTP API 타입(`promptlab_shared::authoring`)에서 뽑는다(`schemars`).
//!
//! 히든 테스트·모범 답안이 실리는 결과(`get_problem`·`create_problem`·`update_problem`)는 HTTP 편집 API처럼 자기 조직의
//! 시험 관리 역할에게만 나간다.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use promptlab_shared::api::OrgView;
use promptlab_shared::authoring::{
    OrgProblem, OrgProblemSummary, PreviewRequest, PreviewView, ProblemSource, Publication,
};
use schemars::JsonSchema;
use schemars::generate::SchemaSettings;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::auth::token::AccountKind;
use crate::auth::{Account, bearer_account};
use crate::{ApiError, AppState, orgs, problems};

/// 받는 MCP 판(새 것부터). 클라이언트가 고른 판이 여기 있으면 그대로, 없으면 맨 앞 판으로 답한다.
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];

/// `MCP-Protocol-Version` 헤더.
const PROTOCOL_HEADER: &str = "mcp-protocol-version";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

// ── 도구 입력·출력 ──

/// 입력 없음.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct NoArgs {}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct OrgArgs {
    /// 조직 id(`list_orgs`).
    pub org_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProblemArgs {
    /// 조직 id(`list_orgs`).
    pub org_id: String,
    /// 조직 문제 id(`org-`로 시작, `list_problems`).
    pub problem_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateArgs {
    /// 조직 id(`list_orgs`).
    pub org_id: String,
    /// 문제 원본.
    pub problem: ProblemSource,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateArgs {
    /// 조직 id(`list_orgs`).
    pub org_id: String,
    /// 고칠 조직 문제 id.
    pub problem_id: String,
    /// 새 문제 원본(통째로 바꾼다).
    pub problem: ProblemSource,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StartPreviewArgs {
    /// 조직 id(`list_orgs`).
    pub org_id: String,
    #[serde(flatten)]
    pub request: PreviewRequest,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PreviewArgs {
    /// 조직 id(`list_orgs`).
    pub org_id: String,
    /// `start_preview`가 준 id.
    pub preview_id: String,
}

/// `list_orgs` 결과.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct OrgList {
    pub orgs: Vec<OrgView>,
}

/// `list_problems` 결과(히든 테스트 없음).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ProblemList {
    pub problems: Vec<OrgProblemSummary>,
}

/// 도구 하나의 정의.
struct Tool {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    input: fn(&mut schemars::SchemaGenerator) -> schemars::Schema,
    output: fn(&mut schemars::SchemaGenerator) -> schemars::Schema,
    read_only: bool,
}

fn schema<T: JsonSchema>(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    generator.root_schema_for::<T>()
}

const TOOLS: &[Tool] = &[
    Tool {
        name: "list_orgs",
        title: "내 조직",
        description: "내가 구성원인 조직과 내 역할. 문제 도구는 관리자(admin)·출제자(author) 역할인 조직에서만 쓸 수 있다.",
        input: schema::<NoArgs>,
        output: schema::<OrgList>,
        read_only: true,
    },
    Tool {
        name: "list_problems",
        title: "조직 문제 목록",
        description: "조직 코딩 문제 목록(제목·예제 수·히든 테스트 수·발행 여부). 히든 테스트는 싣지 않는다.",
        input: schema::<OrgArgs>,
        output: schema::<ProblemList>,
        read_only: true,
    },
    Tool {
        name: "get_problem",
        title: "조직 문제 조회",
        description: "조직 코딩 문제 원본 전체(제목·설명·함수 서명·예제·히든 테스트·모범 답안과 그 언어·케이스 시간).",
        input: schema::<ProblemArgs>,
        output: schema::<OrgProblem>,
        read_only: true,
    },
    Tool {
        name: "create_problem",
        title: "조직 문제 만들기",
        description: "새 조직 코딩 문제를 검증하고 저장한다. 함수 서명(이름·인자·반환 타입 — int·double·bool·string과 그 1차원 배열, int[][]·string[][]), 예제 1~10개·히든 테스트 5~50개(각 {args, expected}), 모범 답안(reference_solution, 언어 reference_language — python·javascript·java·cpp)을 넣는다. 새 문제는 발행 전이라 시험에 넣을 수 없다 — start_preview로 발행 전 점검을 돌리고 publish_problem으로 발행한다.",
        input: schema::<CreateArgs>,
        output: schema::<OrgProblem>,
        read_only: false,
    },
    Tool {
        name: "update_problem",
        title: "조직 문제 고치기",
        description: "조직 문제 원본을 통째로 바꾼다(검증 같음). 원본이 바뀌면 발행이 풀린다 — 다시 점검·발행해야 새 시험에 넣는다(이미 만든 시험은 그대로).",
        input: schema::<UpdateArgs>,
        output: schema::<OrgProblem>,
        read_only: false,
    },
    Tool {
        name: "start_preview",
        title: "발행 전 점검",
        description: "저장된 조직 문제의 지금 원본으로 발행 전 점검을 큐에 넣는다: ① 모범 답안을 실제 판정기로 예제+히든 테스트에 돌려 모두 통과하고 ② 서명에서 만든 4개 언어(python·javascript·java·cpp) 빈 틀이 모두 컴파일돼야 한다. 바로 돌아온다(채점이 먼저라 기다릴 수 있다, 조직마다 진행 중 3개까지). get_preview로 state가 done·failed가 될 때까지 몇 초 간격으로 확인한다.",
        input: schema::<StartPreviewArgs>,
        output: schema::<PreviewView>,
        read_only: false,
    },
    Tool {
        name: "get_preview",
        title: "발행 전 점검 결과",
        description: "점검 상태(queued·running·done·failed), 모범 답안의 통과 수(result: passed·total·compiled)와 언어별 틀 컴파일 결과(result.templates: [{language, compiled}] — 4개 모두 true여야 발행 가능), 발행 가능(publishable)과 짧은 까닭(reason). current가 false면 그 뒤에 문제를 고쳤다는 뜻이라 다시 점검해야 한다. 케이스 입력·실행 출력은 싣지 않는다.",
        input: schema::<PreviewArgs>,
        output: schema::<PreviewView>,
        read_only: true,
    },
    Tool {
        name: "publish_problem",
        title: "조직 문제 발행",
        description: "지금 원본의 마지막 발행 전 점검이 모두 통과면 발행한다(이제 시험에 넣을 수 있다). 점검하지 않았거나·끝나지 않았거나·통과하지 못했으면 오류(422)와 까닭·안내.",
        input: schema::<ProblemArgs>,
        output: schema::<Publication>,
        read_only: false,
    },
];

/// `tools/list`의 도구 목록(입력·출력 JSON Schema 포함, 하위 스키마는 펼친다).
pub fn tool_list() -> Value {
    let mut generator = SchemaSettings::draft2020_12()
        .with(|s| s.inline_subschemas = true)
        .into_generator();
    let tools: Vec<Value> = TOOLS
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "title": tool.title,
                "description": tool.description,
                "inputSchema": (tool.input)(&mut generator),
                "outputSchema": (tool.output)(&mut generator),
                "annotations": {
                    "title": tool.title,
                    "readOnlyHint": tool.read_only,
                    "destructiveHint": false,
                    "openWorldHint": false,
                },
            })
        })
        .collect();
    json!({ "tools": tools })
}

// ── 전송 ──

/// `GET`·`DELETE /mcp`: 서버가 여는 SSE 흐름·세션이 없다.
pub async fn method_not_allowed() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, "POST")],
        Json(json!({"error": "MCP는 POST만 받는다"})),
    )
        .into_response()
}

/// `POST /mcp`: JSON-RPC 메시지 하나.
pub async fn handle(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let account = match bearer_account(&state.auth, &headers).await {
        Ok(account) => account,
        Err(failure) => return failure.into_response(&state.auth),
    };
    if account.kind == AccountKind::Machine {
        tracing::info!(sub = %account.sub, "MCP 거부: 기계 주체");
        return ApiError::new(StatusCode::FORBIDDEN, "기계 계정은 MCP를 쓸 수 없다")
            .into_response();
    }
    if let Some(version) = headers.get(PROTOCOL_HEADER) {
        let known = version
            .to_str()
            .is_ok_and(|v| PROTOCOL_VERSIONS.contains(&v));
        if !known {
            return ApiError::new(StatusCode::BAD_REQUEST, "모르는 MCP 판이다").into_response();
        }
    }
    let message: Value = match serde_json::from_slice(&body) {
        Ok(message) => message,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(rpc_error(Value::Null, PARSE_ERROR, "JSON이 아니다")),
            )
                .into_response();
        }
    };
    let Some(object) = message.as_object() else {
        // 묶음(배열)은 받지 않는다(2025-06-18부터 명세에서 빠졌다).
        return (
            StatusCode::BAD_REQUEST,
            Json(rpc_error(
                Value::Null,
                INVALID_REQUEST,
                "메시지 하나만 받는다",
            )),
        )
            .into_response();
    };
    let id = object.get("id").cloned();
    let method = object.get("method").and_then(Value::as_str);
    let (Some(id), Some(method)) = (id, method) else {
        // 알림(id 없음)이나 클라이언트의 응답: 받기만 한다.
        return StatusCode::ACCEPTED.into_response();
    };
    let params = object.get("params").cloned().unwrap_or(Value::Null);
    let reply = match method {
        "initialize" => Ok(initialize(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tool_list()),
        "tools/call" => call_tool(&state, &account, params).await,
        _ => Err((METHOD_NOT_FOUND, format!("모르는 메서드: {method}"))),
    };
    let body = match reply {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err((code, message)) => rpc_error(id, code, &message),
    };
    Json(body).into_response()
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn initialize(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = requested
        .filter(|v| PROTOCOL_VERSIONS.contains(v))
        .unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {
            "name": "promptlab",
            "title": "promptlab 출제",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": "promptlab 조직 문제 출제 도구. list_orgs로 관리자·출제자인 조직을 고르고, list_problems·get_problem으로 보고, create_problem·update_problem으로 저장한다. 저장한 뒤 start_preview → get_preview로 발행 전 점검(모범 답안이 모든 케이스 통과)을 보고, 모두 통과면 publish_problem으로 발행해야 시험에 넣을 수 있다.",
    })
}

#[derive(Deserialize)]
struct CallParams {
    name: String,
    #[serde(default)]
    arguments: Option<Value>,
}

async fn call_tool(
    state: &AppState,
    account: &Account,
    params: Value,
) -> Result<Value, (i64, String)> {
    let CallParams { name, arguments } = serde_json::from_value(params)
        .map_err(|e| (INVALID_PARAMS, format!("tools/call 인자가 잘못됐다: {e}")))?;
    let arguments = arguments.unwrap_or_else(|| json!({}));
    let outcome = match name.as_str() {
        "list_orgs" => {
            run(arguments, |_: NoArgs| async {
                orgs::my_orgs(state, account)
                    .await
                    .map(|orgs| OrgList { orgs })
            })
            .await
        }
        "list_problems" => {
            run(arguments, |a: OrgArgs| async move {
                problems::list(state, account, &a.org_id)
                    .await
                    .map(|problems| ProblemList { problems })
            })
            .await
        }
        "get_problem" => {
            run(arguments, |a: ProblemArgs| async move {
                problems::get(state, account, &a.org_id, &a.problem_id).await
            })
            .await
        }
        "create_problem" => {
            run(arguments, |a: CreateArgs| async move {
                problems::create(state, account, &a.org_id, a.problem).await
            })
            .await
        }
        "update_problem" => {
            run(arguments, |a: UpdateArgs| async move {
                problems::update(state, account, &a.org_id, &a.problem_id, a.problem).await
            })
            .await
        }
        "start_preview" => {
            run(arguments, |a: StartPreviewArgs| async move {
                problems::start_preview(state, account, &a.org_id, a.request).await
            })
            .await
        }
        "get_preview" => {
            run(arguments, |a: PreviewArgs| async move {
                problems::preview(state, account, &a.org_id, &a.preview_id).await
            })
            .await
        }
        "publish_problem" => {
            run(arguments, |a: ProblemArgs| async move {
                problems::publish(state, account, &a.org_id, &a.problem_id).await
            })
            .await
        }
        _ => return Err((INVALID_PARAMS, format!("모르는 도구: {name}"))),
    };
    Ok(outcome)
}

/// 인자를 풀어 서비스 함수를 부르고 MCP 도구 결과로 바꾼다.
async fn run<A, T, F, Fut>(arguments: Value, f: F) -> Value
where
    A: DeserializeOwned,
    T: Serialize,
    F: FnOnce(A) -> Fut,
    Fut: Future<Output = Result<T, ApiError>>,
{
    let args = match serde_json::from_value::<A>(arguments) {
        Ok(args) => args,
        Err(e) => {
            return tool_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!("인자가 잘못됐다: {e}"),
            );
        }
    };
    match f(args).await {
        Ok(value) => {
            let value = serde_json::to_value(value).expect("API 타입은 JSON으로 바뀐다");
            json!({
                "content": [{"type": "text", "text": value.to_string()}],
                "structuredContent": value,
                "isError": false,
            })
        }
        Err(e) => tool_error(e.status(), e.message()),
    }
}

/// HTTP와 같은 상태·이유를 담은 도구 오류.
fn tool_error(status: StatusCode, message: &str) -> Value {
    let body = json!({"status": status.as_u16(), "error": message});
    json!({
        "content": [{"type": "text", "text": body.to_string()}],
        "isError": true,
    })
}
