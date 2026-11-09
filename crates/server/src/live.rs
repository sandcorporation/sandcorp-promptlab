//! 실시간 감독(#14): 채용 시험의 응시 가능 시간과 WebRTC 실시간 보기 시그널링. 규칙은 [`promptlab_shared::live`].
//!
//! 시그널링은 폴링이다(웹소켓 없이 기존 인증 미들웨어를 그대로 탄다 — 앱은 베어러, 대시보드는 세션 쿠키 + Origin).
//! 세션과 메시지는 이 프로세스 메모리에만 있다([`Hub`]): 서버가 하나라는 전제(워커와 같다)이고, 서버가 다시 뜨면
//! 열린 세션은 사라져 양쪽이 「닫힘」을 보고 멈춘다. 미디어는 서버를 거치지 않는다.
//!
//! 경로:
//! - 감독(관리자·감독관, 다른 조직 404, 출제자·열람자 403):
//!   `GET`·`PUT /orgs/{id}/exams/{exam}/window`,
//!   `POST /orgs/{id}/exams/{exam}/invites/{invite}/live`(세션 열기, TURN 설정이 없으면 503),
//!   `GET`·`POST …/live/{session}/signals`, `DELETE …/live/{session}`(닫기).
//! - 응시자(자기 배정만, 기계 주체 403): `GET /assignments/{id}/live`,
//!   `GET`·`POST /assignments/{id}/live/{session}/signals`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use promptlab_shared::account::Permission;
use promptlab_shared::live::{
    ExamWindow, ExamWindowView, LiveRequest, LiveSession, MAX_SESSION_SECS, MAX_SIGNAL_BYTES,
    MAX_SIGNALS_PER_SIDE, PostSignal, Signal, SignalBatch, SignalKind, VIEWER_IDLE_SECS,
};
use promptlab_store::exams::{self, Assignment};
use promptlab_store::live as store;
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::Account;
use crate::exams::find_exam;
use crate::orgs::require;
use crate::turn::TurnConfig;
use crate::{ApiError, AppState};

/// 실시간 감독 상태: TURN 설정(없으면 꺼짐)과 열린 세션들.
#[derive(Clone, Debug, Default)]
pub struct Live {
    turn: Option<Arc<TurnConfig>>,
    hub: Arc<Hub>,
}

impl Live {
    pub fn new(turn: Option<TurnConfig>) -> Self {
        Self {
            turn: turn.map(Arc::new),
            hub: Arc::default(),
        }
    }

    /// TURN 설정이 없어 실시간 보기가 꺼져 있다.
    pub fn disabled() -> Self {
        Self::default()
    }

    pub fn enabled(&self) -> bool {
        self.turn.is_some()
    }
}

/// 열린 세션들(세션 id → 세션). 접근할 때마다 지난 세션을 치운다.
#[derive(Debug, Default)]
pub struct Hub {
    sessions: Mutex<HashMap<Uuid, Session>>,
}

#[derive(Debug)]
struct Session {
    invite_id: Uuid,
    expires_at: i64,
    viewer_seen: i64,
    to_candidate: Vec<Signal>,
    to_viewer: Vec<Signal>,
}

/// 메시지를 받는 쪽.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Candidate,
    Viewer,
}

impl Session {
    fn inbox(&mut self, side: Side) -> &mut Vec<Signal> {
        match side {
            Side::Candidate => &mut self.to_candidate,
            Side::Viewer => &mut self.to_viewer,
        }
    }
}

impl Hub {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Uuid, Session>> {
        self.sessions.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 수명이 끝났거나 감독관 폴링이 끊긴 세션을 닫는다.
    fn sweep(map: &mut HashMap<Uuid, Session>, now: i64) {
        map.retain(|id, s| {
            let alive = now < s.expires_at && now - s.viewer_seen <= VIEWER_IDLE_SECS;
            if !alive {
                tracing::info!(session = %id, invite_id = %s.invite_id, "실시간 보기 세션 만료");
            }
            alive
        });
    }

    /// 그 배정의 새 세션을 연다(같은 배정의 옛 세션은 닫는다 — 한 응시에 송출은 하나).
    fn open(&self, invite_id: Uuid, now: i64, ttl: i64) -> (Uuid, i64) {
        let mut map = self.lock();
        Self::sweep(&mut map, now);
        map.retain(|_, s| s.invite_id != invite_id);
        let id = Uuid::new_v4();
        let expires_at = now + ttl.min(MAX_SESSION_SECS);
        map.insert(
            id,
            Session {
                invite_id,
                expires_at,
                viewer_seen: now,
                to_candidate: Vec::new(),
                to_viewer: Vec::new(),
            },
        );
        (id, expires_at)
    }

    /// 그 배정의 열린 세션.
    fn current(&self, invite_id: Uuid, now: i64) -> Option<(Uuid, i64)> {
        let mut map = self.lock();
        Self::sweep(&mut map, now);
        map.iter()
            .find(|(_, s)| s.invite_id == invite_id)
            .map(|(id, s)| (*id, s.expires_at))
    }

    /// `side` 쪽이 받을 메시지(`after` 뒤). 세션이 없거나 다른 배정의 것이면 닫힘.
    fn poll(&self, id: Uuid, invite_id: Uuid, side: Side, after: u64, now: i64) -> SignalBatch {
        let mut map = self.lock();
        Self::sweep(&mut map, now);
        match map.get_mut(&id).filter(|s| s.invite_id == invite_id) {
            Some(s) => {
                if side == Side::Viewer {
                    s.viewer_seen = now;
                }
                SignalBatch {
                    signals: s
                        .inbox(side)
                        .iter()
                        .filter(|m| m.seq > after)
                        .cloned()
                        .collect(),
                    open: true,
                }
            }
            None => SignalBatch {
                signals: Vec::new(),
                open: false,
            },
        }
    }

    /// `from` 쪽이 보낸 메시지를 상대 받은편지함에 넣는다. `bye`면 세션을 닫는다.
    fn post(
        &self,
        id: Uuid,
        invite_id: Uuid,
        from: Side,
        signal: PostSignal,
        now: i64,
    ) -> Result<(), ApiError> {
        let allowed = match from {
            Side::Candidate => signal.kind.from_candidate(),
            Side::Viewer => signal.kind.from_viewer(),
        };
        if !allowed {
            return Err(ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "이쪽에서 보낼 수 없는 메시지 종류다",
            ));
        }
        if signal.data.len() > MAX_SIGNAL_BYTES {
            return Err(ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "메시지가 너무 크다",
            ));
        }
        let mut map = self.lock();
        Self::sweep(&mut map, now);
        let Some(session) = map.get_mut(&id).filter(|s| s.invite_id == invite_id) else {
            return Err(ApiError::new(StatusCode::GONE, "실시간 보기가 닫혔다"));
        };
        if from == Side::Viewer {
            session.viewer_seen = now;
        }
        if signal.kind == SignalKind::Bye {
            map.remove(&id);
            tracing::info!(session = %id, invite_id = %invite_id, ?from, "실시간 보기 닫음");
            return Ok(());
        }
        let to = match from {
            Side::Candidate => Side::Viewer,
            Side::Viewer => Side::Candidate,
        };
        let inbox = session.inbox(to);
        if inbox.len() >= MAX_SIGNALS_PER_SIDE {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "메시지가 너무 많다",
            ));
        }
        let seq = inbox.len() as u64 + 1;
        inbox.push(Signal {
            seq,
            kind: signal.kind,
            data: signal.data,
        });
        Ok(())
    }
}

/// `?after=N`.
#[derive(Debug, Deserialize)]
pub struct After {
    #[serde(default)]
    after: u64,
}

fn parse_session(id: &str) -> Result<Uuid, ApiError> {
    id.parse()
        .map_err(|_| ApiError::new(StatusCode::NOT_FOUND, "그런 실시간 보기가 없다"))
}

fn session_view(
    state: &AppState,
    turn: &TurnConfig,
    id: Uuid,
    expires_at: i64,
    who: &str,
) -> LiveSession {
    // 꼬리표는 세션 id 앞 8자와 쪽 — 응시자 정보를 TURN 로그에 남기지 않는다.
    let label = format!("{who}-{}", &id.simple().to_string()[..8]);
    LiveSession {
        session_id: id.to_string(),
        ice_servers: vec![turn.ice_server(state.clock.now(), &label)],
        expires_at,
    }
}

// ───────────────────────── 응시 가능 시간 ─────────────────────────

/// `GET /orgs/{id}/exams/{exam}/window`
pub async fn get_window(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((org_id, exam_id)): Path<(String, String)>,
) -> Result<Json<ExamWindowView>, ApiError> {
    let org = require(&state, &org_id, &account, Permission::Proctor).await?;
    let exam = find_exam(&state, org.org_id, &exam_id).await?;
    let (purpose, window) = store::window_of_exam(&state.pool, exam.id)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "그런 시험이 없다"))?;
    Ok(Json(ExamWindowView::new(
        purpose,
        window,
        state.clock.now(),
    )))
}

/// `PUT /orgs/{id}/exams/{exam}/window` — 채용 시험의 응시 가능 시간을 열고 닫는다(감독 권한).
pub async fn put_window(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((org_id, exam_id)): Path<(String, String)>,
    Json(window): Json<ExamWindow>,
) -> Result<Json<ExamWindowView>, ApiError> {
    let org = require(&state, &org_id, &account, Permission::Proctor).await?;
    let exam = find_exam(&state, org.org_id, &exam_id).await?;
    window
        .validate()
        .map_err(|m| ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, m))?;
    if !store::set_window(&state.pool, exam.id, &window).await? {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "응시 가능 시간은 채용 시험에만 건다",
        ));
    }
    tracing::info!(exam_id = %exam.id, by = %account.sub, open = window.open, "응시 가능 시간 바꿈");
    Ok(Json(ExamWindowView::new(
        exam.purpose,
        window,
        state.clock.now(),
    )))
}

/// 문항 `start` 앞에서: 채용 시험이 닫혀 있으면 403. 이미 시작한 문항(다시 부르기)은 막지 않는다.
pub(crate) async fn require_window(
    state: &AppState,
    assignment: &Assignment,
    position: u32,
) -> Result<(), ApiError> {
    if assignment.started.contains_key(&position) {
        return Ok(());
    }
    let Some((purpose, window, is_smoke)) =
        store::window_of_invite(&state.pool, assignment.id).await?
    else {
        return Ok(());
    };
    if is_smoke || ExamWindowView::new(purpose, window, state.clock.now()).open_now {
        return Ok(());
    }
    Err(ApiError::new(
        StatusCode::FORBIDDEN,
        "지금은 응시 가능 시간이 아니다 — 감독관이 응시를 열면 다시 시작하세요",
    ))
}

// ───────────────────────── 실시간 보기: 감독관 ─────────────────────────

async fn viewer_invite(
    state: &AppState,
    account: &Account,
    org_id: &str,
    exam_id: &str,
    invite_id: &str,
) -> Result<Uuid, ApiError> {
    let (_, invite, _) = crate::logs::invite_with(
        state,
        account,
        org_id,
        exam_id,
        invite_id,
        Permission::Proctor,
    )
    .await?;
    Ok(invite.id)
}

/// `POST /orgs/{id}/exams/{exam}/invites/{invite}/live` — 실시간 보기를 연다(앱이 다음 폴링에서 송출을 시작한다).
pub async fn open_live(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((org_id, exam_id, invite_id)): Path<(String, String, String)>,
) -> Result<(StatusCode, Json<LiveSession>), ApiError> {
    let (_, invite, _) = crate::logs::invite_with(
        &state,
        &account,
        &org_id,
        &exam_id,
        &invite_id,
        Permission::Proctor,
    )
    .await?;
    let Some(turn) = state.live.turn.clone() else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "실시간 감독이 꺼져 있다(TURN 설정 없음)",
        ));
    };
    if !invite.accepted {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "아직 시험을 배정받지 않은 응시자다",
        ));
    }
    let now = state.clock.now();
    let (id, expires_at) = state.live.hub.open(invite.id, now, turn.ttl_secs());
    tracing::info!(session = %id, invite_id = %invite.id, by = %account.sub, "실시간 보기 열림");
    Ok((
        StatusCode::CREATED,
        Json(session_view(&state, &turn, id, expires_at, "v")),
    ))
}

/// `GET …/live/{session}/signals?after=N` — 감독관이 받을 메시지(폴링이 곧 감독관 연결 신호다).
pub async fn viewer_signals(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((org_id, exam_id, invite_id, session)): Path<(String, String, String, String)>,
    Query(q): Query<After>,
) -> Result<Json<SignalBatch>, ApiError> {
    let invite = viewer_invite(&state, &account, &org_id, &exam_id, &invite_id).await?;
    let id = parse_session(&session)?;
    Ok(Json(state.live.hub.poll(
        id,
        invite,
        Side::Viewer,
        q.after,
        state.clock.now(),
    )))
}

/// `POST …/live/{session}/signals` — 감독관이 answer·ICE 후보·bye를 보낸다.
pub async fn viewer_post(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((org_id, exam_id, invite_id, session)): Path<(String, String, String, String)>,
    Json(signal): Json<PostSignal>,
) -> Result<StatusCode, ApiError> {
    let invite = viewer_invite(&state, &account, &org_id, &exam_id, &invite_id).await?;
    let id = parse_session(&session)?;
    state
        .live
        .hub
        .post(id, invite, Side::Viewer, signal, state.clock.now())?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE …/live/{session}` — 실시간 보기를 닫는다(앱은 다음 폴링에서 송출을 멈춘다). 없는 세션도 204.
pub async fn close_live(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((org_id, exam_id, invite_id, session)): Path<(String, String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let invite = viewer_invite(&state, &account, &org_id, &exam_id, &invite_id).await?;
    let id = parse_session(&session)?;
    let bye = PostSignal {
        kind: SignalKind::Bye,
        data: String::new(),
    };
    match state
        .live
        .hub
        .post(id, invite, Side::Viewer, bye, state.clock.now())
    {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(e) if e.status() == StatusCode::GONE => Ok(StatusCode::NO_CONTENT),
        Err(e) => Err(e),
    }
}

// ───────────────────────── 실시간 보기: 응시자(앱) ─────────────────────────

async fn my_assignment(state: &AppState, account: &Account, id: &str) -> Result<Uuid, ApiError> {
    let not_found = || ApiError::new(StatusCode::NOT_FOUND, "그런 배정이 없다");
    let id: Uuid = id.parse().map_err(|_| not_found())?;
    exams::assignments_of(&state.pool, &account.sub, Some(id))
        .await?
        .pop()
        .map(|a| a.id)
        .ok_or_else(not_found)
}

/// `GET /assignments/{id}/live` — 감독관이 열었으면 세션(송출하라는 뜻), 아니면 없음.
pub async fn candidate_request(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path(id): Path<String>,
) -> Result<Json<LiveRequest>, ApiError> {
    let invite = my_assignment(&state, &account, &id).await?;
    let session = match &state.live.turn {
        Some(turn) => state
            .live
            .hub
            .current(invite, state.clock.now())
            .map(|(id, expires_at)| session_view(&state, turn, id, expires_at, "c")),
        None => None,
    };
    Ok(Json(LiveRequest { session }))
}

/// `GET /assignments/{id}/live/{session}/signals?after=N`
pub async fn candidate_signals(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((id, session)): Path<(String, String)>,
    Query(q): Query<After>,
) -> Result<Json<SignalBatch>, ApiError> {
    let invite = my_assignment(&state, &account, &id).await?;
    let id = parse_session(&session)?;
    Ok(Json(state.live.hub.poll(
        id,
        invite,
        Side::Candidate,
        q.after,
        state.clock.now(),
    )))
}

/// `POST /assignments/{id}/live/{session}/signals` — 앱이 offer·ICE 후보·bye를 보낸다.
pub async fn candidate_post(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((id, session)): Path<(String, String)>,
    Json(signal): Json<PostSignal>,
) -> Result<StatusCode, ApiError> {
    let invite = my_assignment(&state, &account, &id).await?;
    let id = parse_session(&session)?;
    state
        .live
        .hub
        .post(id, invite, Side::Candidate, signal, state.clock.now())?;
    Ok(StatusCode::NO_CONTENT)
}
