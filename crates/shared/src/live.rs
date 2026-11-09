//! 실시간 감독(#14): 채용 시험의 응시 가능 시간과 WebRTC 실시간 보기의 시그널링 약속.
//!
//! ## 응시 가능 시간
//! - 채용 시험([`Purpose::Recruiting`])에만 적용한다([`window_applies`]). 사내 진단은 구성원이 제 시간에 보는
//!   자기 진단이라 열고 닫지 않는다(서버가 설정 요청을 422로 거부한다).
//! - 감독 권한(관리자·감독관)이 연다·닫는다([`ExamWindow::open`]). 시각 범위(`starts_at`~`ends_at`)를 더 걸 수 있다.
//!   응시 가능 = 열림이고 범위 안([`ExamWindow::allows`]). 기본은 열림·범위 없음 — 이전에 만든 시험은 그대로 열려 있다.
//! - 닫힌 동안 서버는 문항 `start`를 거부한다(시작 시각을 남기지 않는다). **이미 시작한 문항은 그대로 진행한다** —
//!   시작 시각부터 제한 시간을 서버가 이미 강제하므로 끝은 정해져 있고, 푸는 도중 끊으면 감독관이 닫은 시각에 따라
//!   응시자 사이에 불공평이 생긴다. 닫힌 뒤에는 다음 문항을 시작하지 못한다.
//! - 스모크 조직의 시험은 늘 열려 있다(기계 주체는 조직 경로에 닿지 못해 바꿀 수도 없다).
//!
//! ## 실시간 보기(WebRTC)
//! - 감독관이 대시보드에서 「실시간 보기」를 누를 때만 세션이 생기고, 앱은 그 세션이 있을 때만 영상 트랙 하나를
//!   보낸다(음성 없음). 감독관이 닫거나 [`VIEWER_IDLE_SECS`] 동안 폴링이 끊기면 세션이 닫히고 앱은 송출을 멈춘다.
//! - 시그널링은 서버 폴링(양쪽 [`POLL_MS`]): 앱이 offer, 대시보드가 answer, 양쪽이 ICE 후보를 보낸다.
//!   서버는 메시지를 메모리에만 잠깐 쥐고(세션이 닫히면 버린다) 미디어는 서버를 거치지 않는다(TURN relay만).
//! - 녹화하지 않는다. 응시자 화면에 「감독관이 실시간으로 보는 중」을 띄운다(안내문의 실시간 열람 항목과 맞춘다).
//! - TURN 자격은 TURN REST API 방식의 시간 제한 자격이다(서버가 공유 비밀로 HMAC을 만든다, 비밀은 내보내지 않는다).

use serde::{Deserialize, Serialize};

use crate::exam::Purpose;

/// 시그널링 폴링 간격(밀리초).
pub const POLL_MS: u64 = 1000;
/// 감독관 폴링이 이만큼(초) 끊기면 세션을 닫는다(창을 닫았거나 연결이 끊겼다).
pub const VIEWER_IDLE_SECS: i64 = 20;
/// 세션 최대 길이(초). TURN 자격 수명도 같다 — 더 보려면 다시 연다.
pub const MAX_SESSION_SECS: i64 = 60 * 60;
/// 시그널링 메시지 한 개의 최대 크기(바이트). SDP 한 장이 넉넉히 들어간다.
pub const MAX_SIGNAL_BYTES: usize = 16 * 1024;
/// 한 세션에서 한쪽이 보낼 수 있는 메시지 수.
pub const MAX_SIGNALS_PER_SIDE: usize = 200;

/// 이 용도에 응시 가능 시간을 적용하는가(채용만).
pub fn window_applies(purpose: Purpose) -> bool {
    purpose == Purpose::Recruiting
}

/// 채용 시험의 응시 가능 시간 설정. `PUT /orgs/{id}/exams/{exam}/window`의 본문이기도 하다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExamWindow {
    /// 감독관이 열었는가(닫으면 범위와 상관없이 시작하지 못한다).
    pub open: bool,
    /// 이 시각(유닉스 초)부터. 없으면 제한 없음.
    #[serde(default)]
    pub starts_at: Option<i64>,
    /// 이 시각(유닉스 초) 전까지. 없으면 제한 없음.
    #[serde(default)]
    pub ends_at: Option<i64>,
}

impl Default for ExamWindow {
    fn default() -> Self {
        Self {
            open: true,
            starts_at: None,
            ends_at: None,
        }
    }
}

impl ExamWindow {
    /// 지금(`now`, 유닉스 초) 문항을 시작할 수 있는가.
    pub fn allows(&self, now: i64) -> bool {
        self.open && self.starts_at.is_none_or(|s| now >= s) && self.ends_at.is_none_or(|e| now < e)
    }

    /// 범위가 말이 되는가(시작 < 끝).
    pub fn validate(&self) -> Result<(), String> {
        match (self.starts_at, self.ends_at) {
            (Some(s), Some(e)) if s >= e => {
                Err("응시 가능 시간의 끝이 시작보다 늦어야 한다".into())
            }
            _ => Ok(()),
        }
    }
}

/// 응시 가능 시간과 지금 상태. `GET`·`PUT /orgs/{id}/exams/{exam}/window`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExamWindowView {
    #[serde(flatten)]
    pub window: ExamWindow,
    /// 이 시험에 적용되는가(채용만).
    pub applies: bool,
    /// 지금 문항을 시작할 수 있는가(적용되지 않으면 늘 참).
    pub open_now: bool,
    /// 서버 시각(유닉스 초).
    pub now: i64,
}

impl ExamWindowView {
    pub fn new(purpose: Purpose, window: ExamWindow, now: i64) -> Self {
        let applies = window_applies(purpose);
        Self {
            window,
            applies,
            open_now: !applies || window.allows(now),
            now,
        }
    }
}

/// WebRTC `RTCIceServer` 하나(TURN 시간 제한 자격 포함).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IceServer {
    pub urls: Vec<String>,
    /// `<만료 유닉스 초>:<꼬리표>`.
    pub username: String,
    /// base64(HMAC-SHA1(공유 비밀, username)).
    pub credential: String,
}

/// 실시간 보기 세션 하나(양쪽이 받는 모양은 같고 자격만 다르다).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveSession {
    pub session_id: String,
    pub ice_servers: Vec<IceServer>,
    /// 세션이 저절로 닫히는 시각(유닉스 초).
    pub expires_at: i64,
}

/// `GET /assignments/{id}/live` — 앱이 폴링한다. 감독관이 열었을 때만 세션이 있다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveRequest {
    pub session: Option<LiveSession>,
}

/// 시그널링 메시지 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    /// SDP offer(앱 → 감독관).
    Offer,
    /// SDP answer(감독관 → 앱).
    Answer,
    /// ICE 후보(양쪽).
    Ice,
    /// 끝(양쪽). 서버가 세션을 닫을 때도 상대에게 보낸다.
    Bye,
}

impl SignalKind {
    /// 앱(송출하는 쪽)이 보낼 수 있는가.
    pub fn from_candidate(self) -> bool {
        matches!(self, SignalKind::Offer | SignalKind::Ice | SignalKind::Bye)
    }
    /// 감독관이 보낼 수 있는가.
    pub fn from_viewer(self) -> bool {
        matches!(self, SignalKind::Answer | SignalKind::Ice | SignalKind::Bye)
    }
}

/// `POST …/live/{session}/signals` 본문.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostSignal {
    pub kind: SignalKind,
    /// SDP나 ICE 후보 JSON(서버는 내용을 보지 않고 크기만 막는다).
    #[serde(default)]
    pub data: String,
}

/// 받은 시그널링 메시지.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signal {
    /// 이 방향에서의 순번(1부터). 다음 폴링은 `?after=<마지막 seq>`.
    pub seq: u64,
    pub kind: SignalKind,
    pub data: String,
}

/// `GET …/live/{session}/signals?after=N`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignalBatch {
    pub signals: Vec<Signal>,
    /// 세션이 아직 열려 있는가. 거짓이면 송출·연결을 멈춘다.
    pub open: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 기본은_열림이고_범위가_시작과_끝을_가른다() {
        assert!(ExamWindow::default().allows(0));
        let w = ExamWindow {
            open: true,
            starts_at: Some(100),
            ends_at: Some(200),
        };
        assert!(!w.allows(99));
        assert!(w.allows(100));
        assert!(w.allows(199));
        assert!(!w.allows(200));
        assert!(!ExamWindow { open: false, ..w }.allows(150));
        assert!(
            ExamWindow {
                starts_at: Some(200),
                ..w
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn 사내_진단에는_적용하지_않는다() {
        let closed = ExamWindow {
            open: false,
            ..ExamWindow::default()
        };
        assert!(ExamWindowView::new(Purpose::Internal, closed, 0).open_now);
        assert!(!ExamWindowView::new(Purpose::Recruiting, closed, 0).open_now);
    }

    #[test]
    fn 방향마다_보낼_수_있는_종류가_다르다() {
        assert!(SignalKind::Offer.from_candidate() && !SignalKind::Offer.from_viewer());
        assert!(SignalKind::Answer.from_viewer() && !SignalKind::Answer.from_candidate());
        assert!(SignalKind::Ice.from_viewer() && SignalKind::Ice.from_candidate());
    }
}
