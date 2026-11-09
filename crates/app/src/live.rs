//! 실시간 감독 송출(#14): 감독관이 대시보드에서 「실시간 보기」를 열었을 때만 웹캠 영상 트랙을 WebRTC로 보낸다.
//! 규칙은 [`promptlab_shared::live`].
//!
//! [`LiveLink::tick`]을 주기적으로 부르면 서버를 폴링해
//! - 송출 요청이 새로 생기면 피어를 만들고 offer를 보낸다,
//! - 감독관의 answer·ICE 후보를 피어에 넣고 내 ICE 후보를 보낸다,
//! - 요청이 사라지거나(감독관이 닫음·연결 끊김) 세션이 닫히면 피어를 닫아 송출을 멈춘다.
//!
//! 실제 WebRTC는 웹뷰 JS가 하고(화면 쪽 `JsPeer`), 시험은 가짜 피어를 넣는다([`Peer`]).

use std::future::Future;

use promptlab_shared::live::{LiveSession, PostSignal, Signal, SignalKind};

use crate::client::{ApiClient, ClientError};

/// WebRTC 피어(송출하는 쪽). 영상 트랙만 붙인다 — 음성은 없다.
pub trait Peer {
    /// 세션의 ICE 서버로 피어를 만들고 웹캠 영상 트랙을 붙여 offer SDP를 돌려준다.
    fn start(&mut self, session: &LiveSession) -> impl Future<Output = Result<String, String>>;
    /// 감독관이 보낸 answer·ICE 후보를 넣는다.
    fn apply(&mut self, signal: &Signal) -> impl Future<Output = Result<(), String>>;
    /// 그새 모인 내 ICE 후보(JSON)를 꺼낸다.
    fn take_candidates(&mut self) -> impl Future<Output = Vec<String>>;
    /// 피어를 닫는다(송출 멈춤). 웹캠 자체는 감시 분석이 계속 쓴다.
    fn stop(&mut self) -> impl Future<Output = ()>;
}

/// 지금 송출 상태.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveStatus {
    /// 보내지 않는다.
    Idle,
    /// 감독관이 보고 있다(송출 중).
    Streaming,
}

/// 한 배정의 송출 연결.
#[derive(Debug, Default)]
pub struct LiveLink {
    session: Option<String>,
    after: u64,
}

impl LiveLink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn status(&self) -> LiveStatus {
        if self.session.is_some() {
            LiveStatus::Streaming
        } else {
            LiveStatus::Idle
        }
    }

    /// 폴링 한 번. 서버에 닿지 못하면 지금 상태를 그대로 두고 오류를 돌려준다(다음 틱에서 다시).
    pub async fn tick<P: Peer>(
        &mut self,
        api: &ApiClient,
        assignment_id: &str,
        peer: &mut P,
    ) -> Result<LiveStatus, ClientError> {
        let wanted = api.live_request(assignment_id).await?.session;
        match (&self.session, wanted) {
            (None, None) => return Ok(LiveStatus::Idle),
            (Some(_), None) => {
                self.halt(peer).await;
                return Ok(LiveStatus::Idle);
            }
            (Some(current), Some(s)) if *current == s.session_id => {}
            (_, Some(s)) => {
                if self.session.is_some() {
                    self.halt(peer).await;
                }
                match peer.start(&s).await {
                    Ok(offer) => {
                        self.session = Some(s.session_id.clone());
                        self.after = 0;
                        api.post_signal(
                            assignment_id,
                            &s.session_id,
                            &PostSignal {
                                kind: SignalKind::Offer,
                                data: offer,
                            },
                        )
                        .await?;
                    }
                    Err(why) => {
                        // 카메라를 못 쓴다 등 — 감독관 쪽에 끝을 알린다.
                        peer.stop().await;
                        let _ = api
                            .post_signal(
                                assignment_id,
                                &s.session_id,
                                &PostSignal {
                                    kind: SignalKind::Bye,
                                    data: why,
                                },
                            )
                            .await;
                        return Ok(LiveStatus::Idle);
                    }
                }
            }
        }
        let Some(session) = self.session.clone() else {
            return Ok(LiveStatus::Idle);
        };
        let batch = api
            .live_signals(assignment_id, &session, self.after)
            .await?;
        if !batch.open {
            self.halt(peer).await;
            return Ok(LiveStatus::Idle);
        }
        for signal in &batch.signals {
            self.after = self.after.max(signal.seq);
            if signal.kind == SignalKind::Bye {
                self.halt(peer).await;
                return Ok(LiveStatus::Idle);
            }
            // 잘못된 후보 하나로 송출 전체를 끊지 않는다.
            let _ = peer.apply(signal).await;
        }
        for candidate in peer.take_candidates().await {
            api.post_signal(
                assignment_id,
                &session,
                &PostSignal {
                    kind: SignalKind::Ice,
                    data: candidate,
                },
            )
            .await?;
        }
        Ok(LiveStatus::Streaming)
    }

    /// 응시가 끝났다 — 송출 중이면 감독관에게 끝을 알리고 피어를 닫는다.
    pub async fn finish<P: Peer>(&mut self, api: &ApiClient, assignment_id: &str, peer: &mut P) {
        if let Some(session) = self.session.take() {
            peer.stop().await;
            let _ = api
                .post_signal(
                    assignment_id,
                    &session,
                    &PostSignal {
                        kind: SignalKind::Bye,
                        data: String::new(),
                    },
                )
                .await;
        }
    }

    async fn halt<P: Peer>(&mut self, peer: &mut P) {
        self.session = None;
        self.after = 0;
        peer.stop().await;
    }
}
