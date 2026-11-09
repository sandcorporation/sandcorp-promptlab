//! 서버 시험 공통: 가짜 JWKS·고정 시계·시험 키로 서명한 토큰·가짜 토큰 끝점.
//! (sandcorp-authkit `testing`의 `FakeJwks`·`FixedClock`·`FakeTokenEndpoint`에 대응한다.)
#![allow(dead_code)]

use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use http_body_util::BodyExt;
use promptlab_server::{AppState, router};
use promptlab_store::{PgPool, catalog};
use tower::ServiceExt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signer, SigningKey};
use promptlab_server::auth::AuthConfig;
use promptlab_server::auth::login::{CodeExchange, DashboardClient, TokenEndpoint, TokenResponse};
use promptlab_server::auth::token::{
    BoxFuture, Clock, FetchError, JwksFetcher, TokenRules, TokenValidator,
};
use serde_json::{Value, json};

pub const PUBLIC_URL: &str = "https://promptlab.sandcorp.co.kr";
pub const ISSUER: &str = "https://auth.sandcorp.co.kr";
pub const AUDIENCE: &str = "https://promptlab.sandcorp.co.kr/api";
pub const OWNER_SUB: &str = "farrar142";
pub const KID: &str = "key-1";
/// 허용 목록([`rules`])에 든 스모크 기계 클라이언트.
pub const SMOKE_CLIENT: &str = "smoke-client-1";
/// 허용 목록에 없는 기계 클라이언트.
pub const OTHER_MACHINE: &str = "other-machine";
/// 시험 시작 시각.
pub const NOW: i64 = 1_767_225_600;

pub const ALICE: &str = "m_01JB2Z3Y4X5W6V7T8S9R0Q1P2A";
pub const BOB: &str = "m_01JB2Z3Y4X5W6V7T8S9R0Q1P2B";
pub const CAROL: &str = "m_01JB2Z3Y4X5W6V7T8S9R0Q1P2C";
pub const DAVE: &str = "m_01JB2Z3Y4X5W6V7T8S9R0Q1P2D";
pub const ERIN: &str = "m_01JB2Z3Y4X5W6V7T8S9R0Q1P2E";

pub fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

pub fn jwk(kid: &str, key: &SigningKey) -> Value {
    json!({
        "kty": "OKP", "crv": "Ed25519", "alg": "EdDSA", "use": "sig", "kid": kid,
        "x": URL_SAFE_NO_PAD.encode(key.verifying_key().as_bytes()),
    })
}

pub fn jwks(keys: &[Value]) -> String {
    json!({ "keys": keys }).to_string()
}

/// 고정 시계.
#[derive(Default)]
pub struct FixedClock(pub AtomicI64);

impl FixedClock {
    pub fn at(now: i64) -> Arc<Self> {
        Arc::new(Self(AtomicI64::new(now)))
    }
    pub fn advance(&self, secs: i64) {
        self.0.fetch_add(secs, Ordering::SeqCst);
    }
    pub fn set(&self, now: i64) {
        self.0.store(now, Ordering::SeqCst);
    }
}

impl Clock for FixedClock {
    fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// 가짜 JWKS. 응답을 바꿀 수 있고 조회 수를 센다.
pub struct FakeJwks {
    pub response: Mutex<Result<String, FetchError>>,
    pub calls: AtomicUsize,
}

impl FakeJwks {
    pub fn serving(body: String) -> Arc<Self> {
        Arc::new(Self {
            response: Mutex::new(Ok(body)),
            calls: AtomicUsize::new(0),
        })
    }
    pub fn set(&self, response: Result<String, FetchError>) {
        *self.response.lock().unwrap() = response;
    }
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl JwksFetcher for FakeJwks {
    fn fetch(&self) -> BoxFuture<'_, Result<String, FetchError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let response = self.response.lock().unwrap().clone();
        Box::pin(async move { response })
    }
}

pub fn rules() -> TokenRules {
    TokenRules {
        issuer: ISSUER.into(),
        audience: AUDIENCE.into(),
        owner_sub: OWNER_SUB.into(),
        machine_client_ids: [SMOKE_CLIENT.to_owned()].into(),
    }
}

/// 시험 키(`KID`, 시드 1)를 내는 JWKS와 고정 시계로 만든 검증기.
pub fn validator() -> (TokenValidator, Arc<FakeJwks>, Arc<FixedClock>) {
    let fetcher = FakeJwks::serving(jwks(&[jwk(KID, &key(1))]));
    let clock = FixedClock::at(NOW);
    (
        TokenValidator::new(rules(), fetcher.clone(), clock.clone()),
        fetcher,
        clock,
    )
}

/// 2절 모양의 일반 사용자 클레임.
pub fn member_claims(sub: &str) -> Value {
    json!({
        "iss": ISSUER, "sub": sub, "aud": AUDIENCE, "client_id": "app",
        "scope": "login promptlab", "iat": NOW, "exp": NOW + 600, "jti": "j1", "acct": "member",
    })
}

/// 12절 모양의 기계 클라이언트 클레임(`acct=machine`, `sub=client:<client_id>`).
pub fn machine_claims(client_id: &str) -> Value {
    json!({
        "iss": ISSUER, "sub": format!("client:{client_id}"), "aud": AUDIENCE, "client_id": client_id,
        "scope": "promptlab", "iat": NOW, "exp": NOW + 600, "jti": "m1", "acct": "machine",
    })
}

/// 그 기계 클라이언트의 유효한 API 토큰.
pub fn machine_token(client_id: &str) -> String {
    sign(&machine_claims(client_id))
}

pub fn sign_parts(header: &Value, claims: &Value, key: &SigningKey) -> String {
    let h = URL_SAFE_NO_PAD.encode(header.to_string());
    let c = URL_SAFE_NO_PAD.encode(claims.to_string());
    let sig = key.sign(format!("{h}.{c}").as_bytes());
    format!("{h}.{c}.{}", URL_SAFE_NO_PAD.encode(sig.to_bytes()))
}

pub fn sign(claims: &Value) -> String {
    sign_parts(
        &json!({"alg": "EdDSA", "typ": "JWT", "kid": KID}),
        claims,
        &key(1),
    )
}

/// 그 사람의 유효한 API 토큰.
pub fn token_for(sub: &str) -> String {
    sign(&member_claims(sub))
}

/// 대표 계정(운영자, #19)의 유효한 API 토큰: `acct` 없음 + `sub` = [`OWNER_SUB`].
pub fn owner_token() -> String {
    let mut claims = member_claims(OWNER_SUB);
    claims.as_object_mut().unwrap().remove("acct");
    sign(&claims)
}

/// 운영자가 조직을 승인한다(#19). 승인 전 조직은 시험을 만들 수 없으므로 시험을 만드는 시험이 부른다.
pub async fn approve_org(app: &axum::Router, org_id: &str) {
    let (status, body) = call(
        app,
        "PUT",
        &format!("/operator/orgs/{org_id}/approval"),
        Some(&owner_token()),
        Some(json!({"approved": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// 조직 문제·운영자 문항을 발행 전 점검 없이 발행한 것으로 표시한다(#73 — 점검·발행 흐름은 `problems_api.rs`·
/// `operator_api.rs`가 따로 시험한다).
#[allow(dead_code)]
pub async fn force_publish(pool: &promptlab_store::PgPool, problem_id: &str) {
    let mut done = 0;
    for table in ["org_problems", "bank_problems"] {
        done += sqlx::query(&format!(
            "UPDATE {table} SET published_at = now() WHERE id = $1"
        ))
        .bind(problem_id)
        .execute(pool)
        .await
        .unwrap()
        .rows_affected();
    }
    assert_eq!(done, 1, "{problem_id}");
}

/// 가짜 토큰 끝점: 받은 교환 요청을 남기고 정해 둔 응답을 준다.
pub struct FakeTokenEndpoint {
    pub response: Mutex<Result<TokenResponse, String>>,
    pub requests: Mutex<Vec<CodeExchange>>,
}

impl FakeTokenEndpoint {
    pub fn answering(response: Result<TokenResponse, String>) -> Arc<Self> {
        Arc::new(Self {
            response: Mutex::new(response),
            requests: Mutex::new(Vec::new()),
        })
    }
}

impl TokenEndpoint for FakeTokenEndpoint {
    fn exchange<'a>(
        &'a self,
        _client: &'a DashboardClient,
        request: CodeExchange,
    ) -> BoxFuture<'a, Result<TokenResponse, String>> {
        self.requests.lock().unwrap().push(request);
        let response = self.response.lock().unwrap().clone();
        Box::pin(async move { response })
    }
}

pub const DASHBOARD_CLIENT_ID: &str = "dash-client";

/// 시험용 인증 설정(가짜 JWKS·고정 시계·가짜 토큰 끝점).
pub fn auth_config(token_endpoint: Arc<FakeTokenEndpoint>) -> AuthConfig {
    let (validator, _, _) = validator();
    AuthConfig {
        public_url: PUBLIC_URL.into(),
        auth_url: ISSUER.into(),
        validator: Arc::new(validator),
        dashboard_client: Some(DashboardClient {
            client_id: DASHBOARD_CLIENT_ID.into(),
            client_secret: "test-secret".into(),
        }),
        token_endpoint,
    }
}

/// 시험 픽스처(조직 문제·운영자 문항, [`org_source`])의 히든 테스트 인자·기대값·모범 답안에 넣는 표식. 출제자 편집
/// 응답([`author_call`]·[`operator_call`]) 말고 어떤 응답에 보여도 샌 것이다.
pub const ORG_HIDDEN_MARK: &str = "조직숨은표식-c41d";

/// 시험 픽스처 코딩 문제 원본(`ProblemSource` JSON): `echo(s: string) -> string`. 히든 테스트 6개의 인자·기대값과
/// 모범 답안 주석에 [`ORG_HIDDEN_MARK`]를 넣는다. 예제는 공개라 표식이 없다.
#[allow(dead_code)]
pub fn org_source(title: &str) -> Value {
    let hidden: Vec<Value> = (1..=6)
        .map(|i| {
            let arg = format!("{ORG_HIDDEN_MARK}-{i}");
            json!({"args": [arg], "expected": arg})
        })
        .collect();
    json!({
        "title": title,
        "description": "문자열 s를 그대로 돌려준다.",
        "signature": {"name": "echo", "params": [{"name": "s", "type": "string"}], "returns": "string"},
        "examples": [{"args": ["abc"], "expected": "abc"}],
        "hidden": hidden,
        "reference_solution": format!("def echo(s):\n    # {ORG_HIDDEN_MARK}\n    return s\n"),
        "reference_language": "python",
        "limits": {"case_time_ms": 2000},
    })
}

/// 가짜 판정기(`promptlab_worker::fake::FakeJudge`): 내장 문제와 [`org_source`]의 모든 케이스(예제·히든)를 안다.
/// 코드에 `correct`가 있으면 모두 맞히고(`echo`는 인자를 그대로), `first-only`면 내장 문제의 첫 히든 케이스만
/// 맞히고, 그 밖은 런타임 오류(모두 실패)다.
#[allow(dead_code)]
pub fn judge() -> promptlab_worker::fake::FakeJudge {
    use std::collections::{HashMap, HashSet};
    let mut answers: HashMap<String, Value> = HashMap::new();
    let mut firsts: HashSet<String> = HashSet::new();
    for problem in catalog::all() {
        for case in problem.examples.iter().chain(&problem.hidden) {
            answers.insert(
                Value::Array(case.args.clone()).to_string(),
                case.expected.clone(),
            );
        }
        if let Some(first) = problem.hidden.first() {
            firsts.insert(Value::Array(first.args.clone()).to_string());
        }
    }
    promptlab_worker::fake::FakeJudge::new(move |code, args| {
        let key = Value::Array(args.to_vec()).to_string();
        let echo = || match args {
            [Value::String(s)] => Some(Value::String(s.clone())),
            _ => None,
        };
        if code.contains("correct") {
            answers.get(&key).cloned().or_else(echo)
        } else if code.contains("first-only") && firsts.contains(&key) {
            answers.get(&key).cloned()
        } else {
            None
        }
    })
}

/// 채점 큐에서 하나를 [`judge`]로 채점한다.
#[allow(dead_code)]
pub async fn grade(pool: &PgPool) -> promptlab_worker::Outcome {
    promptlab_worker::run_once(pool, &judge()).await.unwrap()
}

/// 가짜 인증(시험 키·고정 시계·가짜 토큰 끝점)으로 만든 라우터.
pub fn app(pool: PgPool) -> axum::Router {
    app_with(pool, FakeTokenEndpoint::answering(Err("쓰지 않음".into())))
}

pub fn app_with(pool: PgPool, token_endpoint: Arc<FakeTokenEndpoint>) -> axum::Router {
    router(AppState {
        pool,
        auth: auth_config(token_endpoint),
        clock: FixedClock::at(NOW),
        mailer: Arc::new(promptlab_server::mail::Disabled),
        dashboard_dir: None,
        webcam_retention_days: promptlab_shared::webcam::DEFAULT_RETENTION_DAYS,
        live: promptlab_server::live::Live::disabled(),
        reports: None,
        webhook_secrets: Default::default(),
    })
}

/// 서버 시계·메일 발송기·대시보드 폴더를 정한 라우터(토큰 검증 시계는 따로 `NOW`에 멈춰 있다).
pub fn app_parts(
    pool: PgPool,
    clock: Arc<FixedClock>,
    mailer: Arc<dyn promptlab_server::mail::Mailer>,
    dashboard_dir: Option<std::path::PathBuf>,
) -> axum::Router {
    router(AppState {
        pool,
        auth: auth_config(FakeTokenEndpoint::answering(Err("쓰지 않음".into()))),
        clock,
        mailer,
        dashboard_dir,
        webcam_retention_days: promptlab_shared::webcam::DEFAULT_RETENTION_DAYS,
        live: promptlab_server::live::Live::disabled(),
        reports: None,
        webhook_secrets: Default::default(),
    })
}

/// 실시간 감독(#14) 설정을 넣은 라우터. `turn`이 없으면 실시간 보기가 꺼진 서버다.
pub fn app_live(
    pool: PgPool,
    clock: Arc<FixedClock>,
    turn: Option<promptlab_server::turn::TurnConfig>,
) -> axum::Router {
    router(AppState {
        pool,
        auth: auth_config(FakeTokenEndpoint::answering(Err("쓰지 않음".into()))),
        clock,
        mailer: Arc::new(promptlab_server::mail::Disabled),
        dashboard_dir: None,
        webcam_retention_days: promptlab_shared::webcam::DEFAULT_RETENTION_DAYS,
        live: promptlab_server::live::Live::new(turn),
        reports: None,
        webhook_secrets: Default::default(),
    })
}

/// AI 리포트 모델(#15)까지 정한 서버 상태(라우터는 `router(state.clone())`).
pub fn app_state(
    pool: PgPool,
    clock: Arc<FixedClock>,
    reports: Option<Arc<dyn promptlab_server::claude::ReportModel>>,
) -> AppState {
    AppState {
        pool,
        auth: auth_config(FakeTokenEndpoint::answering(Err("쓰지 않음".into()))),
        clock,
        mailer: Arc::new(promptlab_server::mail::Disabled),
        dashboard_dir: None,
        webcam_retention_days: promptlab_shared::webcam::DEFAULT_RETENTION_DAYS,
        live: promptlab_server::live::Live::disabled(),
        reports,
        webhook_secrets: Default::default(),
    }
}

/// 문항 시작 요청 본문: 허용 목록에 있는 앱 판·고정 모델 sha256(#12).
pub fn start_body() -> Value {
    serde_json::json!({
        "app_version": promptlab_shared::integrity::ALLOWED_APP_VERSIONS[0],
        "model_sha256": promptlab_shared::inference::LARGEST_MODEL.sha256,
    })
}

/// 시험용 웹캠 사진(녹화 영상 프레임, 320×180 JPEG — 출처는 crates/vision/tests/frames/SOURCES.md).
pub const PHOTO: &[u8] = include_bytes!("../../../vision/tests/frames/frontal-1.jpg");

/// 본문을 그대로(`content_type`) 실어 요청하고 (상태, 헤더, 본문 바이트)를 준다. 숨은 입력 불변식을 건다.
pub async fn send_raw(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    content_type: &str,
    body: Vec<u8>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(body))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    assert_no_hidden(&Value::String(String::from_utf8_lossy(&bytes).to_string()));
    (status, headers, bytes)
}

/// 사람 응시자가 문항을 시작할 수 있게 웹캠 안내에 동의하고 기준 사진을 올린다(#13).
pub async fn webcam_ready(app: &axum::Router, token: &str, assignment_id: &str) {
    let (status, body) = call(
        app,
        "POST",
        &format!("/assignments/{assignment_id}/consent"),
        Some(token),
        Some(serde_json::json!({ "notice_version": promptlab_shared::webcam::NOTICE_VERSION })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _, _) = send_raw(
        app,
        "PUT",
        &format!("/assignments/{assignment_id}/webcam/baseline"),
        token,
        "image/jpeg",
        PHOTO.to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// 토큰을 실어 요청하고 (상태, 본문)을 준다. 모든 응답에 숨은 입력 불변식을 건다.
pub async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let (status, _, body) = send(app, request, body).await;
    (status, body)
}

/// 출제자 편집 응답(#16 조직 문제·미리 돌리기) 전용: 자기 조직 문제의 숨은 입력·기준 프롬프트·모델 출력이
/// 일부러 실리므로 [`assert_no_hidden`]을 걸지 않는다. 내장 문제은행의 숨은 입력은 여기서도 나가면 안 된다.
/// 성공 응답에만 쓴다 — 거부(403·404·422)는 [`call`]로 불변식을 건다.
pub async fn author_call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    sub: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    editor_call(app, method, uri, &token_for(sub), body).await
}

/// 운영자 문항 편집 응답(#19) 전용 — [`author_call`]과 같은 예외(운영자 문항의 숨은 입력이 일부러 실린다).
/// 성공 응답에만 쓴다.
pub async fn operator_call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    editor_call(app, method, uri, &owner_token(), body).await
}

async fn editor_call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let body = match body {
        Some(body) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    let response = app
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes);
    assert_no_catalog_hidden(&text);
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// 요청을 보내고 (상태, 헤더, 본문)을 준다. 모든 응답에 숨은 입력 불변식을 건다.
pub async fn send(
    app: &axum::Router,
    request: axum::http::request::Builder,
    body: Option<Value>,
) -> (StatusCode, HeaderMap, Value) {
    let request = match body {
        Some(body) => request
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => request.body(Body::empty()).unwrap(),
    };
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes);
    assert_no_hidden(&Value::String(text.to_string()));
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert_no_hidden(&body);
    (status, headers, body)
}

/// 내장 문제의 히든 테스트(공개 예제와 겹치지 않는 충분히 긴 인자 JSON)·모범 답안이 글에 없는지 본다.
pub fn assert_no_catalog_hidden(text: &str) {
    for problem in catalog::all() {
        let public: Vec<String> = problem
            .examples
            .iter()
            .map(|c| Value::Array(c.args.clone()).to_string())
            .collect();
        for case in &problem.hidden {
            let args = Value::Array(case.args.clone()).to_string();
            if args.len() >= 16 && !public.iter().any(|p| p.contains(&args)) {
                // 응답 JSON은 다시 직렬화돼 공백이 없으므로 같은 꼴로 찾는다.
                assert!(
                    !text.contains(&args),
                    "내장 문제 {}의 히든 테스트가 응답에 있다",
                    problem.id
                );
            }
        }
        let reference = problem.reference_solution.trim();
        assert!(
            !text.contains(reference),
            "내장 문제 {}의 모범 답안이 응답에 있다",
            problem.id
        );
    }
}

/// 응답 본문에 히든 테스트·모범 답안·케이스별 실행 결과가 없는지 본다. [`call`]이 모든 응답에 건다.
pub fn assert_no_hidden(body: &Value) {
    let text = body.to_string();
    assert_no_catalog_hidden(&text);
    if let Value::String(raw) = body {
        assert_no_catalog_hidden(raw);
    }
    assert!(
        !text.contains(ORG_HIDDEN_MARK),
        "조직 문제의 히든 테스트·모범 답안이 응답에 있다: {text}"
    );
    walk_keys(body, &mut |key| {
        assert!(
            !matches!(
                key,
                "hidden"
                    | "reference_solution"
                    | "failure_detail"
                    | "stdout"
                    | "stderr"
                    | "case_results"
            ),
            "내보내면 안 되는 필드 {key}: {text}"
        );
    });
}

fn walk_keys(value: &Value, visit: &mut impl FnMut(&str)) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                visit(k);
                walk_keys(v, visit);
            }
        }
        Value::Array(items) => items.iter().for_each(|v| walk_keys(v, visit)),
        _ => {}
    }
}

/// 개인 연습 제출 본문.
#[allow(dead_code)]
pub fn practice_request(
    problem_id: &str,
    code: &str,
    elapsed_secs: u32,
    org_id: Option<&str>,
) -> promptlab_shared::api::CreateSubmissionRequest {
    promptlab_shared::api::CreateSubmissionRequest {
        problem_id: problem_id.into(),
        language: promptlab_shared::coding::Language::Python,
        code: code.into(),
        elapsed_secs,
        org_id: org_id.map(Into::into),
    }
}

/// 문항 과정 로그에 사건들을 서명해 올린다(설치 키 시드 7). `uri`는 문항 경로(`/assignments/{id}/items/{n}`).
/// 올린 뒤의 `log_seq`(받은 사건 수)를 준다 — 올리기가 거부되면(마감 뒤 등) 올리기 전 값.
#[allow(dead_code)]
pub async fn upload_events(
    app: &axum::Router,
    item_uri: &str,
    token: &str,
    events: Vec<promptlab_shared::log::LogEvent>,
) -> u64 {
    use promptlab_shared::api::LogHead;
    use promptlab_shared::log::{Attempt, LogSigner, event_hash};
    let mut parts = item_uri.trim_start_matches("/api").split('/');
    let assignment_id = parts.nth(2).expect("배정 id").to_string();
    let position: u32 = parts.nth(1).expect("문항 번호").parse().unwrap();
    let signer = LogSigner::from_seed([7; 32]);
    let (_, head) = call(
        app,
        "POST",
        &format!("{item_uri}/log/keys"),
        Some(token),
        Some(json!({"public_key": signer.public_key()})),
    )
    .await;
    let Ok(head) = serde_json::from_value::<LogHead>(head) else {
        return 0;
    };
    let attempt = Attempt {
        assignment_id,
        position,
    };
    let mut prev = head.head.clone();
    let n = events.len() as u64;
    let signed: Vec<_> = events
        .into_iter()
        .enumerate()
        .map(|(i, event)| {
            let s = signer.seal(
                &attempt,
                head.next_seq + i as u64,
                &prev,
                NOW * 1000 + i as i64,
                event,
            );
            prev = event_hash(&s.body);
            s
        })
        .collect();
    let (status, _) = call(
        app,
        "POST",
        &format!("{item_uri}/log"),
        Some(token),
        Some(json!({ "events": signed })),
    )
    .await;
    if status == StatusCode::OK {
        head.next_seq + n
    } else {
        head.next_seq
    }
}

/// 시험 문항 제출 본문. 제출 URI(`/assignments/{id}/items/{n}/submit`)의 문항 로그에 언어 고름·코드 스냅숏·제출
/// 사건(언어·코드 sha256)을 올리고 `{"language": "python", "code": code, "log_seq": …}`를 돌려준다.
#[allow(dead_code)]
pub async fn logged_submit_body(
    app: &axum::Router,
    submit_uri: &str,
    token: &str,
    code: &str,
) -> Value {
    use promptlab_shared::coding::Language;
    use promptlab_shared::log::{LogEvent, code_sha256};
    let base = submit_uri
        .strip_suffix("/submit")
        .expect("제출 URI는 …/submit");
    let events = vec![
        LogEvent::LanguageChosen {
            language: Language::Python,
        },
        LogEvent::CodeSnapshot { code: code.into() },
        LogEvent::Submitted {
            language: Language::Python,
            code_sha256: code_sha256(code),
        },
    ];
    let log_seq = upload_events(app, base, token, events).await;
    json!({"language": "python", "code": code, "log_seq": log_seq})
}
