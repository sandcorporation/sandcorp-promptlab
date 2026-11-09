//! 시험 이음매 ②: MCP 출제 서버(#17) — 초기화·도구 목록·도구 호출(가짜 판정기), 인증 실패의 `WWW-Authenticate`,
//! 권한 밖 조직·문제·기계 주체 거부, 히든 테스트 불변식.
//!
//! 조직 문제의 히든 테스트·모범 답안에는 `ORG_HIDDEN_MARK`를 넣는다(`common::org_source`). 자기 조직의 시험 관리 역할이 받은 성공 결과([`author_mcp`])
//! 말고 MCP 응답(거부·다른 역할·다른 조직·목록)은 전부 `common::send`가 숨은 입력 불변식을 건다.

mod common;

use axum::http::{HeaderMap, Request, StatusCode, header};
use common::*;
use promptlab_shared::account::Role;
use promptlab_shared::api::{InviteCreated, OrgView};
use promptlab_shared::authoring::{
    OrgProblem, OrgProblemSummary, PreviewState, PreviewView, Publication,
};
use promptlab_store::PgPool;
use serde_json::{Value, json};

const METADATA_URL: &str =
    "https://promptlab.sandcorp.co.kr/.well-known/oauth-protected-resource/api";

fn rpc(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn tool(name: &str, arguments: Value) -> Value {
    rpc(
        7,
        "tools/call",
        json!({"name": name, "arguments": arguments}),
    )
}

/// 토큰을 실어 MCP 메시지를 보낸다. 응답 전체에 숨은 입력 불변식을 건다.
async fn mcp_with(
    app: &axum::Router,
    token: Option<&str>,
    message: Value,
) -> (StatusCode, HeaderMap, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/mcp")
        .header(header::ACCEPT, "application/json, text/event-stream");
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    send(app, request, Some(message)).await
}

/// 그 사람으로 도구를 부르고 결과(`result`)를 준다. 숨은 입력 불변식을 건다.
async fn mcp(app: &axum::Router, sub: &str, message: Value) -> Value {
    let (status, _, body) = mcp_with(app, Some(&token_for(sub)), message).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["result"].clone()
}

/// 자기 조직의 시험 관리 역할이 받는 성공 결과(숨은 입력이 일부러 실린다 — 내장 문제의 숨은 입력만 본다).
async fn author_mcp(app: &axum::Router, sub: &str, message: Value) -> Value {
    let (status, body) = author_call(app, "POST", "/api/mcp", sub, Some(message)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["result"].clone()
}

/// 성공한 도구 결과의 구조화 출력. 텍스트 내용도 같은 JSON이다.
fn structured<T: serde::de::DeserializeOwned>(result: &Value) -> T {
    assert_eq!(result["isError"], json!(false), "{result}");
    let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text, result["structuredContent"]);
    serde_json::from_value(result["structuredContent"].clone()).unwrap()
}

/// 도구 오류의 HTTP 상태와 같은 뜻의 번호.
fn tool_status(result: &Value) -> u64 {
    assert_eq!(result["isError"], json!(true), "{result}");
    let body: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    body["status"].as_u64().unwrap()
}

async fn http_ok<T: serde::de::DeserializeOwned>(
    app: &axum::Router,
    method: &str,
    uri: &str,
    sub: &str,
    body: Option<Value>,
    expect: StatusCode,
) -> T {
    let (status, body) = call(app, method, uri, Some(&token_for(sub)), body).await;
    assert_eq!(status, expect, "{method} {uri}: {body}");
    serde_json::from_value(body).unwrap()
}

/// ALICE(관리자)의 조직에 BOB 출제자, CAROL 감독관, DAVE 열람자.
async fn staffed_org(app: &axum::Router) -> OrgView {
    let org: OrgView = http_ok(
        app,
        "POST",
        "/orgs",
        ALICE,
        Some(json!({"name": "에이 회사", "terms_version": promptlab_shared::terms::TERMS_VERSION})),
        StatusCode::CREATED,
    )
    .await;
    common::approve_org(app, &org.id).await;
    for (sub, role) in [
        (BOB, Role::Author),
        (CAROL, Role::Proctor),
        (DAVE, Role::Viewer),
    ] {
        let invite: InviteCreated = http_ok(
            app,
            "POST",
            &format!("/orgs/{}/invites", org.id),
            ALICE,
            Some(json!({"role": role})),
            StatusCode::CREATED,
        )
        .await;
        let _: OrgView = http_ok(
            app,
            "POST",
            &format!("/invites/{}/accept", invite.token),
            sub,
            Some(json!({"terms_version": promptlab_shared::terms::TERMS_VERSION})),
            StatusCode::OK,
        )
        .await;
    }
    org
}

fn source() -> Value {
    org_source("메아리")
}

async fn create(app: &axum::Router, org: &OrgView, by: &str) -> OrgProblem {
    let result = author_mcp(
        app,
        by,
        tool(
            "create_problem",
            json!({"org_id": org.id, "problem": source()}),
        ),
    )
    .await;
    structured(&result)
}

fn schemas_have_no_refs(value: &Value) -> bool {
    match value {
        Value::Object(map) => !map.contains_key("$ref") && map.values().all(schemas_have_no_refs),
        Value::Array(items) => items.iter().all(schemas_have_no_refs),
        _ => true,
    }
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 초기화하고_도구_목록을_받는다(pool: PgPool) {
    let app = app(pool);
    let token = token_for(BOB);

    // 판 협상: 아는 판은 그대로, 모르는 판은 가장 새 판.
    let (status, _, body) = mcp_with(
        &app,
        Some(&token),
        rpc(
            1,
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "시험", "version": "0"}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], json!(1));
    assert_eq!(body["result"]["protocolVersion"], json!("2025-06-18"));
    assert!(body["result"]["capabilities"]["tools"].is_object());
    assert_eq!(body["result"]["serverInfo"]["name"], json!("promptlab"));
    let result = mcp(
        &app,
        BOB,
        rpc(2, "initialize", json!({"protocolVersion": "1999-01-01"})),
    )
    .await;
    assert_eq!(result["protocolVersion"], json!("2025-11-25"));

    // 알림은 202·본문 없음. ping은 빈 결과.
    let (status, _, _) = mcp_with(
        &app,
        Some(&token),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(mcp(&app, BOB, rpc(3, "ping", json!({}))).await, json!({}));

    // 도구 목록: 스키마에는 숨은 입력 필드 이름이 있으므로 데이터 불변식(필드 이름) 대신 표식·내장 숨은 입력만 본다.
    let list = author_mcp(&app, BOB, rpc(4, "tools/list", json!({}))).await;
    let tools = list["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "list_orgs",
            "list_problems",
            "get_problem",
            "create_problem",
            "update_problem",
            "start_preview",
            "get_preview",
            "publish_problem"
        ]
    );
    for tool in tools {
        assert_eq!(tool["inputSchema"]["type"], json!("object"), "{tool}");
        assert_eq!(tool["outputSchema"]["type"], json!("object"), "{tool}");
        assert!(schemas_have_no_refs(tool), "하위 스키마는 펼친다: {tool}");
    }
    // 입력 스키마는 HTTP API 타입(ProblemSource·PreviewRequest)에서 나온다.
    let create = &tools[3]["inputSchema"];
    assert_eq!(create["required"], json!(["org_id", "problem"]));
    let problem = &create["properties"]["problem"];
    for field in [
        "title",
        "description",
        "signature",
        "examples",
        "hidden",
        "reference_solution",
        "reference_language",
        "limits",
    ] {
        assert!(
            problem["properties"][field].is_object(),
            "{field}: {problem}"
        );
    }
    assert!(
        problem.to_string().contains("string[][]"),
        "닫힌 타입 집합: {problem}"
    );
    // 점검 결과 스키마에 언어별 틀 컴파일(#96)이 있다.
    assert_eq!(tools[6]["name"], json!("get_preview"));
    assert!(
        tools[6]["outputSchema"].to_string().contains("templates"),
        "{}",
        tools[6]["outputSchema"]
    );
    let preview = &tools[5]["inputSchema"]["properties"];
    assert!(preview["problem_id"].is_object() && preview["org_id"].is_object());

    // 모르는 메서드·도구는 JSON-RPC 오류.
    let (_, _, body) = mcp_with(&app, Some(&token), rpc(5, "resources/list", json!({}))).await;
    assert_eq!(body["error"]["code"], json!(-32601));
    let (_, _, body) = mcp_with(&app, Some(&token), tool("delete_everything", json!({}))).await;
    assert_eq!(body["error"]["code"], json!(-32602));
    // 묶음·JSON 아님은 400.
    let (status, _, body) = mcp_with(&app, Some(&token), json!([rpc(6, "ping", json!({}))])).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], json!(-32600));
    // 모르는 MCP-Protocol-Version 헤더는 400.
    let request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header("mcp-protocol-version", "1999-01-01");
    let (status, _, _) = send(&app, request, Some(rpc(1, "ping", json!({})))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // 서버가 여는 SSE 흐름은 없다.
    let request = Request::builder()
        .method("GET")
        .uri("/api/mcp")
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let (status, _, _) = send(&app, request, None).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 인증_실패는_보호_리소스_메타데이터를_가리킨다(pool: PgPool) {
    let app = app(pool);
    let www = |headers: &HeaderMap| {
        headers
            .get(header::WWW_AUTHENTICATE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned()
    };
    let ping = || rpc(1, "ping", json!({}));

    // 토큰 없음.
    let (status, headers, _) = mcp_with(&app, None, ping()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        www(&headers),
        format!("Bearer resource_metadata=\"{METADATA_URL}\"")
    );
    // `/mcp`(접두 없음)도 같다.
    let (status, headers, _) = send(
        &app,
        Request::builder().method("POST").uri("/mcp"),
        Some(ping()),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(www(&headers).contains(METADATA_URL));

    // 대시보드 세션 쿠키는 MCP에서 받지 않는다(토큰 없음과 같다).
    let request = Request::builder()
        .method("POST")
        .uri("/api/mcp")
        .header(header::COOKIE, "pl_session=anything");
    let (status, headers, _) = send(&app, request, Some(ping())).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(www(&headers).contains(METADATA_URL));

    // 잘못된 aud.
    let mut claims = member_claims(BOB);
    claims["aud"] = json!("https://sandcorp.co.kr/mcp");
    let (status, headers, _) = mcp_with(&app, Some(&sign(&claims)), ping()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        www(&headers),
        format!("Bearer resource_metadata=\"{METADATA_URL}\", error=\"invalid_token\"")
    );

    // 스코프 없음: auth 문서 5절대로 403 insufficient_scope(필요한 스코프를 알린다).
    let mut claims = member_claims(BOB);
    claims["scope"] = json!("login");
    let (status, headers, _) = mcp_with(&app, Some(&sign(&claims)), ping()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        www(&headers),
        "Bearer error=\"insufficient_scope\", scope=\"promptlab\""
    );

    // 메타데이터 문서: 리소스 `…/api`, 권한 서버 sandcorp-auth, 스코프 promptlab.
    for uri in [
        "/.well-known/oauth-protected-resource/api",
        "/api/.well-known/oauth-protected-resource/api",
    ] {
        let (status, body) = call(&app, "GET", uri, None, None).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert_eq!(body["resource"], json!(AUDIENCE));
        assert_eq!(body["authorization_servers"], json!([ISSUER]));
        assert_eq!(body["scopes_supported"], json!(["promptlab"]));
    }
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 기계_주체는_mcp를_쓸_수_없다(pool: PgPool) {
    let app = app(pool);
    let (status, _, body) = mcp_with(
        &app,
        Some(&machine_token(SMOKE_CLIENT)),
        rpc(1, "initialize", json!({"protocolVersion": "2025-06-18"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.get("result").is_none());
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 출제자가_도구로_문제를_만들고_고친다(pool: PgPool) {
    let app = app(pool.clone());
    let org = staffed_org(&app).await;

    // 내 조직과 역할.
    let orgs: Value = structured(&mcp(&app, BOB, tool("list_orgs", json!({}))).await);
    assert_eq!(orgs["orgs"][0]["id"], json!(org.id));
    assert_eq!(orgs["orgs"][0]["my_role"], json!("author"));

    // 만들기 → 목록(숨은 입력 없음, 불변식 검사) → 조회 → 고치기. 관리자도 본다.
    let created = create(&app, &org, BOB).await;
    assert!(created.id.starts_with("org-"));
    assert!(
        created.source.hidden[0].args[0]
            .to_string()
            .contains(ORG_HIDDEN_MARK)
    );
    let list: Value = structured(
        &mcp(
            &app,
            ALICE,
            tool("list_problems", json!({"org_id": org.id})),
        )
        .await,
    );
    let list: Vec<OrgProblemSummary> = serde_json::from_value(list["problems"].clone()).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].hidden_count, 6);
    let got: OrgProblem = structured(
        &author_mcp(
            &app,
            ALICE,
            tool(
                "get_problem",
                json!({"org_id": org.id, "problem_id": created.id}),
            ),
        )
        .await,
    );
    assert_eq!(got, created);
    let mut changed = source();
    changed["description"] = json!("고친 설명");
    let updated: OrgProblem = structured(
        &author_mcp(
            &app,
            BOB,
            tool(
                "update_problem",
                json!({"org_id": org.id, "problem_id": created.id, "problem": changed}),
            ),
        )
        .await,
    );
    assert_eq!(updated.id, created.id);
    assert_eq!(updated.source.description, "고친 설명");
    // HTTP 편집 API와 같은 저장소다.
    let via_http: Value = {
        let (status, body) = author_call(
            &app,
            "GET",
            &format!("/orgs/{}/problems/{}", org.id, created.id),
            BOB,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        body
    };
    assert_eq!(via_http["source"]["description"], json!("고친 설명"));

    // 검증에 걸리면 422(HTTP와 같은 까닭), 인자 모양이 틀려도 422.
    let mut bad = source();
    bad["hidden"] = json!([]);
    let result = mcp(
        &app,
        BOB,
        tool("create_problem", json!({"org_id": org.id, "problem": bad})),
    )
    .await;
    assert_eq!(tool_status(&result), 422);
    let result = mcp(&app, BOB, tool("get_problem", json!({"org_id": org.id}))).await;
    assert_eq!(tool_status(&result), 422);

    // 발행 전 점검은 저장된 문제만 받는다(없으면 404).
    let result = mcp(
        &app,
        BOB,
        tool(
            "start_preview",
            json!({"org_id": org.id, "problem_id": "org-없음"}),
        ),
    )
    .await;
    assert_eq!(tool_status(&result), 404);
    let result = mcp(
        &app,
        BOB,
        tool(
            "get_preview",
            json!({"org_id": org.id, "preview_id": "없는-id"}),
        ),
    )
    .await;
    assert_eq!(tool_status(&result), 404);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 권한_밖_조직과_문제는_거부하고_숨은_입력을_내보내지_않는다(
    pool: PgPool,
) {
    let app = app(pool);
    let org = staffed_org(&app).await;
    let problem = create(&app, &org, BOB).await;
    // ERIN은 다른 조직의 관리자.
    let other: OrgView = http_ok(
        &app,
        "POST",
        "/orgs",
        ERIN,
        Some(json!({"name": "비 회사", "terms_version": promptlab_shared::terms::TERMS_VERSION})),
        StatusCode::CREATED,
    )
    .await;

    let get = |org_id: &str| {
        tool(
            "get_problem",
            json!({"org_id": org_id, "problem_id": problem.id}),
        )
    };
    let update = |org_id: &str| {
        tool(
            "update_problem",
            json!({"org_id": org_id, "problem_id": problem.id, "problem": source()}),
        )
    };
    let preview = |org_id: &str| {
        tool(
            "start_preview",
            json!({"org_id": org_id, "problem_id": problem.id}),
        )
    };
    let publish = |org_id: &str| {
        tool(
            "publish_problem",
            json!({"org_id": org_id, "problem_id": problem.id}),
        )
    };
    // 감독관·열람자: 403(구성원이지만 역할이 모자란다). 응답은 전부 불변식 검사를 거친다.
    for sub in [CAROL, DAVE] {
        for message in [
            tool("list_problems", json!({"org_id": org.id})),
            get(&org.id),
            update(&org.id),
            preview(&org.id),
            publish(&org.id),
        ] {
            assert_eq!(tool_status(&mcp(&app, sub, message).await), 403);
        }
    }
    // 다른 조직 관리자: 남의 조직은 404, 자기 조직 id로 남의 문제를 찾아도 404.
    for message in [
        tool("list_problems", json!({"org_id": org.id})),
        get(&org.id),
        update(&org.id),
        preview(&org.id),
        publish(&org.id),
        get(&other.id),
        update(&other.id),
        preview(&other.id),
        publish(&other.id),
    ] {
        assert_eq!(tool_status(&mcp(&app, ERIN, message).await), 404);
    }
    let list: Value = structured(
        &mcp(
            &app,
            ERIN,
            tool("list_problems", json!({"org_id": other.id})),
        )
        .await,
    );
    assert_eq!(list["problems"], json!([]));
    // 고치기를 거부당한 문제는 그대로다.
    let still: OrgProblem = structured(&author_mcp(&app, BOB, get(&org.id)).await);
    assert_eq!(still, problem);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn mcp로_점검하고_발행하면_http와_같은_결과다(pool: PgPool) {
    let app = app(pool.clone());
    let org = staffed_org(&app).await;
    let source = source();
    let result = author_mcp(
        &app,
        BOB,
        tool(
            "create_problem",
            json!({"org_id": org.id, "problem": source}),
        ),
    )
    .await;
    let problem: OrgProblem = structured(&result);
    assert!(!problem.publication.published);

    let started: PreviewView = structured(
        &mcp(
            &app,
            BOB,
            tool(
                "start_preview",
                json!({"org_id": org.id, "problem_id": problem.id}),
            ),
        )
        .await,
    );
    // 모범 답안(`return s`)은 인자를 그대로 돌려준다.
    let judge = promptlab_worker::fake::FakeJudge::new(|code, args| match args {
        [Value::String(s)] if code.contains("return s") => Some(Value::String(s.clone())),
        _ => None,
    });
    while promptlab_worker::run_once(&pool, &judge).await.unwrap()
        != promptlab_worker::Outcome::Idle
    {}
    let by_mcp: PreviewView = structured(
        &mcp(
            &app,
            BOB,
            tool(
                "get_preview",
                json!({"org_id": org.id, "preview_id": started.id}),
            ),
        )
        .await,
    );
    let by_http: PreviewView = http_ok(
        &app,
        "GET",
        &format!("/orgs/{}/problem-previews/{}", org.id, started.id),
        BOB,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(by_mcp, by_http);
    assert_eq!(by_mcp.state, PreviewState::Done);
    assert!(by_mcp.publishable, "{by_mcp:?}");

    let published: Publication = structured(
        &mcp(
            &app,
            BOB,
            tool(
                "publish_problem",
                json!({"org_id": org.id, "problem_id": problem.id}),
            ),
        )
        .await,
    );
    assert!(published.published);
    let list: Vec<OrgProblemSummary> = http_ok(
        &app,
        "GET",
        &format!("/orgs/{}/problems", org.id),
        BOB,
        None,
        StatusCode::OK,
    )
    .await;
    assert!(list.iter().all(|p| p.published));
}
