//! HTTP API 요청·응답 타입. 서버와 클라이언트(앱·대시보드)가 같은 타입을 쓴다.
//!
//! 서버는 아래 경로를 `/`와 `/api` 두 곳에서 받는다(운영 프록시가 `/api/*`를 그대로 넘긴다).
//!
//! | 메서드·경로 | 누가 | 요청 | 응답 |
//! |---|---|---|---|
//! | `GET /health` | 누구나 | — | [`HealthResponse`] |
//! | `GET /.well-known/oauth-protected-resource/api` | 누구나 | — | RFC 9728 보호 리소스 메타데이터 |
//! | `GET /oauth/app-client.json` | 누구나 | — | 응시 앱 공개 클라이언트의 CIMD 문서 |
//! | `POST /mcp` | 사람(Bearer만) | JSON-RPC | MCP 출제 서버(#17, `crates/server/src/mcp.rs`). `GET`·`DELETE`는 `405` |
//! | `GET /auth/app-config` | 누구나 | — | [`AppAuthConfig`] |
//! | `GET /auth/login?next=/경로` | 누구나 | — | `302` auth `/authorize`(대시보드 로그인 시작) |
//! | `GET /auth/callback` | auth가 돌려보냄 | — | `302` 대시보드(세션 쿠키) |
//! | `POST /auth/logout` | 로그인 | — | `204` |
//! | `GET /me` | 로그인 | — | [`MeView`] |
//! | `POST /orgs` | 로그인 | [`CreateOrgRequest`] | `201` [`OrgView`](만든 사람이 관리자) |
//! | `GET /orgs/{id}` | 구성원 | — | [`OrgView`] |
//! | `GET /orgs/{id}/members` | 구성원 | — | `[`[`MemberView`]`]` |
//! | `PATCH /orgs/{id}/members/{sub}` | 구성원 관리 | [`UpdateMemberRequest`] | [`MemberView`] |
//! | `DELETE /orgs/{id}/members/{sub}` | 구성원 관리 | — | `204` |
//! | `POST /orgs/{id}/invites` | 구성원 관리 | [`CreateInviteRequest`] | `201` [`InviteCreated`] |
//! | `POST /invites/{token}/accept` | 로그인 | [`AcceptInviteRequest`](crate::terms::AcceptInviteRequest) | [`OrgView`]. 약관 판이 지금 것이 아니면 `422`(초대는 쓰이지 않는다, #86) |
//! | `GET /orgs/{id}/submissions` | 결과 보기 | — | `[`[`SubmissionView`]`]` |
//! | `GET /practice/problems` | 사람(#18) | — | `[`[`PublicCodingProblem`]`]`(연습 문제) |
//! | `GET /problems/{id}` | 사람 | — | [`PublicCodingProblem`](연습 문제만, 시험 문항은 `404`) |
//! | `POST /submissions` | 사람 | [`CreateSubmissionRequest`](언어 + 코드) | `202` [`SubmissionView`](연습 문제만. 코드가 [`MAX_CODE_BYTES`](crate::coding::MAX_CODE_BYTES)를 넘거나 비면 `422`. 조직 없이 내면 개인 연습 — 채점을 기다리는 연습이 [`MAX_PENDING`](crate::practice::MAX_PENDING)개면 `429`) |
//! | `GET /me/practice` | 사람(#18) | — | [`PracticeRecords`](crate::practice::PracticeRecords)(내 연습 기록·문항별 최고·최근·추이) |
//! | `GET /submissions/{id}` | 낸 사람·그 조직의 결과 보기 | — | [`SubmissionView`](시험 응시는 응시자에게 「응시 완료」만, 아래. 점수를 볼 수 있으면 `score` = 최종 점수·계산식) |
//! | `GET /orgs/{id}/problem-bank` | 시험 관리 | — | `[`[`PublicCodingProblem`]`]`(문제은행, 공개 보기) |
//! | `POST /orgs/{id}/exams` | 시험 관리 | [`CreateExamRequest`] | `201` [`ExamView`] |
//! | `GET /orgs/{id}/exams` | 구성원 | — | `[`[`ExamView`]`]` |
//! | `GET /orgs/{id}/exams/{exam}` | 구성원 | — | [`ExamView`] |
//! | `PATCH /orgs/{id}/exams/{exam}` | 시험 관리 | [`UpdateExamRequest`] | [`ExamView`](점수 공개는 사내 진단만 — 채용이면 422. 채점 설정(w·추가 기준표) 범위 밖 422) |
//! | `GET /orgs/{id}/exams/{exam}/results` | 결과 보기 | — | [`ExamResults`](응시 현황·점수·언어·통과 수. 문항마다 `score` = 최종 점수·계산식, 응시자마다 `final_score`) |
//! | `GET /orgs/{id}/exams/{exam}/results.csv` | 결과 보기 | — | `text/csv`(UTF-8 BOM, 수식 주입 방지. 최종 점수·언어·통과 수 열) |
//! | `GET /orgs/{id}/rounds` | 결과 보기 | — | `[`[`MemberRounds`]`]`(사내 진단, 같은 사람의 회차를 시간순) |
//! | `POST /orgs/{id}/exams/{exam}/invites` | 시험 관리 | [`CreateExamInvitesRequest`] | `201` [`ExamInvitesCreated`] |
//! | `GET /orgs/{id}/exams/{exam}/invites` | 시험 관리·결과 보기 | — | `[`[`ExamInviteView`]`]`(초대 현황) |
//! | `POST /exam-invites/{token}/accept` | 로그인 | — | [`AssignmentView`](응시 배정) |
//! | `GET /me/assignments` | 로그인 | — | `[`[`AssignmentView`]`]`(내게 배정된 시험) |
//! | `POST /assignments/{id}/items/{n}/start` | 배정받은 사람 | [`StartItemRequest`](앱 판·모델 sha256 신고, #12) | [`ItemStarted`](문제 공개 보기 + 마감). 허용 목록에 없으면 `403` |
//! | `POST /assignments/{id}/items/{n}/submit` | 배정받은 사람 | [`SubmitItemRequest`](언어 + 코드 + 앱의 로그 순번) | `202` [`SubmissionView`]. 과정 로그의 마지막 `Submitted` 사건(언어·코드 sha256)과 같아야 한다 — 로그가 덜 올라왔으면 `409`, 다르거나 코드가 비었거나 너무 길면 `422` |
//! | `POST /assignments/{id}/items/{n}/log/keys` | 배정받은 사람(시작한 문항) | [`RegisterLogKeyRequest`] | [`LogHead`](서버가 아는 체인 머리) |
//! | `POST /assignments/{id}/items/{n}/log` | 배정받은 사람(시작한 문항) | [`UploadLogRequest`] | [`LogHead`] |
//! | `GET /orgs/{id}/exams/{exam}/invites/{invite}/items/{n}/log` | 결과 보기 | — | [`ProcessLogView`](다시보기) |
//! | `GET /orgs/{id}/exams/{exam}/invites/{invite}/items/{n}/activity` | 결과 보기 | — | [`ItemActivity`](AI 대화·편집 제안·수락 뒤 수정·로컬 실행·최종 코드·평가, #99) |
//! | `GET /orgs/{id}/exams/{exam}/invites/{invite}/signals` | 결과 보기 | — | [`SuspicionView`](의심 신호 시간순, #12) |
//! | `GET /orgs/{id}/exams/{exam}/invites/{invite}/report` | 결과 보기 | — | [`ReportView`](crate::report::ReportView)(AI 리포트, #15) |
//! | `POST /orgs/{id}/exams/{exam}/invites/{invite}/report` | 결과 보기 | — | `202` [`ReportView`](crate::report::ReportView). 키 없음 `503`, 채점 덜 끝남·진행 중 `409`, 배정당 3회 넘음 `429` |
//! | `POST /orgs/{id}/exams/{exam}/invites/{invite}/erase` | 구성원 관리 | [`EraseRequest`](crate::erasure::EraseRequest)(사유 선택, 500자) | [`ErasureRecord`](crate::erasure::ErasureRecord)(감사 기록, #55). 진행 중·채점 큐도 지운다. 이미 지운 배정은 처음 기록 그대로(멱등), 없으면 `404`, 사유가 길면 `422` |
//! | `GET /orgs/{id}/erasures[?email=…]` | 구성원 관리 | — | [`ErasureLog`](crate::erasure::ErasureLog)(삭제 감사 로그 새것부터, 이메일은 해시로만 거른다, 3년 보관) |
//! | `GET /orgs/{id}/retention` | 구성원 | — | [`RetentionView`](crate::retention::RetentionView)(조직 기본 보관 기간·이력, #54) |
//! | `PUT /orgs/{id}/retention` | 구성원 관리 | [`SetRetentionRequest`](crate::retention::SetRetentionRequest)(`null`=기본 365일) | [`RetentionView`](crate::retention::RetentionView). 범위(30~1095일) 밖 `422` |
//! | `GET /orgs/{id}/exams/{exam}/retention` | 구성원 | — | [`RetentionView`](crate::retention::RetentionView)(시험별·조직 기본·실제 값) |
//! | `PUT /orgs/{id}/exams/{exam}/retention` | 시험 관리 | [`SetRetentionRequest`](crate::retention::SetRetentionRequest)(`null`=조직 기본) | [`RetentionView`](crate::retention::RetentionView) |
//! | `POST /auth/webhook` | sandcorp-auth(HMAC 서명) | `user.deleted` 이벤트 | `204`(그 `sub`의 개인 연습 기록·세션 삭제). 서명 틀림 `401`, 형식·시각 `400`, 비밀 없음 `503` |
//! | `GET /orgs/{id}/usage` | 구성원 관리 | — | [`OrgUsage`](crate::report::OrgUsage)(응시 수·종류별 토큰·추정 비용, #15) |
//! | `POST /assignments/{id}/consent` | 배정받은 사람 | [`ConsentRequest`](crate::webcam::ConsentRequest)(안내문 판) | [`WebcamTerms`](crate::webcam::WebcamTerms). 판이 다르면 `409` |
//! | `PUT /assignments/{id}/webcam/baseline` | 배정받은 사람(동의 뒤, 첫 문항 시작 전) | `image/jpeg` 한 장(64KB·320×320 이하) | [`WebcamTerms`](crate::webcam::WebcamTerms) |
//! | `POST /assignments/{id}/items/{n}/snapshots?reason=…&ref=…&at_ms=…` | 배정받은 사람(시작한 문항) | `image/jpeg` 한 장 | `201` [`SnapshotStored`](crate::webcam::SnapshotStored)(같은 `ref`는 `200`) |
//! | `GET /orgs/{id}/exams/{exam}/invites/{invite}/webcam` | 감독 | — | [`WebcamView`](crate::webcam::WebcamView)(기준 사진·스냅샷 시간순) |
//! | `GET /orgs/{id}/exams/{exam}/invites/{invite}/webcam/photos/{photo}` | 감독 | — | `image/jpeg`(`Cache-Control: private, no-store`) |
//!
//! 웹캠(#13, 규칙은 [`crate::webcam`]): 사람 응시자는 지금 판 안내문에 동의하고 기준 사진을 올려야 문항 `start`가
//! 된다(아니면 `403`). 스모크 기계 주체는 면제. 영상(전체 녹화)을 받는 경로는 없다.
//!
//! 과정 로그(#9, 형식은 [`crate::log`]): 앱이 문항을 시작하면 설치 키의 공개키를 등록하고, 서명한 사건을 순번대로
//! 올린다. 이미 받은 순번을 같은 내용으로 다시 올리면 건너뛴다(멱등) — 응답이 끊겨도 다시 올리면 된다. 빠진 순번·
//! 맞지 않는 서명·체인은 업로드 전체를 거부하고(`409`·`422`, 저장하지 않음) 거부 횟수를 다시보기 신호로 남긴다.
//!
//! 시험 응시(배정)의 제출을 응시자 본인이 볼 때는 [`crate::exam::candidate_sees_scores`]가 아니면
//! 상태가 [`SubmissionState::Completed`](「응시 완료」)로만 나간다 — 점수·지표·실패 이유는 고객사만 본다.
//! 제한은 서버가 강제한다: 시작(`start`) 뒤 제한 시간 + [`crate::exam::SUBMIT_GRACE_SECS`]가 지난 제출과
//! 문항마다 제출은 한 번이다.
//!
//! 「로그인」은 sandcorp-auth 액세스 토큰(`Authorization: Bearer`, 응시 앱) 또는 대시보드 세션 쿠키다.
//! 권한은 [`crate::account::Role`] 표를 따른다. 구성원이 아닌 조직·볼 수 없는 제출은 `404`
//! (있는지 알리지 않는다), 구성원인데 역할이 모자라면 `403`. 오류 본문은 [`ErrorResponse`].
//!
//! 운영자 화면(#19, `/operator/*` — 대표 계정만)의 경로와 타입은 [`crate::operator`]. 승인 전 조직(`OrgView::approved`
//! 가 `false`)은 `POST /orgs/{id}/exams`·문항 `start`가 `403`이다. 내린 문항은 시험 만들기 `422`, 연습 경로 `404`.
//!
//! 조직 문제 편집·발행 전 점검(#16, `/orgs/{id}/problems`·`/orgs/{id}/problem-previews`)의 경로와 타입은
//! [`crate::authoring`] — 그 응답만 자기 조직 시험 관리 역할에게 히든 테스트·모범 답안을 싣는다.
//!
//! 여기 실리는 것에는 히든 테스트·모범 답안·케이스별 실행 결과가 없다 — 문제는 공개 보기만, 채점은
//! [`GradeReport`](언어·통과 수)만 싣는다.

use serde::{Deserialize, Serialize};

use crate::account::Role;
use crate::coding::{Language, PublicCodingProblem};
use crate::exam::{Delivery, ExamItem, ExamSettings, InviteStatus, ItemLimits, Purpose};
use crate::grading::GradeReport;
use crate::integrity::{ClientIntegrity, Suspicion};
use crate::log::{Chain, LogEvent, LogSignal, SignedEvent};

/// `GET /health` 응답.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
}

/// 오류 응답 본문.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
}

/// `POST /submissions` 요청(개인 연습 — 과정 로그 없이 받는다).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateSubmissionRequest {
    pub problem_id: String,
    pub language: Language,
    /// 제출 코드(1~[`MAX_CODE_BYTES`](crate::coding::MAX_CODE_BYTES)바이트).
    pub code: String,
    /// 응시 시작부터 제출까지(초, 앱이 잰 값 — 연습은 감독이 아니다).
    #[serde(default)]
    pub elapsed_secs: u32,
    /// 이 조직의 응시로 낸다(구성원이어야 한다). 없으면 개인 제출 — 낸 사람만 본다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
}

/// 제출 하나의 지금 상태. `GET /submissions/{id}`와 `POST /submissions`의 응답.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubmissionView {
    pub id: String,
    pub problem_id: String,
    /// 이 제출이 속한 조직(개인 제출이면 없음).
    #[serde(default)]
    pub org_id: Option<String>,
    pub state: SubmissionState,
    /// 최종 점수·계산식. 채점이 끝났고 점수를 볼 수 있을 때만 — 채용 시험 응시자 본인
    /// (「응시 완료」)에게는 없다.
    #[serde(default)]
    pub score: Option<crate::evaluation::ItemScore>,
}

/// 채점 큐 위의 제출 상태.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SubmissionState {
    /// 큐에서 채점을 기다린다.
    Queued,
    /// 채점 워커가 채점 중이다.
    Grading,
    /// 채점이 끝났다. 점수는 [`GradeReport::pass_rate`](통과 수만 — 케이스별 결과는 없다).
    Done { report: GradeReport },
    /// 채점하지 못했다(판정기 쪽 문제 — 컴파일 오류는 실패가 아니라 0점). `reason`은 응시자에게 보여도 되는 말이다.
    Failed { reason: String },
    /// 응시 완료. 시험 응시를 응시자 본인이 볼 때 점수 대신 이것만 나간다(채용 시험 등 — 결과는 고객사만).
    Completed,
}

impl SubmissionState {
    /// 더 바뀌지 않는 상태인가(채점 끝·실패, 또는 응시자에게 보이는 「응시 완료」).
    pub fn is_final(&self) -> bool {
        matches!(
            self,
            Self::Done { .. } | Self::Failed { .. } | Self::Completed
        )
    }
}

/// 응시 앱이 로그인에 쓰는 값. 서버가 알려 준다(앱에 auth 주소를 따로 넣지 않는다).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppAuthConfig {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    /// 공개 클라이언트 id(CIMD 문서 URL).
    pub client_id: String,
    /// 인가 요청의 `resource`(토큰 `aud`).
    pub resource: String,
    pub scope: String,
}

/// 로그인한 사람과 그가 속한 조직들. `GET /me`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeView {
    /// sandcorp-auth 계정 키(`m_…`). 바뀌지 않는다.
    pub sub: String,
    pub name: Option<String>,
    pub orgs: Vec<OrgView>,
    /// 운영자(대표 계정)인가(#19). 대시보드가 운영자 메뉴를 보일지 정한다 — 권한은 서버가 강제한다.
    #[serde(default)]
    pub is_owner: bool,
}

/// 조직과 그 안에서 내 역할.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct OrgView {
    pub id: String,
    pub name: String,
    pub my_role: Role,
    /// 운영자가 승인한 조직인가(#19). 승인 전에는 시험을 만들거나 문항을 시작할 수 없다.
    #[serde(default)]
    pub approved: bool,
    /// 내가 이 조직에 대해 이용약관에 동의한 기록(#86). 이전에 만든 조직·들어온 사람은 없다.
    #[serde(default)]
    pub terms: Option<crate::terms::TermsAcceptance>,
}

/// `POST /orgs`. 이용약관에 동의해야 한다(#86) — `terms_version`이 지금 판(`terms::TERMS_VERSION`)이 아니면 `422`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateOrgRequest {
    pub name: String,
    pub terms_version: String,
}

/// 조직의 구성원 한 명.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberView {
    pub sub: String,
    pub name: Option<String>,
    pub role: Role,
}

/// `PATCH /orgs/{id}/members/{sub}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateMemberRequest {
    pub role: Role,
}

/// `POST /orgs/{id}/invites` — 동료 초대 링크를 만든다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateInviteRequest {
    pub role: Role,
}

/// 만든 초대. `token`은 이때 한 번만 보인다(서버에는 해시만 남는다). 한 번 쓰면 끝나고 7일 뒤 만료된다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteCreated {
    pub token: String,
    pub role: Role,
    /// 만료 시각(유닉스 초).
    pub expires_at: i64,
}

/// `POST /orgs/{id}/exams` — 문제은행에서 고른 문항으로 시험을 만든다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateExamRequest {
    pub title: String,
    pub purpose: Purpose,
    /// 사내 진단에서 응시자에게 점수를 공개하는가(채용이면 무시되고 언제나 비공개).
    #[serde(default)]
    pub reveal_scores: bool,
    pub items: Vec<ExamItem>,
    /// 채점 설정. 없으면 기본값(w 50·추가 기준표 없음). 범위 밖이면 `422`.
    #[serde(default)]
    pub settings: ExamSettings,
}

/// 시험 하나.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExamView {
    pub id: String,
    pub org_id: String,
    pub title: String,
    pub purpose: Purpose,
    pub reveal_scores: bool,
    /// 문항(순서대로, 번호는 0부터).
    pub items: Vec<ExamItem>,
    /// 만든 시각(유닉스 초).
    pub created_at: i64,
    /// 채점 설정.
    #[serde(default)]
    pub settings: ExamSettings,
}

/// `POST /orgs/{id}/exams/{exam}/invites` — 응시자를 이메일로 초대한다(일괄).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateExamInvitesRequest {
    pub emails: Vec<String>,
}

/// 초대 결과. 틀린 주소는 `rejected`에 그대로 돌려준다(나머지는 만든다).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExamInvitesCreated {
    pub invites: Vec<ExamInviteCreated>,
    pub rejected: Vec<String>,
}

/// 초대 하나를 만든(또는 다시 낸) 결과.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExamInviteCreated {
    pub id: String,
    pub email: String,
    /// 초대 링크의 토큰. 이때 한 번만 보인다(서버에는 해시만). 이미 배정이 끝난 사람이면 `None`(링크를 다시 내지 않는다).
    /// 링크는 `{공개 주소}/exam-invite/{token}`.
    pub token: Option<String>,
    /// 이미 같은 시험에 배정이 끝난 사람이다.
    pub already_assigned: bool,
    pub delivery: Delivery,
    /// 만료 시각(유닉스 초).
    pub expires_at: i64,
}

/// 초대 현황 한 줄.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExamInviteView {
    pub id: String,
    pub email: String,
    pub status: InviteStatus,
    /// 링크로 들어와 배정을 받았는가.
    pub accepted: bool,
    /// 배정받은 사람의 표시 이름(있으면).
    pub candidate_name: Option<String>,
    pub delivery: Delivery,
    pub expires_at: i64,
}

/// 응시자에게 배정된 시험 하나(초대 하나).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssignmentView {
    /// 배정(초대) id. 응시 경로의 `{id}`.
    pub id: String,
    pub exam_id: String,
    pub exam_title: String,
    pub org_name: String,
    pub purpose: Purpose,
    /// 응시자 본인에게 점수가 보이는 시험인가.
    pub scores_visible: bool,
    pub items: Vec<AssignmentItem>,
    /// 이 시험이 허용하는 언어(#96 — 응시자는 이 중에서 고른다).
    pub allowed_languages: Vec<Language>,
    /// 웹캠 동의·기준 사진 조건(#13, `GET /me/assignments`만 채운다).
    #[serde(default)]
    pub webcam: Option<crate::webcam::WebcamTerms>,
}

/// 배정된 시험의 문항 하나. 문제 내용은 시작(`start`)할 때 받는다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssignmentItem {
    /// 문항 번호(0부터).
    pub position: u32,
    pub limits: ItemLimits,
    pub state: ItemState,
}

/// 문항 응시 상태.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ItemState {
    NotStarted,
    /// 시작했다. `deadline`(유닉스 초)이 지나면 제출을 받지 않는다(여유 [`crate::exam::SUBMIT_GRACE_SECS`]).
    InProgress {
        deadline: i64,
    },
    /// 냈다. 결과는 `GET /submissions/{submission_id}`.
    Submitted {
        submission_id: String,
    },
}

/// `POST /assignments/{id}/items/{n}/start` 응답. 다시 불러도 처음 시작한 시각을 그대로 준다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemStarted {
    /// 문제(공개 보기 — 설명·서명·예제·케이스 시간·허용 언어의 틀만).
    pub problem: PublicCodingProblem,
    /// 이 시험이 허용하는 언어(#96). 제출 언어가 이 밖이면 서버가 `422`.
    pub allowed_languages: Vec<Language>,
    /// 이 문항의 제한 시간.
    pub limits: ItemLimits,
    pub started_at: i64,
    pub deadline: i64,
    /// 서버의 지금 시각(앱이 남은 시간을 서버 기준으로 잰다).
    pub now: i64,
}

/// `POST /assignments/{id}/items/{n}/submit` — 코드를 낸다. 서버는 과정 로그(`log_seq` 이하)의 마지막
/// [`LogEvent::Submitted`]의 언어·코드 sha256과 맞춰 본다(로그가 정본). 소요 시간은 서버가 시작 시각으로 잰다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitItemRequest {
    pub language: Language,
    /// 제출 코드(1~[`MAX_CODE_BYTES`](crate::coding::MAX_CODE_BYTES)바이트).
    pub code: String,
    /// 앱이 이 문항에 쓴 사건 수(다음 순번). 서버가 받은 로그가 이보다 짧으면 `409` — 앱은 로그를 비우고 다시 낸다.
    pub log_seq: u64,
}

/// `POST /assignments/{id}/items/{n}/log/keys` — 이 설치의 공개키 등록(이미 있으면 그대로).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterLogKeyRequest {
    /// Ed25519 공개키(base64url).
    pub public_key: String,
}

/// 서버가 아는 체인 머리. 앱은 이 순번 앞의 사건을 지워도 된다.
pub type LogHead = Chain;

/// `POST /assignments/{id}/items/{n}/log` — 서명된 사건을 순번대로(최대 [`crate::log::MAX_EVENTS_PER_UPLOAD`]개).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadLogRequest {
    pub events: Vec<SignedEvent>,
}

/// 다시보기의 사건 하나(서명·체인을 검증해 받은 것).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayEvent {
    pub seq: u64,
    /// 앱 시계의 유닉스 밀리초.
    pub at_ms: i64,
    /// 서버가 받은 시각(유닉스 초). 오프라인으로 쌓였다 올라온 사건은 `at_ms`보다 한참 늦다.
    pub received_at: i64,
    /// 몇 번째로 등록된 키로 서명했는가(0부터). 바뀌면 다른 설치다.
    pub key_index: u32,
    pub event: LogEvent,
}

/// `GET /orgs/{id}/exams/{exam}/invites/{invite}/items/{n}/activity` — 결과 보기(#99). 응시 문항 하나의 코딩·AI 활용 기록:
/// AI 대화(턴)·편집 제안과 결정·수락 뒤 수정·로컬 실행([`crate::activity::Activity`], 검증된 과정 로그에서 세운다), 최종 코드
/// (제출 코드), 최종 점수와 AI 활용 평가(항목·근거 = 턴/제안 번호). 코드 스냅숏 원문은 `…/log`에 있다. 히든 테스트는 없다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemActivity {
    pub position: u32,
    pub submission_id: Option<String>,
    /// 제출 언어·코드(냈을 때).
    pub language: Option<Language>,
    pub final_code: Option<String>,
    pub activity: crate::activity::Activity,
    /// 최종 점수·계산식·평가(냈을 때). 평가 근거는 `activity`의 턴·제안 번호를 가리킨다.
    pub score: Option<crate::evaluation::ItemScore>,
    /// 문항 시작 때 신고한 AI 도우미 모델 이름(#131, 시작했을 때).
    #[serde(default)]
    pub model: Option<String>,
}

/// `GET /orgs/{id}/exams/{exam}/invites/{invite}/items/{n}/log` — 응시 문항 하나의 과정 다시보기.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessLogView {
    /// 응시자 표시 이름(있으면)과 이메일.
    pub candidate: String,
    pub position: u32,
    /// 서버가 남긴 문항 시작 시각(유닉스 초). 시작하지 않았으면 없다.
    pub started_at: Option<i64>,
    /// 제출했으면 그 제출 id.
    pub submission_id: Option<String>,
    /// 시간순(순번순) 사건.
    pub events: Vec<ReplayEvent>,
    /// 살펴볼 신호(판단은 고객사).
    pub signals: Vec<LogSignal>,
}

/// `PATCH /orgs/{id}/exams/{exam}` — 사내 진단의 응시자 점수 공개(채용에 주면 422)와 채점 설정(범위 밖 422).
/// 준 칸만 바꾼다.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UpdateExamRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reveal_scores: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<ExamSettings>,
}

/// 결과 한 칸(응시자 × 문항)의 상태.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultItemStatus {
    NotStarted,
    InProgress,
    /// 냈고 채점을 기다린다(큐·채점 중).
    Submitted,
    /// 채점이 끝났다.
    Done,
    /// 채점하지 못했다(점수 0으로 센다).
    Failed,
}

impl ResultItemStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ResultItemStatus::NotStarted => "not_started",
            ResultItemStatus::InProgress => "in_progress",
            ResultItemStatus::Submitted => "submitted",
            ResultItemStatus::Done => "done",
            ResultItemStatus::Failed => "failed",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ResultItemStatus::NotStarted => "시작 전",
            ResultItemStatus::InProgress => "진행 중",
            ResultItemStatus::Submitted => "채점 대기",
            ResultItemStatus::Done => "채점 완료",
            ResultItemStatus::Failed => "채점 실패",
        }
    }
}

/// 응시자 한 사람의 문항 하나 결과. 히든 테스트·코드·실패 상세는 없다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultItem {
    /// 문항 번호(0부터).
    pub position: u32,
    pub problem_id: String,
    pub status: ResultItemStatus,
    /// 냈으면: 제출 언어.
    pub language: Option<Language>,
    /// 주 점수: 히든 케이스 통과율(0.0~1.0). 채점이 끝난 문항만.
    pub pass_rate: Option<f64>,
    pub passed: Option<u32>,
    pub total: Option<u32>,
    /// 냈으면: 서버가 잰 소요 시간(초).
    pub elapsed_secs: Option<u32>,
    /// 냈으면: 최종 점수·계산식.
    #[serde(default)]
    pub score: Option<crate::evaluation::ItemScore>,
    /// 시작했으면: 문항 시작 때 앱이 신고한 AI 도우미 모델 이름(#131 — [`crate::inference::model_name`]). 판단은 고객사.
    #[serde(default)]
    pub model: Option<String>,
}

/// 응시자 한 사람의 시험 결과.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultRow {
    pub invite_id: String,
    pub email: String,
    pub candidate_name: Option<String>,
    pub status: InviteStatus,
    pub items: Vec<ResultItem>,
    /// 히든 케이스 점수 = 문항 통과율의 평균(0.0~1.0, 실패 문항은 0). 모든 문항의 채점이 끝났을 때만.
    pub score: Option<f64>,
    /// 시험 점수 = 문항 최종 점수의 평균(0~100). 모든 문항의 최종 점수가 나왔을 때만.
    #[serde(default)]
    pub final_score: Option<f64>,
    /// AI 활용 평가 평균(0~100, #99) — 평가가 끝난 문항들의 평균. 평가된 문항이 없으면 없다.
    #[serde(default)]
    pub evaluation_score: Option<f64>,
    /// 이 응시가 자동 삭제되는 시각(유닉스 초, #54: 응시 종료 + 보관 기간)과 남은 날(올림).
    #[serde(default)]
    pub delete_at: Option<i64>,
    #[serde(default)]
    pub delete_in_days: Option<u32>,
}

/// `GET /orgs/{id}/exams/{exam}/results`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExamResults {
    pub exam_id: String,
    pub title: String,
    pub purpose: Purpose,
    pub reveal_scores: bool,
    pub rows: Vec<ResultRow>,
    /// 이 시험에 쓰는 보관 기간(일, #54 — 시험별 → 조직 기본 → 365).
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
}

fn default_retention_days() -> u32 {
    crate::retention::DEFAULT_DAYS
}

/// 회차 하나(사내 진단 시험 하나에서의 결과).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Round {
    pub exam_id: String,
    pub exam_title: String,
    /// 시험을 배정받은 시각(유닉스 초) — 회차의 시간순 기준.
    pub assigned_at: i64,
    pub status: InviteStatus,
    /// 모든 문항의 채점이 끝났을 때만.
    pub score: Option<f64>,
}

/// 같은 사람(`sub`)의 사내 진단 회차들(시간순).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemberRounds {
    pub sub: String,
    pub name: Option<String>,
    pub rounds: Vec<Round>,
}

/// `POST /assignments/{id}/items/{n}/start`의 요청: 앱이 신고하는 앱 판·모델 sha256([`crate::integrity`]).
pub type StartItemRequest = ClientIntegrity;

/// `GET /orgs/{id}/exams/{exam}/invites/{invite}/signals` — 응시자 한 명의 의심 신호(#12). 판단은 고객사가 한다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuspicionView {
    /// 응시자(이름 <이메일>).
    pub candidate: String,
    /// 시간순(시각을 모르는 것은 끝에). 앱 시계와 서버 시계가 섞인다 — 각 신호의 `clock`.
    pub signals: Vec<Suspicion>,
}
