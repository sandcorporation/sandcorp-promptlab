//! 과정 로그(#9). 형식·검증은 [`promptlab_shared::log`], 저장은 [`promptlab_store::logs`].
//!
//! - 응시 앱이 문항을 시작한 뒤 설치 공개키를 등록하고(`log/keys`), 서명한 사건을 순번대로 올린다(`log`).
//! - **검증 정책**: 서명·체인·순번이 맞지 않는 업로드는 거부하고(아무것도 저장하지 않는다) 거부 횟수·까닭을
//!   남겨 다시보기에 신호로 보인다. 저장된 로그는 언제나 검증을 통과한 체인이라 다시보기가 믿을 수 있다.
//!   거부할 수 없는 누락(끝까지 안 올라옴, 다른 설치 키로 이어짐)은 신호로만 표시한다 — 판단은 고객사.
//! - 다시보기(`GET …/log`)와 의심 신호 목록(`GET …/signals`, #12)은 결과 보기 권한. 다른 조직이면 404.

use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use promptlab_shared::account::Permission;
use promptlab_shared::api::{
    LogHead, ProcessLogView, RegisterLogKeyRequest, ReplayEvent, SuspicionView, UploadLogRequest,
};
use promptlab_shared::integrity::{SignalClock, Suspicion, SuspicionKind, clip_reported};
use promptlab_shared::log::{
    self, Attempt, Chain, LogEvent, LogSignal, MAX_EVENTS_PER_UPLOAD, is_valid_public_key,
};
use promptlab_shared::webcam::SnapshotReason;
use promptlab_store::exams::{Exam, InviteRow};
use promptlab_store::{exams, integrity, logs};
use uuid::Uuid;

use crate::auth::Account;
use crate::exams::{find_exam, my_item};
use crate::orgs::require;
use crate::{ApiError, AppState};

fn not_started() -> ApiError {
    ApiError::new(StatusCode::CONFLICT, "시작하지 않은 문항이다")
}

/// `POST /assignments/{id}/items/{n}/log/keys` — 이 설치의 공개키를 등록하고 서버가 아는 체인 머리를 준다.
pub async fn register_key(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((id, position)): Path<(String, String)>,
    Json(request): Json<RegisterLogKeyRequest>,
) -> Result<Json<LogHead>, ApiError> {
    let (assignment, position) = my_item(&state, &account, &id, &position).await?;
    if !is_valid_public_key(&request.public_key) {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Ed25519 공개키(base64url 32바이트)가 아니다",
        ));
    }
    let now = state.clock.now();
    match logs::register_key(
        &state.pool,
        assignment.id,
        position,
        &request.public_key,
        now,
    )
    .await?
    {
        None => Err(not_started()),
        Some(logs::KeyOutcome::Registered(chain)) => Ok(Json(chain)),
        Some(logs::KeyOutcome::TooManyKeys) => Err(ApiError::new(
            StatusCode::CONFLICT,
            format!(
                "이 문항에는 키를 {}개까지만 등록한다",
                log::MAX_KEYS_PER_ITEM
            ),
        )),
    }
}

/// `POST /assignments/{id}/items/{n}/log` — 서명된 사건을 순번대로 받는다.
pub async fn upload(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((id, position)): Path<(String, String)>,
    Json(request): Json<UploadLogRequest>,
) -> Result<Json<LogHead>, ApiError> {
    let (assignment, position) = my_item(&state, &account, &id, &position).await?;
    if request.events.len() > MAX_EVENTS_PER_UPLOAD {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("한 번에 사건 {MAX_EVENTS_PER_UPLOAD}개까지 올린다"),
        ));
    }
    let now = state.clock.now();
    match logs::append(&state.pool, assignment.id, position, &request.events, now).await? {
        None => Err(not_started()),
        Some(logs::AppendOutcome::Accepted(chain)) => Ok(Json(chain)),
        Some(logs::AppendOutcome::TooMany) => Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("이 문항의 사건이 {}개를 넘는다", log::MAX_EVENTS_PER_ITEM),
        )),
        Some(logs::AppendOutcome::Rejected(e)) => {
            let reason = e.to_string();
            logs::record_rejection(&state.pool, assignment.id, position, &reason, now).await?;
            tracing::warn!(invite_id = %assignment.id, position, %reason, "과정 로그 업로드 거부");
            let status = match e {
                log::LogError::Gap { .. } | log::LogError::Conflict { .. } => StatusCode::CONFLICT,
                _ => StatusCode::UNPROCESSABLE_ENTITY,
            };
            Err(ApiError::new(status, reason))
        }
    }
}

/// 저장된 문항 로그를 다시 검증한 결과.
pub(crate) struct VerifiedLog {
    pub(crate) started_at: Option<i64>,
    pub(crate) submission_id: Option<Uuid>,
    pub(crate) events: Vec<ReplayEvent>,
    pub(crate) signals: Vec<LogSignal>,
}

/// 문항 하나의 저장된 체인을 다시 검증한다(저장 뒤 바뀐 것을 잡는다). 처음 어긋난 곳부터는 보이지 않는다.
pub(crate) async fn verified_log(
    state: &AppState,
    invite_id: Uuid,
    position: u32,
) -> Result<VerifiedLog, ApiError> {
    let (started_at, submission_id) = logs::item_progress(&state.pool, invite_id, position).await?;
    let stored = logs::item_log(&state.pool, invite_id, position).await?;
    let attempt = Attempt {
        assignment_id: invite_id.to_string(),
        position,
    };
    let mut chain = Chain::default();
    let mut events = Vec::with_capacity(stored.events.len());
    let mut signals = Vec::new();
    let mut last_key: Option<String> = None;
    for s in &stored.events {
        let opened = match log::open(&s.event, &attempt, &stored.keys)
            .and_then(|o| chain.append(&o).map(|()| o))
        {
            Ok(o) => o,
            Err(_) => {
                signals.push(LogSignal::Tampered { seq: s.seq });
                break;
            }
        };
        if last_key.as_ref().is_some_and(|k| *k != opened.body.key) {
            signals.push(LogSignal::KeyChanged {
                seq: opened.body.seq,
            });
        }
        let key_index = stored
            .keys
            .iter()
            .position(|k| *k == opened.body.key)
            .unwrap_or(0);
        last_key = Some(opened.body.key.clone());
        events.push(ReplayEvent {
            seq: opened.body.seq,
            at_ms: opened.body.at_ms,
            received_at: s.received_at,
            key_index: u32::try_from(key_index).unwrap_or(0),
            event: opened.body.event,
        });
    }
    if submission_id.is_some()
        && !events
            .iter()
            .any(|e| matches!(e.event, LogEvent::Submitted { .. }))
    {
        signals.push(LogSignal::Incomplete);
    }
    if let Some(r) = stored.rejections {
        signals.push(LogSignal::Rejected {
            count: r.count,
            last_reason: r.last_reason,
            last_at: r.last_at,
        });
    }
    Ok(VerifiedLog {
        started_at,
        submission_id,
        events,
        signals,
    })
}

/// 결과 보기 권한으로 조직·시험·초대를 찾는다. 다른 조직·없는 초대는 404.
pub(crate) async fn viewable_invite(
    state: &AppState,
    account: &Account,
    org_id: &str,
    exam_id: &str,
    invite_id: &str,
) -> Result<(Exam, InviteRow), ApiError> {
    invite_for(
        state,
        account,
        org_id,
        exam_id,
        invite_id,
        Permission::ViewResults,
    )
    .await
}

/// 권한 `permission`으로 조직·시험·초대와 응시자 이름을 찾는다(웹캠 사진은 감독 권한, #13).
pub(crate) async fn invite_with(
    state: &AppState,
    account: &Account,
    org_id: &str,
    exam_id: &str,
    invite_id: &str,
    permission: Permission,
) -> Result<(Exam, InviteRow, String), ApiError> {
    let (exam, invite) = invite_for(state, account, org_id, exam_id, invite_id, permission).await?;
    let candidate = candidate_label(invite.clone());
    Ok((exam, invite, candidate))
}

async fn invite_for(
    state: &AppState,
    account: &Account,
    org_id: &str,
    exam_id: &str,
    invite_id: &str,
    permission: Permission,
) -> Result<(Exam, InviteRow), ApiError> {
    let org = require(state, org_id, account, permission).await?;
    let exam = find_exam(state, org.org_id, exam_id).await?;
    let not_found = || ApiError::new(StatusCode::NOT_FOUND, "그런 응시가 없다");
    let invite_id: Uuid = invite_id.parse().map_err(|_| not_found())?;
    let invite = exams::list_invites(&state.pool, exam.id)
        .await?
        .into_iter()
        .find(|i| i.id == invite_id)
        .ok_or_else(not_found)?;
    Ok((exam, invite))
}

fn candidate_label(invite: InviteRow) -> String {
    match invite.candidate_name {
        Some(name) => format!("{name} <{}>", invite.email),
        None => invite.email,
    }
}

/// `GET /orgs/{id}/exams/{exam}/invites/{invite}/items/{n}/log` — 응시 문항 하나의 과정 다시보기.
pub async fn replay(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((org_id, exam_id, invite_id, position)): Path<(String, String, String, String)>,
) -> Result<Json<ProcessLogView>, ApiError> {
    let (exam, invite) = viewable_invite(&state, &account, &org_id, &exam_id, &invite_id).await?;
    let position: u32 = position
        .parse()
        .ok()
        .filter(|&p| (p as usize) < exam.items.len())
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "그런 응시가 없다"))?;
    let log = verified_log(&state, invite.id, position).await?;
    Ok(Json(ProcessLogView {
        candidate: candidate_label(invite),
        position,
        started_at: log.started_at,
        submission_id: log.submission_id.map(|id| id.to_string()),
        events: log.events,
        signals: log.signals,
    }))
}

/// `GET /orgs/{id}/exams/{exam}/invites/{invite}/signals` — 응시자 한 명의 의심 신호를 시간순으로(#12).
/// 과정 로그(검증된 체인)의 포커스 이탈·전체화면 이탈·붙여넣기 막음·웹캠 감지, 로그 검증 신호, 허용 목록 거부를
/// 모은다. 판단은 고객사가 한다.
pub async fn suspicions(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((org_id, exam_id, invite_id)): Path<(String, String, String)>,
) -> Result<Json<SuspicionView>, ApiError> {
    let (exam, invite) = viewable_invite(&state, &account, &org_id, &exam_id, &invite_id).await?;
    Ok(Json(build_suspicions(&state, &exam, invite).await?))
}

/// 응시자 한 명의 의심 신호 목록을 만든다(권한은 부르는 쪽이 본다 — AI 리포트(#15)도 이것을 읽는다).
pub(crate) async fn build_suspicions(
    state: &AppState,
    exam: &Exam,
    invite: InviteRow,
) -> Result<SuspicionView, ApiError> {
    let mut signals = Vec::new();
    let app = |at_ms: i64, position: u32, kind: SuspicionKind| Suspicion {
        at_ms: Some(at_ms),
        clock: SignalClock::App,
        position,
        kind,
    };
    for position in 0..u32::try_from(exam.items.len()).unwrap_or(0) {
        let log = verified_log(state, invite.id, position).await?;
        // 포커스 이탈은 바로 뒤의 복귀와 짝지어 비운 시간을 붙인다.
        let mut away: Option<usize> = None;
        for e in &log.events {
            match &e.event {
                LogEvent::FocusLost => {
                    away = Some(signals.len());
                    signals.push(app(
                        e.at_ms,
                        position,
                        SuspicionKind::FocusLost {
                            seq: e.seq,
                            away_ms: None,
                        },
                    ));
                }
                LogEvent::FocusReturned { away_ms } => {
                    if let Some(Suspicion {
                        kind: SuspicionKind::FocusLost { away_ms: slot, .. },
                        ..
                    }) = away.take().and_then(|i| signals.get_mut(i))
                    {
                        *slot = Some(*away_ms);
                    }
                }
                LogEvent::FullscreenExited => {
                    signals.push(app(
                        e.at_ms,
                        position,
                        SuspicionKind::FullscreenExited { seq: e.seq },
                    ));
                }
                LogEvent::PasteBlocked { chars } => signals.push(app(
                    e.at_ms,
                    position,
                    SuspicionKind::PasteBlocked {
                        seq: e.seq,
                        chars: *chars,
                    },
                )),
                // 웹캠 감지(#13). 스냅샷 참조는 앱 신고값이라 길이를 자른다.
                LogEvent::NoFace { snapshot }
                | LogEvent::GazeAway { snapshot }
                | LogEvent::MultipleFaces { snapshot, .. } => {
                    let (reason, count) = match &e.event {
                        LogEvent::NoFace { .. } => (SnapshotReason::NoFace, None),
                        LogEvent::GazeAway { .. } => (SnapshotReason::GazeAway, None),
                        LogEvent::MultipleFaces { count, .. } => {
                            (SnapshotReason::MultipleFaces, Some(*count))
                        }
                        _ => unreachable!("바깥 match가 고른 세 종류"),
                    };
                    signals.push(app(
                        e.at_ms,
                        position,
                        SuspicionKind::Webcam {
                            seq: e.seq,
                            reason,
                            count,
                            snapshot: snapshot.as_deref().map(clip_reported),
                        },
                    ));
                }
                LogEvent::CameraUnavailable { reason } => signals.push(app(
                    e.at_ms,
                    position,
                    SuspicionKind::CameraUnavailable {
                        seq: e.seq,
                        reason: clip_reported(reason),
                    },
                )),
                _ => {}
            }
        }
        for signal in log.signals {
            let (at_ms, clock) = match &signal {
                LogSignal::KeyChanged { seq } => (
                    log.events.iter().find(|e| e.seq == *seq).map(|e| e.at_ms),
                    SignalClock::App,
                ),
                LogSignal::Rejected { last_at, .. } => (Some(last_at * 1000), SignalClock::Server),
                LogSignal::Incomplete | LogSignal::Tampered { .. } => (None, SignalClock::Server),
            };
            signals.push(Suspicion {
                at_ms,
                clock,
                position,
                kind: SuspicionKind::Log { signal },
            });
        }
    }
    for r in integrity::rejections(&state.pool, invite.id).await? {
        signals.push(Suspicion {
            at_ms: Some(r.last_at * 1000),
            clock: SignalClock::Server,
            position: r.position,
            kind: SuspicionKind::IntegrityRejected {
                app_version: r.app_version,
                model_sha256: r.model_sha256,
                app_ok: r.app_ok,
                model_ok: r.model_ok,
                count: r.count,
            },
        });
    }
    // 시간순(시각을 모르는 것은 끝에). 같은 시각이면 모은 순서(문항·순번) 그대로.
    signals.sort_by_key(|s| (s.at_ms.is_none(), s.at_ms));
    Ok(SuspicionView {
        candidate: candidate_label(invite),
        signals,
    })
}
