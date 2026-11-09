//! promptlab 백엔드. [`router`]가 HTTP API 전체다(인증은 [`auth`], 조직·역할은 [`orgs`], 시험·응시 배정은 [`exams`],
//! 초대 메일은 [`mail`]) — 시험(이음매 ②)은 이 라우터에 프로세스 안에서 요청을 보낸다.
//! 대시보드(wasm) 산출물 폴더를 주면([`AppState::dashboard_dir`]) API에 없는 경로를 그 폴더에서 서빙한다
//! (없는 파일은 `index.html` — 대시보드가 주소로 화면을 고른다). `/api/…`의 없는 경로는 JSON 404다.
//! 경로·요청·응답은 [`promptlab_shared::api`] 문서에 있다.
//!
//! 불변식: 히든 테스트·모범 답안·케이스별 실행 결과는 응답에 싣지 않는다. 문제는 [`CodingProblem::public_view`]만,
//! 채점은 `GradeReport`(언어·통과 수)만 내보낸다(`tests/http_api.rs`가 응답 본문 전체를 훑어 확인한다).
//!
//! [`CodingProblem::public_view`]: promptlab_shared::coding::CodingProblem::public_view

pub mod auth;
pub mod claude;
pub mod erasure;
pub mod evaluations;
pub mod exams;
pub mod live;
pub mod logs;
pub mod mail;
pub mod mcp;
pub mod operator;
pub mod orgs;
pub mod practice;
pub mod problems;
pub mod reports;
pub mod results;
pub mod retention;
pub mod turn;
pub mod webcam;

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router};
use promptlab_shared::account::Permission;
use promptlab_shared::api::{
    CreateSubmissionRequest, ErrorResponse, HealthResponse, SubmissionView,
};
use promptlab_shared::coding::PublicCodingProblem;
use promptlab_store::{PgPool, accounts, bank, submissions};
use uuid::Uuid;

use auth::token::Clock;
use auth::{Account, AuthConfig};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

/// 라우터가 쥐는 것.
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub auth: AuthConfig,
    /// 서버 시계(유닉스 초). 시험 제한 시간·초대 만료를 잰다 — 시험에서는 고정 시계.
    pub clock: Arc<dyn Clock>,
    /// 초대 메일 발송기(설정이 없으면 [`mail::Disabled`]).
    pub mailer: Arc<dyn mail::Mailer>,
    /// 대시보드 wasm 산출물 폴더(`index.html` 등). 없으면 API만 서빙한다.
    pub dashboard_dir: Option<PathBuf>,
    /// 웹캠 사진 보유 기간(일, #13). 지난 사진은 서버가 지운다.
    pub webcam_retention_days: u32,
    /// 실시간 감독(#14): TURN 설정(없으면 꺼짐)과 열린 시그널링 세션.
    pub live: live::Live,
    /// AI 리포트 모델(#15). Anthropic API 키가 없으면 `None` — 리포트 기능만 꺼진다.
    pub reports: Option<Arc<dyn claude::ReportModel>>,
    /// sandcorp-auth 탈퇴 웹훅 비밀(#54, 여럿). 비면 `POST /auth/webhook`이 503.
    pub webhook_secrets: retention::WebhookSecrets,
}

impl AppState {
    /// 최종 점수 조립에 드는 서버 사정(#72): Claude 키가 있나, 지금 시각.
    pub fn eval_ctx(&self) -> promptlab_store::evaluations::EvalContext {
        promptlab_store::evaluations::EvalContext {
            enabled: self.reports.is_some(),
            now: self.clock.now(),
        }
    }
}

/// API 전체. 같은 경로를 `/`와 `/api` 두 곳에서 받는다(운영 프록시가 `/api/*`를 접두 그대로 넘긴다).
/// 웹캠 사진 경로의 본문 상한(사진 한 장 + 여유). 넘으면 읽지 않고 413.
fn photo_limit() -> DefaultBodyLimit {
    DefaultBodyLimit::max(promptlab_shared::webcam::MAX_PHOTO_BYTES + 1024)
}

pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/me", get(orgs::me))
        .route("/auth/logout", post(auth::login::logout))
        .route("/orgs", post(orgs::create_org))
        .route("/orgs/{id}", get(orgs::get_org))
        .route("/orgs/{id}/members", get(orgs::list_members))
        .route(
            "/orgs/{id}/members/{sub}",
            delete(orgs::remove_member).patch(orgs::update_member),
        )
        .route("/orgs/{id}/invites", post(orgs::create_invite))
        .route("/orgs/{id}/submissions", get(orgs::list_submissions))
        .route("/invites/{token}/accept", post(orgs::accept_invite))
        .route("/orgs/{id}/problem-bank", get(exams::problem_bank))
        .route(
            "/orgs/{id}/problems",
            get(problems::list_problems).post(problems::create_problem),
        )
        .route(
            "/orgs/{id}/problems/{problem}",
            get(problems::get_problem).put(problems::update_problem),
        )
        .route(
            "/orgs/{id}/problems/{problem}/publish",
            post(problems::publish_problem),
        )
        .route(
            "/orgs/{id}/problem-previews",
            post(problems::create_preview),
        )
        .route(
            "/orgs/{id}/problem-previews/{preview}",
            get(problems::get_preview),
        )
        .route(
            "/orgs/{id}/exams",
            get(exams::list_exams).post(exams::create_exam),
        )
        .route(
            "/orgs/{id}/exams/{exam}",
            get(exams::get_exam).patch(results::update_exam),
        )
        .route(
            "/orgs/{id}/exams/{exam}/results",
            get(results::exam_results),
        )
        .route(
            "/orgs/{id}/exams/{exam}/results.csv",
            get(results::exam_results_csv),
        )
        .route("/orgs/{id}/rounds", get(results::rounds))
        .route(
            "/orgs/{id}/retention",
            get(retention::get_org_retention).put(retention::set_org_retention),
        )
        .route(
            "/orgs/{id}/exams/{exam}/retention",
            get(retention::get_exam_retention).put(retention::set_exam_retention),
        )
        .route(
            "/orgs/{id}/exams/{exam}/invites",
            get(exams::list_invites).post(exams::create_invites),
        )
        .route("/exam-invites/{token}/accept", post(exams::accept_invite))
        .route("/me/assignments", get(exams::my_assignments))
        .route("/assignments/{id}/items/{n}/start", post(exams::start_item))
        .route(
            "/assignments/{id}/items/{n}/submit",
            post(exams::submit_item),
        )
        .route(
            "/assignments/{id}/items/{n}/log/keys",
            post(logs::register_key),
        )
        .route("/assignments/{id}/items/{n}/log", post(logs::upload))
        .route(
            "/orgs/{id}/exams/{exam}/invites/{invite}/items/{n}/log",
            get(logs::replay),
        )
        .route(
            "/orgs/{id}/exams/{exam}/invites/{invite}/items/{n}/activity",
            get(evaluations::item_activity),
        )
        .route(
            "/orgs/{id}/exams/{exam}/invites/{invite}/signals",
            get(logs::suspicions),
        )
        .route(
            "/orgs/{id}/exams/{exam}/invites/{invite}/report",
            get(reports::get_report).post(reports::request_report),
        )
        .route(
            "/orgs/{id}/exams/{exam}/invites/{invite}/erase",
            post(erasure::erase_assignment),
        )
        .route("/orgs/{id}/erasures", get(erasure::erasure_log))
        .route("/orgs/{id}/usage", get(reports::org_usage))
        .route("/operator/orgs", get(operator::orgs))
        .route("/operator/orgs/{id}/approval", put(operator::set_approval))
        .route(
            "/operator/problems",
            get(operator::problems).post(operator::create_problem),
        )
        .route(
            "/operator/problems/{id}",
            get(operator::problem).put(operator::update_problem),
        )
        .route(
            "/operator/problems/{id}/retirement",
            put(operator::set_retirement),
        )
        .route(
            "/operator/problems/{id}/publish",
            post(operator::publish_problem),
        )
        .route("/operator/problem-checks", post(operator::create_check))
        .route("/operator/problem-checks/{id}", get(operator::get_check))
        .route("/operator/metrics", get(operator::metrics))
        .route(
            "/operator/allowlist",
            get(operator::get_allowlist).post(operator::change_allowlist),
        )
        .route("/assignments/{id}/consent", post(webcam::consent))
        .route(
            "/assignments/{id}/webcam/baseline",
            put(webcam::put_baseline).layer(photo_limit()),
        )
        .route(
            "/assignments/{id}/items/{n}/snapshots",
            post(webcam::upload_snapshot).layer(photo_limit()),
        )
        .route(
            "/orgs/{id}/exams/{exam}/invites/{invite}/webcam",
            get(webcam::view),
        )
        .route(
            "/orgs/{id}/exams/{exam}/invites/{invite}/webcam/photos/{photo}",
            get(webcam::photo),
        )
        .route(
            "/orgs/{id}/exams/{exam}/window",
            get(live::get_window).put(live::put_window),
        )
        .route(
            "/orgs/{id}/exams/{exam}/invites/{invite}/live",
            post(live::open_live),
        )
        .route(
            "/orgs/{id}/exams/{exam}/invites/{invite}/live/{session}",
            delete(live::close_live),
        )
        .route(
            "/orgs/{id}/exams/{exam}/invites/{invite}/live/{session}/signals",
            get(live::viewer_signals).post(live::viewer_post),
        )
        .route("/assignments/{id}/live", get(live::candidate_request))
        .route(
            "/assignments/{id}/live/{session}/signals",
            get(live::candidate_signals).post(live::candidate_post),
        )
        .route("/practice/problems", get(practice::problems))
        .route("/me/practice", get(practice::records))
        .route("/problems/{id}", get(get_problem))
        .route("/submissions", post(create_submission))
        .route("/submissions/{id}", get(get_submission))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_account,
        ));
    let routes = Router::new()
        .route("/health", get(health))
        .route(
            "/.well-known/oauth-protected-resource/api",
            get(auth::login::protected_resource_metadata),
        )
        .route(
            "/oauth/app-client.json",
            get(auth::login::app_client_document),
        )
        .route("/auth/app-config", get(auth::login::app_config))
        .route("/auth/login", get(auth::login::start))
        .route("/auth/callback", get(auth::login::callback))
        // 탈퇴 알림(#54): 인증은 HMAC 서명으로 핸들러가 한다.
        .route("/auth/webhook", post(retention::withdrawal_webhook))
        // MCP 출제 서버(#17): 세션 쿠키 없이 Bearer만, 인증은 핸들러가 한다.
        .route(
            "/mcp",
            post(mcp::handle)
                .get(mcp::method_not_allowed)
                .delete(mcp::method_not_allowed),
        )
        .merge(protected);
    let dashboard_dir = state.dashboard_dir.clone();
    let router = Router::new()
        .nest("/api", routes.clone().fallback(api_not_found))
        .merge(routes)
        .with_state(state);
    match dashboard_dir {
        // 파일 이름에 해시가 없으므로(wasm-bindgen 산출물) 매번 다시 확인하게 한다(ETag·Last-Modified로 304).
        Some(dir) => router.fallback_service(
            tower::ServiceBuilder::new()
                .layer(SetResponseHeaderLayer::overriding(
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("no-cache"),
                ))
                .service(ServeDir::new(&dir).fallback(ServeFile::new(dir.join("index.html")))),
        ),
        None => router.fallback(api_not_found),
    }
}

async fn api_not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "그런 경로가 없다")
}

/// API 오류. 본문은 [`ErrorResponse`].
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!(error = %e, "DB 오류");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "서버 저장소 오류")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse {
                error: self.message,
            }),
        )
            .into_response()
    }
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    })
}

/// 연습 문제 하나(공개 보기). 시험 문항은 연습에 열지 않는다(#18) — 없는 문제와 같이 404. 내린 문항(#19)도 404.
async fn get_problem(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PublicCodingProblem>, ApiError> {
    bank::find_practice(&state.pool, &id)
        .await?
        .map(|p| Json(p.public_view()))
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "그런 연습 문제가 없다"))
}

/// 제출 코드 검사: 비었거나 [`MAX_CODE_BYTES`](promptlab_shared::coding::MAX_CODE_BYTES)를 넘으면 422.
pub(crate) fn check_code(code: &str) -> Result<(), ApiError> {
    if code.trim().is_empty() {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "제출 코드가 비었다",
        ));
    }
    if code.len() > promptlab_shared::coding::MAX_CODE_BYTES {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "제출 코드는 {}바이트까지다",
                promptlab_shared::coding::MAX_CODE_BYTES
            ),
        ));
    }
    Ok(())
}

async fn create_submission(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Json(request): Json<CreateSubmissionRequest>,
) -> Result<(StatusCode, Json<SubmissionView>), ApiError> {
    // 시험 문항에 마음껏 제출해 히든 테스트 통과 수를 볼 수 있으면 시험 유출 통로다 — 연습 문제만 받는다(#18).
    // 내린 연습 문항(#19)도 새 제출을 받지 않는다.
    if bank::find_practice(&state.pool, &request.problem_id)
        .await?
        .is_none()
    {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "그런 연습 문제가 없다",
        ));
    }
    check_code(&request.code)?;
    // 조직 응시로 내려면 그 조직의 구성원이어야 한다(아니면 404 — 조직이 있는지 알리지 않는다).
    let org_id = match &request.org_id {
        Some(org_id) => Some(orgs::membership(&state, org_id, &account).await?.org_id),
        None => None,
    };
    let owner = submissions::Owner {
        candidate_sub: account.sub.clone(),
        org_id,
        item: None,
    };
    // 개인 연습은 사람마다 채점을 기다리는 것이 MAX_PENDING개까지(큐를 혼자 채우지 못하게).
    if owner.is_practice()
        && submissions::pending_practice(&state.pool, &account.sub).await?
            >= promptlab_shared::practice::MAX_PENDING as i64
    {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            format!(
                "채점을 기다리는 연습이 {}개다 — 채점이 끝난 뒤 다시 낸다",
                promptlab_shared::practice::MAX_PENDING
            ),
        ));
    }
    let submission = submissions::NewSubmission {
        language: request.language,
        code: request.code,
        elapsed_secs: request.elapsed_secs,
    };
    let view = submissions::insert(&state.pool, &owner, &request.problem_id, &submission).await?;
    tracing::info!(id = %view.id, problem_id = %view.problem_id, "제출 저장·큐에 넣음");
    Ok((StatusCode::ACCEPTED, Json(view)))
}

/// 낸 사람, 또는 그 제출이 속한 조직에서 결과 보기 권한이 있는 구성원만 본다. 나머지는 404.
/// 시험 응시를 낸 사람 본인은 [`exams::for_candidate`]를 거친 상태만 본다.
async fn get_submission(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path(id): Path<String>,
) -> Result<Json<SubmissionView>, ApiError> {
    let not_found = || ApiError::new(StatusCode::NOT_FOUND, "그런 제출이 없다");
    let id: Uuid = id.parse().map_err(|_| not_found())?;
    let (mut view, owner) = submissions::get(&state.pool, id)
        .await?
        .ok_or_else(not_found)?;
    // 채점이 끝났으면 최종 점수·계산식(#72). 「응시 완료」만 보는 응시자에게는 아래 `for_candidate`가 지운다.
    if matches!(
        view.state,
        promptlab_shared::api::SubmissionState::Done { .. }
            | promptlab_shared::api::SubmissionState::Failed { .. }
    ) {
        view.score =
            promptlab_store::evaluations::score_of(&state.pool, id, state.eval_ctx()).await?;
    }
    if owner.candidate_sub == account.sub && owner.item.is_none() {
        return Ok(Json(view));
    }
    if let Some(org_id) = owner.org_id
        && let Some(org) = accounts::org_role(&state.pool, org_id, &account.sub).await?
        && org.role.allows(Permission::ViewResults)
    {
        return Ok(Json(view));
    }
    // 시험 응시를 응시자 본인이 볼 때: 시험 설정에 따라(채용이면 언제나) 「응시 완료」만.
    if owner.candidate_sub == account.sub
        && let Some(item) = owner.item
    {
        let (purpose, reveal) = promptlab_store::exams::exam_of_invite(&state.pool, item.invite_id)
            .await?
            .ok_or_else(not_found)?;
        return Ok(Json(exams::for_candidate(view, purpose, reveal)));
    }
    Err(not_found())
}
