//! 대시보드 → 서버 API(같은 출처의 `/api`, 세션 쿠키). 네트워크·브라우저 접근은 wasm32에서만 있고,
//! 네이티브 빌드(컴포넌트만 컴파일)에서는 「웹에서만」 오류를 돌려준다.

use promptlab_shared::account::Role;
use promptlab_shared::api::{
    AssignmentView, CreateExamRequest, ExamInviteView, ExamInvitesCreated, ExamResults, ExamView,
    InviteCreated, ItemActivity, MeView, MemberRounds, MemberView, OrgView, ProcessLogView,
    SuspicionView, UpdateExamRequest,
};
use promptlab_shared::authoring::{
    OrgProblem, OrgProblemSummary, PreviewRequest, PreviewView, ProblemSource, Publication,
};
use promptlab_shared::coding::PublicCodingProblem;
use promptlab_shared::erasure::{EraseRequest, ErasureLog, ErasureRecord};
use promptlab_shared::exam::ExamSettings;
use promptlab_shared::live::{ExamWindow, ExamWindowView, LiveSession, PostSignal, SignalBatch};
use promptlab_shared::operator::{
    AllowlistChangeRequest, AllowlistView, ApprovalRequest, BankProblem, BankProblemSummary,
    CreateBankProblemRequest, OperatorMetrics, OperatorOrg, RetireRequest,
};
use promptlab_shared::practice::PracticeRecords;
use promptlab_shared::report::{OrgUsage, ReportView};
use promptlab_shared::retention::{RetentionView, SetRetentionRequest};
use promptlab_shared::terms::TERMS_VERSION;

/// API 접두. 운영 프록시와 개발 서버(`Dioxus.toml`)가 `/api/*`를 서버로 넘긴다.
pub const API: &str = "/api";

// 네이티브 빌드에서는 서버를 부르지 않아 401·거절 변형이 생기지 않는다.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// 로그인이 필요하다(401).
    Unauthorized,
    /// 서버가 거절했다.
    Rejected(u16, String),
    Network(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unauthorized => f.write_str("로그인이 필요합니다"),
            Error::Rejected(status, message) => write!(f, "{message} ({status})"),
            Error::Network(e) => write!(f, "서버에 닿지 못했습니다: {e}"),
        }
    }
}

/// 로그인 시작 주소(돌아올 경로를 함께).
pub fn login_url(next: &str) -> String {
    let next: String = url_encode(next);
    format!("{API}/auth/login?next={next}")
}

fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub async fn me() -> Result<MeView, Error> {
    imp::request("GET", "/me", None).await
}

pub async fn create_org(name: &str) -> Result<OrgView, Error> {
    imp::request(
        "POST",
        "/orgs",
        Some(serde_json::json!({ "name": name, "terms_version": TERMS_VERSION })),
    )
    .await
}

pub async fn members(org_id: &str) -> Result<Vec<MemberView>, Error> {
    imp::request("GET", &format!("/orgs/{org_id}/members"), None).await
}

pub async fn set_role(org_id: &str, sub: &str, role: Role) -> Result<MemberView, Error> {
    imp::request(
        "PATCH",
        &format!("/orgs/{org_id}/members/{sub}"),
        Some(serde_json::json!({ "role": role })),
    )
    .await
}

pub async fn remove_member(org_id: &str, sub: &str) -> Result<(), Error> {
    imp::request_empty("DELETE", &format!("/orgs/{org_id}/members/{sub}")).await
}

pub async fn create_invite(org_id: &str, role: Role) -> Result<InviteCreated, Error> {
    imp::request(
        "POST",
        &format!("/orgs/{org_id}/invites"),
        Some(serde_json::json!({ "role": role })),
    )
    .await
}

pub async fn accept_invite(token: &str) -> Result<OrgView, Error> {
    imp::request(
        "POST",
        &format!("/invites/{}/accept", url_encode(token)),
        Some(serde_json::json!({ "terms_version": TERMS_VERSION })),
    )
    .await
}

/// 내 연습 기록(#18).
pub async fn my_practice() -> Result<PracticeRecords, Error> {
    imp::request("GET", "/me/practice", None).await
}

/// 연습 문제 목록(#18).
pub async fn practice_problems() -> Result<Vec<PublicCodingProblem>, Error> {
    imp::request("GET", "/practice/problems", None).await
}

pub async fn problem_bank(org_id: &str) -> Result<Vec<PublicCodingProblem>, Error> {
    imp::request("GET", &format!("/orgs/{org_id}/problem-bank"), None).await
}

/// 조직 문제 목록(#16).
pub async fn org_problems(org_id: &str) -> Result<Vec<OrgProblemSummary>, Error> {
    imp::request("GET", &format!("/orgs/{org_id}/problems"), None).await
}

/// 조직 문제 하나(편집용, 숨은 입력 포함).
pub async fn org_problem(org_id: &str, problem_id: &str) -> Result<OrgProblem, Error> {
    imp::request(
        "GET",
        &format!("/orgs/{org_id}/problems/{}", url_encode(problem_id)),
        None,
    )
    .await
}

/// 조직 문제를 만들거나(`problem_id` 없음) 고친다.
pub async fn save_org_problem(
    org_id: &str,
    problem_id: Option<&str>,
    source: &ProblemSource,
) -> Result<OrgProblem, Error> {
    let body = serde_json::to_value(source).map_err(|e| Error::Network(e.to_string()))?;
    match problem_id {
        Some(id) => {
            imp::request(
                "PUT",
                &format!("/orgs/{org_id}/problems/{}", url_encode(id)),
                Some(body),
            )
            .await
        }
        None => imp::request("POST", &format!("/orgs/{org_id}/problems"), Some(body)).await,
    }
}

/// 풀림 미리 돌리기를 큐에 넣는다.
pub async fn start_preview(org_id: &str, request: &PreviewRequest) -> Result<PreviewView, Error> {
    let body = serde_json::to_value(request).map_err(|e| Error::Network(e.to_string()))?;
    imp::request(
        "POST",
        &format!("/orgs/{org_id}/problem-previews"),
        Some(body),
    )
    .await
}

/// 미리 돌리기 상태.
pub async fn preview(org_id: &str, preview_id: &str) -> Result<PreviewView, Error> {
    imp::request(
        "GET",
        &format!("/orgs/{org_id}/problem-previews/{}", url_encode(preview_id)),
        None,
    )
    .await
}

pub async fn exams(org_id: &str) -> Result<Vec<ExamView>, Error> {
    imp::request("GET", &format!("/orgs/{org_id}/exams"), None).await
}

pub async fn create_exam(org_id: &str, request: &CreateExamRequest) -> Result<ExamView, Error> {
    let body = serde_json::to_value(request).map_err(|e| Error::Network(e.to_string()))?;
    imp::request("POST", &format!("/orgs/{org_id}/exams"), Some(body)).await
}

pub async fn invite_to_exam(
    org_id: &str,
    exam_id: &str,
    emails: Vec<String>,
) -> Result<ExamInvitesCreated, Error> {
    imp::request(
        "POST",
        &format!("/orgs/{org_id}/exams/{exam_id}/invites"),
        Some(serde_json::json!({ "emails": emails })),
    )
    .await
}

pub async fn exam_invites(org_id: &str, exam_id: &str) -> Result<Vec<ExamInviteView>, Error> {
    imp::request(
        "GET",
        &format!("/orgs/{org_id}/exams/{exam_id}/invites"),
        None,
    )
    .await
}

/// 시험의 응시 현황·점수·과정 지표(#10).
pub async fn exam_results(org_id: &str, exam_id: &str) -> Result<ExamResults, Error> {
    imp::request(
        "GET",
        &format!("/orgs/{org_id}/exams/{exam_id}/results"),
        None,
    )
    .await
}

/// 시험의 채점 설정·점수 공개를 바꾼다(준 칸만, #71·#10).
pub async fn update_exam(
    org_id: &str,
    exam_id: &str,
    reveal_scores: Option<bool>,
    settings: Option<&ExamSettings>,
) -> Result<ExamView, Error> {
    let body = to_body(&UpdateExamRequest {
        reveal_scores,
        settings: settings.cloned(),
    })?;
    imp::request(
        "PATCH",
        &format!("/orgs/{org_id}/exams/{exam_id}"),
        Some(body),
    )
    .await
}

/// 조직 문제를 발행한다(#73) — 지금 원본의 마지막 점검이 모두 통과일 때만.
pub async fn publish_org_problem(org_id: &str, problem_id: &str) -> Result<Publication, Error> {
    imp::request(
        "POST",
        &format!("/orgs/{org_id}/problems/{}/publish", url_encode(problem_id)),
        None,
    )
    .await
}

/// 운영자 문항의 발행 전 점검을 큐에 넣는다(#73).
pub async fn start_operator_check(request: &PreviewRequest) -> Result<PreviewView, Error> {
    imp::request("POST", "/operator/problem-checks", Some(to_body(request)?)).await
}

/// 운영자 문항 점검 상태.
pub async fn operator_check(check_id: &str) -> Result<PreviewView, Error> {
    imp::request(
        "GET",
        &format!("/operator/problem-checks/{}", url_encode(check_id)),
        None,
    )
    .await
}

/// 운영자 문항을 발행한다(#73).
pub async fn publish_operator_problem(id: &str) -> Result<Publication, Error> {
    imp::request(
        "POST",
        &format!("/operator/problems/{}/publish", url_encode(id)),
        None,
    )
    .await
}

/// 같은 구성원의 사내 진단 회차(#10).
pub async fn member_rounds(org_id: &str) -> Result<Vec<MemberRounds>, Error> {
    imp::request("GET", &format!("/orgs/{org_id}/rounds"), None).await
}

/// 응시 문항 하나의 과정 다시보기(#9).
pub async fn process_log(
    org_id: &str,
    exam_id: &str,
    invite_id: &str,
    position: u32,
) -> Result<ProcessLogView, Error> {
    imp::request(
        "GET",
        &format!("/orgs/{org_id}/exams/{exam_id}/invites/{invite_id}/items/{position}/log"),
        None,
    )
    .await
}

/// 응시 문항 하나의 AI 대화·편집 제안·로컬 실행·최종 코드·평가(#99, 결과 보기 권한).
pub async fn item_activity(
    org_id: &str,
    exam_id: &str,
    invite_id: &str,
    position: u32,
) -> Result<ItemActivity, Error> {
    imp::request(
        "GET",
        &format!("/orgs/{org_id}/exams/{exam_id}/invites/{invite_id}/items/{position}/activity"),
        None,
    )
    .await
}

/// 응시 하나의 웹캠 사진 목록(#13, 감독 권한).
pub async fn webcam(
    org_id: &str,
    exam_id: &str,
    invite_id: &str,
) -> Result<promptlab_shared::webcam::WebcamView, Error> {
    imp::request(
        "GET",
        &format!("/orgs/{org_id}/exams/{exam_id}/invites/{invite_id}/webcam"),
        None,
    )
    .await
}

/// 응시자 한 명의 의심 신호(#12).
pub async fn suspicions(
    org_id: &str,
    exam_id: &str,
    invite_id: &str,
) -> Result<SuspicionView, Error> {
    imp::request(
        "GET",
        &format!("/orgs/{org_id}/exams/{exam_id}/invites/{invite_id}/signals"),
        None,
    )
    .await
}

/// 채용 시험의 응시 가능 시간(#14, 감독 권한).
pub async fn exam_window(org_id: &str, exam_id: &str) -> Result<ExamWindowView, Error> {
    imp::request(
        "GET",
        &format!("/orgs/{org_id}/exams/{exam_id}/window"),
        None,
    )
    .await
}

/// 보관 기간(#54). `exam_id`가 없으면 조직 기본값.
/// 응시자 데이터 삭제(#55, 관리자). 이미 지운 배정이면 처음 기록이 온다.
pub async fn erase_assignment(
    org_id: &str,
    exam_id: &str,
    invite_id: &str,
    reason: Option<String>,
) -> Result<ErasureRecord, Error> {
    imp::request(
        "POST",
        &format!("/orgs/{org_id}/exams/{exam_id}/invites/{invite_id}/erase"),
        Some(serde_json::to_value(EraseRequest { reason }).unwrap_or_default()),
    )
    .await
}

/// 삭제 감사 로그(#55, 관리자). `email`을 주면 그 응시자 기록만(서버가 해시로 거른다).
pub async fn erasures(org_id: &str, email: Option<&str>) -> Result<ErasureLog, Error> {
    let path = match email {
        Some(email) => format!("/orgs/{org_id}/erasures?email={}", url_encode(email)),
        None => format!("/orgs/{org_id}/erasures"),
    };
    imp::request("GET", &path, None).await
}

fn retention_path(org_id: &str, exam_id: Option<&str>) -> String {
    match exam_id {
        Some(exam_id) => format!("/orgs/{org_id}/exams/{exam_id}/retention"),
        None => format!("/orgs/{org_id}/retention"),
    }
}

pub async fn retention(org_id: &str, exam_id: Option<&str>) -> Result<RetentionView, Error> {
    imp::request("GET", &retention_path(org_id, exam_id), None).await
}

pub async fn set_retention(
    org_id: &str,
    exam_id: Option<&str>,
    days: Option<u32>,
) -> Result<RetentionView, Error> {
    imp::request(
        "PUT",
        &retention_path(org_id, exam_id),
        Some(serde_json::to_value(SetRetentionRequest { days }).unwrap_or_default()),
    )
    .await
}

/// 응시 가능 시간을 열고 닫는다(#14).
pub async fn set_exam_window(
    org_id: &str,
    exam_id: &str,
    window: &ExamWindow,
) -> Result<ExamWindowView, Error> {
    imp::request(
        "PUT",
        &format!("/orgs/{org_id}/exams/{exam_id}/window"),
        Some(serde_json::to_value(window).unwrap_or_default()),
    )
    .await
}

fn live_path(org_id: &str, exam_id: &str, invite_id: &str) -> String {
    format!("/orgs/{org_id}/exams/{exam_id}/invites/{invite_id}/live")
}

/// 실시간 보기를 연다(#14) — 앱이 다음 폴링에서 송출을 시작한다.
pub async fn open_live(org_id: &str, exam_id: &str, invite_id: &str) -> Result<LiveSession, Error> {
    imp::request("POST", &live_path(org_id, exam_id, invite_id), None).await
}

/// 앱이 보낸 시그널링 메시지(`after` 뒤). 이 폴링이 곧 「감독관이 아직 보고 있다」는 신호다.
pub async fn live_signals(
    org_id: &str,
    exam_id: &str,
    invite_id: &str,
    session_id: &str,
    after: u64,
) -> Result<SignalBatch, Error> {
    imp::request(
        "GET",
        &format!(
            "{}/{session_id}/signals?after={after}",
            live_path(org_id, exam_id, invite_id)
        ),
        None,
    )
    .await
}

/// answer·ICE 후보를 보낸다.
pub async fn post_live_signal(
    org_id: &str,
    exam_id: &str,
    invite_id: &str,
    session_id: &str,
    signal: &PostSignal,
) -> Result<(), Error> {
    imp::send_no_content(
        "POST",
        &format!(
            "{}/{session_id}/signals",
            live_path(org_id, exam_id, invite_id)
        ),
        serde_json::to_value(signal).unwrap_or_default(),
    )
    .await
}

/// 실시간 보기를 닫는다(앱이 송출을 멈춘다).
pub async fn close_live(
    org_id: &str,
    exam_id: &str,
    invite_id: &str,
    session_id: &str,
) -> Result<(), Error> {
    imp::request_empty(
        "DELETE",
        &format!("{}/{session_id}", live_path(org_id, exam_id, invite_id)),
    )
    .await
}

/// 응시자 한 명의 AI 리포트(#15, 결과 보기).
pub async fn report(org_id: &str, exam_id: &str, invite_id: &str) -> Result<ReportView, Error> {
    imp::request(
        "GET",
        &format!("/orgs/{org_id}/exams/{exam_id}/invites/{invite_id}/report"),
        None,
    )
    .await
}

/// AI 리포트 (다시) 만들기 요청(#15).
pub async fn request_report(
    org_id: &str,
    exam_id: &str,
    invite_id: &str,
) -> Result<ReportView, Error> {
    imp::request(
        "POST",
        &format!("/orgs/{org_id}/exams/{exam_id}/invites/{invite_id}/report"),
        None,
    )
    .await
}

/// 조직의 응시 수·토큰·추정 비용(#15, 구성원 관리).
pub async fn usage(org_id: &str) -> Result<OrgUsage, Error> {
    imp::request("GET", &format!("/orgs/{org_id}/usage"), None).await
}

fn to_body<T: serde::Serialize>(value: &T) -> Result<serde_json::Value, Error> {
    serde_json::to_value(value).map_err(|e| Error::Network(e.to_string()))
}

// ── 운영자 화면(#19, 대표 계정만) ──

pub async fn operator_orgs() -> Result<Vec<OperatorOrg>, Error> {
    imp::request("GET", "/operator/orgs", None).await
}

pub async fn set_org_approval(org_id: &str, approved: bool) -> Result<OperatorOrg, Error> {
    imp::request(
        "PUT",
        &format!("/operator/orgs/{}/approval", url_encode(org_id)),
        Some(to_body(&ApprovalRequest { approved })?),
    )
    .await
}

pub async fn operator_problems() -> Result<Vec<BankProblemSummary>, Error> {
    imp::request("GET", "/operator/problems", None).await
}

pub async fn operator_problem(id: &str) -> Result<BankProblem, Error> {
    imp::request(
        "GET",
        &format!("/operator/problems/{}", url_encode(id)),
        None,
    )
    .await
}

/// 운영자 문항을 만들거나(`id` 없음) 고친다.
pub async fn save_operator_problem(
    id: Option<&str>,
    practice: bool,
    source: &ProblemSource,
) -> Result<BankProblem, Error> {
    match id {
        Some(id) => {
            imp::request(
                "PUT",
                &format!("/operator/problems/{}", url_encode(id)),
                Some(to_body(source)?),
            )
            .await
        }
        None => {
            imp::request(
                "POST",
                "/operator/problems",
                Some(to_body(&CreateBankProblemRequest {
                    practice,
                    source: source.clone(),
                })?),
            )
            .await
        }
    }
}

pub async fn set_retirement(
    id: &str,
    retired: bool,
    reason: &str,
) -> Result<BankProblemSummary, Error> {
    imp::request(
        "PUT",
        &format!("/operator/problems/{}/retirement", url_encode(id)),
        Some(to_body(&RetireRequest {
            retired,
            reason: reason.to_owned(),
        })?),
    )
    .await
}

pub async fn operator_metrics() -> Result<OperatorMetrics, Error> {
    imp::request("GET", "/operator/metrics", None).await
}

pub async fn allowlist() -> Result<AllowlistView, Error> {
    imp::request("GET", "/operator/allowlist", None).await
}

pub async fn change_allowlist(request: &AllowlistChangeRequest) -> Result<AllowlistView, Error> {
    imp::request("POST", "/operator/allowlist", Some(to_body(request)?)).await
}

pub async fn accept_exam_invite(token: &str) -> Result<AssignmentView, Error> {
    imp::request(
        "POST",
        &format!("/exam-invites/{}/accept", url_encode(token)),
        None,
    )
    .await
}

pub async fn logout() -> Result<(), Error> {
    imp::request_empty("POST", "/auth/logout").await
}

pub use imp::{
    close_modal, copy_text, current_hash, current_path, current_query, navigate, on_hash_change,
    origin, replace_path, scroll_into_view, show_modal, sleep_ms,
};

#[cfg(target_arch = "wasm32")]
mod imp {
    use super::{API, Error};
    use gloo_net::http::{Request, RequestBuilder};
    use promptlab_shared::api::ErrorResponse;
    use serde::de::DeserializeOwned;

    fn builder(method: &str, path: &str) -> RequestBuilder {
        let url = format!("{API}{path}");
        match method {
            "POST" => Request::post(&url),
            "PATCH" => Request::patch(&url),
            "PUT" => Request::put(&url),
            "DELETE" => Request::delete(&url),
            _ => Request::get(&url),
        }
    }

    async fn send(
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<gloo_net::http::Response, Error> {
        let builder = builder(method, path);
        let request = match body {
            Some(body) => builder
                .json(&body)
                .map_err(|e| Error::Network(e.to_string()))?,
            None => builder.build().map_err(|e| Error::Network(e.to_string()))?,
        };
        let response = request
            .send()
            .await
            .map_err(|e| Error::Network(e.to_string()))?;
        match response.status() {
            200..=299 => Ok(response),
            401 => Err(Error::Unauthorized),
            status => {
                let message = response
                    .json::<ErrorResponse>()
                    .await
                    .map(|e| e.error)
                    .unwrap_or_else(|_| "요청이 거절됐습니다".into());
                Err(Error::Rejected(status, message))
            }
        }
    }

    pub async fn request<T: DeserializeOwned>(
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T, Error> {
        send(method, path, body)
            .await?
            .json()
            .await
            .map_err(|e| Error::Network(e.to_string()))
    }

    pub async fn request_empty(method: &str, path: &str) -> Result<(), Error> {
        send(method, path, None).await.map(|_| ())
    }

    /// 본문을 보내고 빈 응답(204)을 받는다.
    pub async fn send_no_content(
        method: &str,
        path: &str,
        body: serde_json::Value,
    ) -> Result<(), Error> {
        send(method, path, Some(body)).await.map(|_| ())
    }

    fn location() -> Option<web_sys::Location> {
        web_sys::window().map(|w| w.location())
    }

    pub fn current_path() -> String {
        location()
            .and_then(|l| l.pathname().ok())
            .unwrap_or_else(|| "/".into())
    }

    pub fn current_query() -> String {
        location().and_then(|l| l.search().ok()).unwrap_or_default()
    }

    pub fn origin() -> String {
        location().and_then(|l| l.origin().ok()).unwrap_or_default()
    }

    /// 주소창의 경로만 바꾼다(새로 불러오지 않는다).
    pub fn replace_path(path: &str) {
        if let Some(history) = web_sys::window().and_then(|w| w.history().ok()) {
            let _ = history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(path));
        }
    }

    /// 기다린다(폴링 간격).
    pub async fn sleep_ms(ms: u32) {
        gloo_timers::future::TimeoutFuture::new(ms).await;
    }

    /// 다른 주소로 간다(로그인 시작 등).
    pub fn navigate(url: &str) {
        if let Some(location) = location() {
            let _ = location.assign(url);
        }
    }

    /// 클립보드에 넣는다(안 되면 조용히 넘어간다 — 화면에 글자로도 보인다).
    pub fn copy_text(text: &str) {
        if let Some(window) = web_sys::window() {
            let _ = window.navigator().clipboard().write_text(text);
        }
    }
    /// 주소의 해시(`#/…`, 페이지 주소 — [`crate::route`]).
    pub fn current_hash() -> String {
        location().and_then(|l| l.hash().ok()).unwrap_or_default()
    }

    /// 해시가 바뀔 때마다(링크·뒤로 가기) 새 해시로 부른다. 앱이 살아 있는 동안 한 번만 건다.
    pub fn on_hash_change(mut f: impl FnMut(String) + 'static) {
        use wasm_bindgen::JsCast;
        use wasm_bindgen::closure::Closure;
        let Some(window) = web_sys::window() else {
            return;
        };
        let closure = Closure::<dyn FnMut()>::new(move || f(current_hash()));
        let _ =
            window.add_event_listener_with_callback("hashchange", closure.as_ref().unchecked_ref());
        closure.forget();
    }

    fn dialog(id: &str) -> Option<web_sys::HtmlDialogElement> {
        use wasm_bindgen::JsCast;
        web_sys::window()?
            .document()?
            .get_element_by_id(id)?
            .dyn_into::<web_sys::HtmlDialogElement>()
            .ok()
    }

    /// `<dialog id>`(서랍·확인 상자)를 `showModal()`로 연다 — Esc·포커스 가두기는 브라우저가 한다.
    pub fn show_modal(id: &str) {
        if let Some(d) = dialog(id)
            && !d.open()
        {
            let _ = d.show_modal();
        }
    }

    /// `<dialog id>`를 닫는다.
    pub fn close_modal(id: &str) {
        if let Some(d) = dialog(id) {
            d.close();
        }
    }

    /// 그 id의 요소가 보이게 스크롤한다.
    pub fn scroll_into_view(id: &str) {
        if let Some(el) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id(id))
        {
            el.scroll_into_view();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use super::Error;
    use serde::de::DeserializeOwned;

    fn web_only() -> Error {
        Error::Network("대시보드는 웹(wasm32)에서만 서버를 부른다".into())
    }

    pub async fn request<T: DeserializeOwned>(
        _: &str,
        _: &str,
        _: Option<serde_json::Value>,
    ) -> Result<T, Error> {
        Err(web_only())
    }

    pub async fn request_empty(_: &str, _: &str) -> Result<(), Error> {
        Err(web_only())
    }

    pub async fn send_no_content(_: &str, _: &str, _: serde_json::Value) -> Result<(), Error> {
        Err(web_only())
    }

    pub async fn sleep_ms(_: u32) {}

    pub fn current_path() -> String {
        "/".into()
    }

    pub fn current_query() -> String {
        String::new()
    }

    pub fn origin() -> String {
        String::new()
    }

    pub fn replace_path(_: &str) {}

    pub fn navigate(_: &str) {}

    pub fn copy_text(_: &str) {}

    pub fn current_hash() -> String {
        String::new()
    }

    pub fn on_hash_change(_: impl FnMut(String) + 'static) {}

    pub fn show_modal(_: &str) {}

    pub fn close_modal(_: &str) {}

    pub fn scroll_into_view(_: &str) {}
}
