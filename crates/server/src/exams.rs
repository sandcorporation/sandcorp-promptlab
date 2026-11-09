//! 시험·응시 배정 초대·응시(#8). 규칙은 [`promptlab_shared::exam`], 저장은 [`promptlab_store::exams`].
//!
//! - 출제자·관리자(시험 관리)가 문제은행에서 문항을 골라 시험을 만들고 응시자를 이메일로 초대한다.
//! - 초대 링크(`/exam-invite/{token}`, 대시보드 화면)로 카카오 로그인한 사람이 `accept`하면 그 시험이 배정된다.
//!   조직에 들어가지는 않는다(#7 동료 초대와 다르다).
//! - 응시 앱은 배정 목록을 받고 문항을 `start`(서버가 시작 시각을 남긴다)한 뒤 `submit`한다. 제한 시간·한 번만 제출은
//!   여기서 강제한다. `start`는 앱이 신고한 앱 판·모델 sha256을 허용 목록([`promptlab_shared::integrity`])과
//!   대조해 없으면 `403`(시작하지 않고 「모델 무결성 실패」 신호로 남긴다, #12).
//! - 응시자 본인에게 나가는 제출 상태는 [`for_candidate`]를 거친다(채용이면 「응시 완료」만).

use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use promptlab_shared::account::Permission;
use promptlab_shared::api::{
    AssignmentItem, AssignmentView, CreateExamInvitesRequest, CreateExamRequest, ExamInviteCreated,
    ExamInviteView, ExamInvitesCreated, ExamView, ItemStarted, ItemState, StartItemRequest,
    SubmissionState, SubmissionView, SubmitItemRequest,
};
use promptlab_shared::exam::{
    self, Delivery, EXAM_INVITE_SECS, MAX_INVITES_PER_REQUEST, MAX_ITEMS, MAX_TITLE_CHARS, Purpose,
    SUBMIT_GRACE_SECS,
};

use promptlab_shared::coding::PublicCodingProblem;
use promptlab_shared::log::{LogEvent, code_sha256};
use promptlab_store::exams::{self, Assignment, Exam, InviteIssue, NewExam};
use promptlab_store::{accounts, bank, problems, smoke, submissions};
use uuid::Uuid;

use crate::auth::token::AccountKind;
use crate::auth::{Account, random_secret, secret_hash};
use crate::mail::{self, MailError};
use crate::orgs::{membership, require};
use crate::{ApiError, AppState};

fn exam_not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "그런 시험이 없다")
}

fn assignment_not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "그런 배정이 없다")
}

fn unprocessable(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, message)
}

fn conflict(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::CONFLICT, message)
}

pub(crate) fn exam_view(exam: Exam) -> ExamView {
    ExamView {
        id: exam.id.to_string(),
        org_id: exam.org_id.to_string(),
        title: exam.title,
        purpose: exam.purpose,
        reveal_scores: exam.reveal_scores,
        items: exam.items,
        created_at: exam.created_at,
        settings: exam.settings,
    }
}

/// 시험의 채점 설정 검사: 범위 밖이면 422.
pub(crate) fn check_settings(
    settings: &promptlab_shared::exam::ExamSettings,
) -> Result<(), ApiError> {
    settings.validate().map_err(unprocessable)
}

/// 응시자 본인에게 보낼 제출 상태. 점수를 공개하지 않는 시험이면 「응시 완료」만 남긴다.
pub fn for_candidate(
    mut view: SubmissionView,
    purpose: Purpose,
    reveal_scores: bool,
) -> SubmissionView {
    if !exam::candidate_sees_scores(purpose, reveal_scores) {
        view.state = SubmissionState::Completed;
        view.score = None;
    }
    view
}

/// `GET /orgs/{id}/problem-bank` — 시험에 넣을 수 있는 문제(공개 보기): 내장 문제은행 + 이 조직의 문제(#16).
pub async fn problem_bank(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path(org_id): Path<String>,
) -> Result<Json<Vec<PublicCodingProblem>>, ApiError> {
    let org = require(&state, &org_id, &account, Permission::ManageExams).await?;
    Ok(Json(problems::bank(&state.pool, org.org_id).await?))
}

pub async fn create_exam(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path(org_id): Path<String>,
    Json(request): Json<CreateExamRequest>,
) -> Result<(StatusCode, Json<ExamView>), ApiError> {
    let org = require(&state, &org_id, &account, Permission::ManageExams).await?;
    // 시범 조직 승인(#19): 승인 전에는 시험을 만들지 않는다.
    crate::operator::require_approved(&state, org.org_id).await?;
    let title = request.title.trim();
    if title.is_empty() || title.chars().count() > MAX_TITLE_CHARS {
        return Err(unprocessable(format!(
            "시험 제목은 1~{MAX_TITLE_CHARS}자다"
        )));
    }
    if request.items.is_empty() || request.items.len() > MAX_ITEMS {
        return Err(unprocessable(format!("문항은 1~{MAX_ITEMS}개다")));
    }
    for (n, item) in request.items.iter().enumerate() {
        // 연습 문제(#18)는 시험에 넣지 않는다 — 연습에서 히든 테스트에 마음껏 제출해 볼 수 있으므로.
        // 내린 문항(#19)도 새 시험에는 넣지 않는다(이미 만든 시험은 그대로 쓴다).
        if !bank::usable_in_new_exam(&state.pool, org.org_id, &item.problem_id).await? {
            return Err(unprocessable(format!(
                "{}번 문항: 문제은행·이 조직의 문제에 없거나 내린 문제다",
                n + 1
            )));
        }
        exam::check_limits(&item.limits)
            .map_err(|why| unprocessable(format!("{}번 문항: {why}", n + 1)))?;
    }
    check_settings(&request.settings)?;
    // 채용 시험은 공개 설정을 받지 않는다(언제나 「응시 완료」만).
    let reveal_scores = request.purpose == Purpose::Internal && request.reveal_scores;
    accounts::upsert_user(&state.pool, &account.sub, None).await?;
    let now = state.clock.now();
    let id = exams::create_exam(
        &state.pool,
        &NewExam {
            org_id: org.org_id,
            created_by: &account.sub,
            title,
            purpose: request.purpose,
            reveal_scores,
            items: &request.items,
            settings: &request.settings,
        },
        now,
    )
    .await?;
    tracing::info!(org_id = %org.org_id, exam_id = %id, items = request.items.len(), by = %account.sub, "시험 만듦");
    Ok((
        StatusCode::CREATED,
        Json(ExamView {
            id: id.to_string(),
            org_id: org.org_id.to_string(),
            title: title.to_owned(),
            purpose: request.purpose,
            reveal_scores,
            items: request.items,
            created_at: now,
            settings: request.settings,
        }),
    ))
}

pub async fn list_exams(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path(org_id): Path<String>,
) -> Result<Json<Vec<ExamView>>, ApiError> {
    let org = membership(&state, &org_id, &account).await?;
    let exams = exams::list_exams(&state.pool, org.org_id).await?;
    Ok(Json(exams.into_iter().map(exam_view).collect()))
}

pub(crate) async fn find_exam(
    state: &AppState,
    org_id: Uuid,
    exam_id: &str,
) -> Result<Exam, ApiError> {
    let exam_id: Uuid = exam_id.parse().map_err(|_| exam_not_found())?;
    exams::get_exam(&state.pool, org_id, exam_id)
        .await?
        .ok_or_else(exam_not_found)
}

pub async fn get_exam(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((org_id, exam_id)): Path<(String, String)>,
) -> Result<Json<ExamView>, ApiError> {
    let org = membership(&state, &org_id, &account).await?;
    Ok(Json(exam_view(
        find_exam(&state, org.org_id, &exam_id).await?,
    )))
}

pub async fn create_invites(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((org_id, exam_id)): Path<(String, String)>,
    Json(request): Json<CreateExamInvitesRequest>,
) -> Result<(StatusCode, Json<ExamInvitesCreated>), ApiError> {
    let org = require(&state, &org_id, &account, Permission::ManageExams).await?;
    let exam = find_exam(&state, org.org_id, &exam_id).await?;
    let (emails, rejected) = exam::split_emails(&request.emails.join("\n"));
    if emails.is_empty() {
        return Err(unprocessable("올바른 이메일 주소가 없다"));
    }
    if emails.len() > MAX_INVITES_PER_REQUEST {
        return Err(unprocessable(format!(
            "한 번에 {MAX_INVITES_PER_REQUEST}명까지 초대한다"
        )));
    }
    accounts::upsert_user(&state.pool, &account.sub, None).await?;
    let now = state.clock.now();
    let mut invites = Vec::with_capacity(emails.len());
    for email in emails {
        let token = random_secret();
        let issued = exams::issue_invite(
            &state.pool,
            exam.id,
            &email,
            &secret_hash(&token),
            &account.sub,
            now,
            EXAM_INVITE_SECS,
        )
        .await?;
        let created = match issued {
            InviteIssue::Issued { id, expires_at } => {
                let link = format!("{}/exam-invite/{token}", state.auth.public_url);
                let message = mail::exam_invite(
                    &email,
                    &org.name,
                    &exam.title,
                    &link,
                    EXAM_INVITE_SECS / 86_400,
                );
                // 발송 실패는 초대를 막지 않는다(링크 복사로 전한다). 오류 문구에 비밀은 없지만 받는 주소는 남기지 않는다.
                let delivery = match state.mailer.send(&message).await {
                    Ok(()) => Delivery::Sent,
                    Err(MailError::NotConfigured) => Delivery::NotConfigured,
                    Err(MailError::Failed(why)) => {
                        tracing::warn!(invite_id = %id, %why, "초대 메일을 보내지 못했다");
                        Delivery::Failed
                    }
                };
                exams::set_delivery(&state.pool, id, delivery).await?;
                ExamInviteCreated {
                    id: id.to_string(),
                    email,
                    token: Some(token),
                    already_assigned: false,
                    delivery,
                    expires_at,
                }
            }
            InviteIssue::AlreadyAssigned {
                id,
                expires_at,
                delivery,
            } => ExamInviteCreated {
                id: id.to_string(),
                email,
                token: None,
                already_assigned: true,
                delivery,
                expires_at,
            },
        };
        invites.push(created);
    }
    tracing::info!(exam_id = %exam.id, count = invites.len(), by = %account.sub, "응시 초대 냄");
    Ok((
        StatusCode::CREATED,
        Json(ExamInvitesCreated { invites, rejected }),
    ))
}

pub async fn list_invites(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((org_id, exam_id)): Path<(String, String)>,
) -> Result<Json<Vec<ExamInviteView>>, ApiError> {
    let org = membership(&state, &org_id, &account).await?;
    if !(org.role.allows(Permission::ManageExams) || org.role.allows(Permission::ViewResults)) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "이 역할로는 할 수 없다",
        ));
    }
    let exam = find_exam(&state, org.org_id, &exam_id).await?;
    let rows = exams::list_invites(&state.pool, exam.id).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| ExamInviteView {
                id: r.id.to_string(),
                email: r.email,
                status: r.status,
                accepted: r.accepted,
                candidate_name: r.candidate_name,
                delivery: r.delivery,
                expires_at: r.expires_at,
            })
            .collect(),
    ))
}

fn assignment_view(a: Assignment) -> AssignmentView {
    let items = a
        .exam
        .items
        .iter()
        .enumerate()
        .map(|(n, item)| {
            let position = u32::try_from(n).unwrap_or(u32::MAX);
            let state = if let Some(id) = a.submitted.get(&position) {
                ItemState::Submitted {
                    submission_id: id.to_string(),
                }
            } else if let Some(started) = a.started.get(&position) {
                ItemState::InProgress {
                    deadline: started + i64::from(item.limits.time_limit_secs),
                }
            } else {
                ItemState::NotStarted
            };
            AssignmentItem {
                position,
                limits: item.limits,
                state,
            }
        })
        .collect();
    AssignmentView {
        id: a.id.to_string(),
        exam_id: a.exam.id.to_string(),
        exam_title: a.exam.title,
        org_name: a.org_name,
        purpose: a.exam.purpose,
        scores_visible: exam::candidate_sees_scores(a.exam.purpose, a.exam.reveal_scores),
        items,
        allowed_languages: a.exam.settings.allowed_languages,
        webcam: None,
    }
}

/// `POST /exam-invites/{token}/accept` — 링크로 들어온 사람에게 시험을 배정한다.
pub async fn accept_invite(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path(token): Path<String>,
) -> Result<Json<AssignmentView>, ApiError> {
    accounts::upsert_user(&state.pool, &account.sub, None).await?;
    let invalid = || {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "초대가 없거나 만료됐거나 다른 사람이 받았다",
        )
    };
    let id = exams::accept_invite(
        &state.pool,
        &secret_hash(&token),
        &account.sub,
        state.clock.now(),
    )
    .await?
    .ok_or_else(invalid)?;
    let assignment = exams::assignments_of(&state.pool, &account.sub, Some(id))
        .await?
        .pop()
        .ok_or_else(invalid)?;
    tracing::info!(invite_id = %id, sub = %account.sub, "응시 배정 받음");
    let views =
        crate::webcam::with_terms(&state, &account, vec![assignment_view(assignment)]).await?;
    Ok(Json(views.into_iter().next().ok_or_else(invalid)?))
}

/// `GET /me/assignments` — 내 배정들. 기계 주체(스모크)는 스모크 배정을 그때 준비한다.
pub async fn my_assignments(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
) -> Result<Json<Vec<AssignmentView>>, ApiError> {
    if account.kind == AccountKind::Machine {
        // 스모크(#28): 스모크 조직의 새 배정을 준비하고, 아직 시작하지 않은 배정만 보인다(지난 실행 것은 뺀다).
        smoke::prepare(&state.pool, &account.sub, state.clock.now()).await?;
        let list = exams::assignments_of(&state.pool, &account.sub, None).await?;
        let views = list
            .into_iter()
            .filter(|a| a.started.is_empty() && a.submitted.is_empty())
            .map(assignment_view)
            .collect();
        return Ok(Json(
            crate::webcam::with_terms(&state, &account, views).await?,
        ));
    }
    let list = exams::assignments_of(&state.pool, &account.sub, None).await?;
    let views = list.into_iter().map(assignment_view).collect();
    Ok(Json(
        crate::webcam::with_terms(&state, &account, views).await?,
    ))
}

/// 그 사람의 배정과 문항 번호. 남의 배정·없는 문항은 404.
pub(crate) async fn my_item(
    state: &AppState,
    account: &Account,
    id: &str,
    position: &str,
) -> Result<(Assignment, u32), ApiError> {
    let id: Uuid = id.parse().map_err(|_| assignment_not_found())?;
    let assignment = exams::assignments_of(&state.pool, &account.sub, Some(id))
        .await?
        .pop()
        .ok_or_else(assignment_not_found)?;
    let position: u32 = position
        .parse()
        .ok()
        .filter(|&p| (p as usize) < assignment.exam.items.len())
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "그런 문항이 없다"))?;
    Ok((assignment, position))
}

/// `POST /assignments/{id}/items/{n}/start` — 문항 응시를 시작하고(처음 한 번 시각을 남긴다) 문제를 준다.
pub async fn start_item(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((id, position)): Path<(String, String)>,
    report: Option<Json<StartItemRequest>>,
) -> Result<Json<ItemStarted>, ApiError> {
    let (assignment, position) = my_item(&state, &account, &id, &position).await?;
    if assignment.submitted.contains_key(&position) {
        return Err(conflict("이미 낸 문항이다"));
    }
    // 시범 조직 승인(#19): 승인을 거둔 조직은 이미 만든 시험도 새 문항을 시작하지 않는다.
    crate::operator::require_approved(&state, assignment.exam.org_id).await?;
    // 응시 가능 시간(#14): 채용 시험이 닫혀 있으면 새 문항을 시작하지 않는다(이미 시작한 문항은 그대로).
    crate::live::require_window(&state, &assignment, position).await?;
    // 허용 목록(#12): 앱 판·모델 sha256 신고값이 목록에 없으면 시작하지 않고(시각을 남기지 않는다) 신호로 남긴다.
    // 웹캠 동의·기준 사진(#13). 사람 응시자만 — 스모크 기계 주체는 면제.
    crate::webcam::require_ready(&state, &account, assignment.id).await?;
    let report = report.map(|Json(r)| r);
    // 허용 목록 = 상수 ∪ 운영자가 더한 값 − 막은 값(#19).
    let allowlist = crate::operator::allowlist(&state).await?;
    if let Err(failure) = allowlist.check(report.as_ref()) {
        let now = state.clock.now();
        promptlab_store::integrity::record_rejection(
            &state.pool,
            assignment.id,
            position,
            report.as_ref(),
            failure,
            now,
        )
        .await?;
        tracing::warn!(
            invite_id = %assignment.id,
            position,
            app_ok = failure.app_ok,
            model_ok = failure.model_ok,
            "허용 목록에 없는 앱 판·모델 — 문항 시작 거부"
        );
        return Err(ApiError::new(StatusCode::FORBIDDEN, failure.message()));
    }
    let item = &assignment.exam.items[position as usize];
    let problem = problems::find_for_org(&state.pool, assignment.exam.org_id, &item.problem_id)
        .await?
        .ok_or_else(|| {
            tracing::error!(problem_id = %item.problem_id, "시험 문항의 문제가 문제은행에 없다");
            ApiError::new(StatusCode::GONE, "이 문항의 문제를 더는 쓸 수 없다")
        })?;
    let now = state.clock.now();
    let started_at = exams::start_item(
        &state.pool,
        assignment.id,
        position,
        report.as_ref().map(|r| r.model_sha256.as_str()),
        now,
    )
    .await?;
    let allowed = assignment.exam.settings.allowed_languages.clone();
    let mut public = problem.public_view();
    public.templates.retain(|t| allowed.contains(&t.language));
    Ok(Json(ItemStarted {
        problem: public,
        allowed_languages: allowed,
        limits: item.limits,
        started_at,
        deadline: started_at + i64::from(item.limits.time_limit_secs),
        now,
    }))
}

/// `POST /assignments/{id}/items/{n}/submit` — 코드를 낸다. 과정 로그(검증된 체인)가 정본이다: 앱이 쓴 사건(`log_seq`개)
/// 안의 **마지막 [`LogEvent::Submitted`]**의 언어·코드 sha256이 본문과 같아야 받는다. 제한 시간은 서버가 강제한다.
///
/// - 코드가 비었거나 [`MAX_CODE_BYTES`](promptlab_shared::coding::MAX_CODE_BYTES)를 넘으면 `422`.
/// - 앱이 쓴 사건(`log_seq`)이 아직 다 올라오지 않았으면 `409`(앱은 로그를 비우고 다시 낸다).
/// - 그 안에 제출 사건이 없거나 언어·sha256이 다르면 `422`.
pub async fn submit_item(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((id, position)): Path<(String, String)>,
    Json(request): Json<SubmitItemRequest>,
) -> Result<(StatusCode, Json<SubmissionView>), ApiError> {
    let (assignment, position) = my_item(&state, &account, &id, &position).await?;
    if assignment.submitted.contains_key(&position) {
        return Err(conflict("이미 낸 문항이다"));
    }
    let Some(&started_at) = assignment.started.get(&position) else {
        return Err(conflict("시작하지 않은 문항이다"));
    };
    let item = &assignment.exam.items[position as usize];
    let now = state.clock.now();
    let deadline = started_at + i64::from(item.limits.time_limit_secs);
    if now > deadline + SUBMIT_GRACE_SECS {
        return Err(conflict("제한 시간이 지나 제출을 받지 않는다"));
    }
    crate::check_code(&request.code)?;
    if !assignment
        .exam
        .settings
        .allowed_languages
        .contains(&request.language)
    {
        return Err(unprocessable(format!(
            "이 시험은 {} 언어를 허용하지 않는다",
            request.language.as_str()
        )));
    }
    let received = promptlab_store::logs::next_seq(&state.pool, assignment.id, position).await?;
    if received < request.log_seq {
        return Err(conflict(format!(
            "과정 로그가 아직 다 올라오지 않았다(받은 사건 {received}개, 앱 {}개) — 로그를 올린 뒤 다시 내라",
            request.log_seq
        )));
    }
    let events = promptlab_store::logs::item_events(&state.pool, assignment.id, position).await?;
    let logged = events
        .iter()
        .filter(|(seq, _)| *seq < request.log_seq)
        .rev()
        .find_map(|(_, event)| match event {
            LogEvent::Submitted {
                language,
                code_sha256,
            } => Some((*language, code_sha256.as_str())),
            _ => None,
        });
    let Some((language, sha256)) = logged else {
        return Err(unprocessable(
            "과정 로그에 제출 사건이 없다 — 앱에서 다시 내라",
        ));
    };
    if language != request.language || sha256 != code_sha256(&request.code) {
        return Err(unprocessable(
            "제출한 코드가 과정 로그의 제출 사건과 다르다",
        ));
    }
    let owner = submissions::Owner {
        candidate_sub: account.sub.clone(),
        org_id: Some(assignment.exam.org_id),
        item: Some(submissions::ItemRef {
            invite_id: assignment.id,
            position,
        }),
    };
    // 소요 시간은 서버가 잰다(시작부터 지금까지, 제한 시간을 넘지 않게).
    let elapsed = (now - started_at).clamp(0, i64::from(item.limits.time_limit_secs));
    let submission = submissions::NewSubmission {
        language: request.language,
        code: request.code,
        elapsed_secs: u32::try_from(elapsed).unwrap_or(0),
    };
    let view = match submissions::insert(&state.pool, &owner, &item.problem_id, &submission).await {
        Ok(view) => view,
        Err(e) if submissions::is_unique_violation(&e) => return Err(conflict("이미 낸 문항이다")),
        Err(e) => return Err(e.into()),
    };
    tracing::info!(id = %view.id, invite_id = %assignment.id, position, "시험 문항 제출·큐에 넣음");
    Ok((
        StatusCode::ACCEPTED,
        Json(for_candidate(
            view,
            assignment.exam.purpose,
            assignment.exam.reveal_scores,
        )),
    ))
}
