//! 실시간 감독 송출 앱 쪽(#14): 감독관이 열 때만 피어를 만들어 offer를 보내고, answer·ICE를 넣고, 닫히면(요청 사라짐·
//! 세션 닫힘·bye) 피어를 닫아 송출을 멈춘다. 서버는 가짜(axum), 피어도 가짜(실제 WebRTC는 웹뷰 JS).

use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use promptlab_app::client::{ApiClient, Config};
use promptlab_app::live::{LiveLink, LiveStatus, Peer};
use promptlab_shared::live::{
    IceServer, LiveRequest, LiveSession, PostSignal, Signal, SignalBatch, SignalKind,
};
use serde::Deserialize;

const ASSIGNMENT: &str = "0b5c1e4e-0000-4000-8000-000000000001";

/// 감독관 역할을 하는 가짜 서버.
#[derive(Default)]
struct Fake {
    /// 열린 세션 id(감독관이 열었다).
    session: Option<String>,
    /// 세션이 열려 있는가(거짓이면 시그널 폴링에 닫힘).
    open: bool,
    /// 앱이 보낸 것.
    from_app: Vec<PostSignal>,
    /// 앱에 줄 것.
    to_app: Vec<Signal>,
}

type Shared = Arc<Mutex<Fake>>;

async fn request(State(s): State<Shared>) -> Json<LiveRequest> {
    let s = s.lock().unwrap();
    Json(LiveRequest {
        session: s.session.clone().map(|id| LiveSession {
            session_id: id,
            ice_servers: vec![IceServer {
                urls: vec!["turn:turn.example.test:3478".into()],
                username: "9999999999:c-x".into(),
                credential: "자격".into(),
            }],
            expires_at: 9_999_999_999,
        }),
    })
}

#[derive(Deserialize)]
struct After {
    after: u64,
}

async fn signals(
    State(s): State<Shared>,
    Path(_): Path<String>,
    Query(q): Query<After>,
) -> Json<SignalBatch> {
    let s = s.lock().unwrap();
    Json(SignalBatch {
        signals: s
            .to_app
            .iter()
            .filter(|m| m.seq > q.after)
            .cloned()
            .collect(),
        open: s.open,
    })
}

async fn post_signal(
    State(s): State<Shared>,
    Path(_): Path<String>,
    Json(signal): Json<PostSignal>,
) -> StatusCode {
    s.lock().unwrap().from_app.push(signal);
    StatusCode::NO_CONTENT
}

async fn serve(fake: Shared) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = Router::new()
        .route(&format!("/assignments/{ASSIGNMENT}/live"), get(request))
        .route(
            &format!("/assignments/{ASSIGNMENT}/live/{{session}}/signals"),
            get(signals).post(post_signal),
        )
        .with_state(fake);
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    addr
}

fn api(addr: std::net::SocketAddr) -> ApiClient {
    let config = Config::from_lookup(|k| match k {
        "PROMPTLAB_SERVER_URL" => Some(format!("http://{addr}")),
        "PROMPTLAB_ACCESS_TOKEN" => Some("시험-토큰".into()),
        _ => None,
    })
    .unwrap();
    ApiClient::with_browser(config, Arc::new(|_| panic!("로그인 창을 띄우면 안 된다"))).unwrap()
}

/// 가짜 피어: 무엇을 받았는지 적는다.
#[derive(Default)]
struct FakePeer {
    running: bool,
    starts: u32,
    stops: u32,
    applied: Vec<SignalKind>,
    candidates: Vec<String>,
    fail_start: bool,
}

impl Peer for FakePeer {
    async fn start(&mut self, session: &LiveSession) -> Result<String, String> {
        assert!(!session.ice_servers.is_empty(), "TURN 자격을 받는다");
        if self.fail_start {
            return Err("NotAllowedError".into());
        }
        self.running = true;
        self.starts += 1;
        self.candidates.push("{\"candidate\":\"relay\"}".into());
        Ok("v=0 가짜 offer".into())
    }
    async fn apply(&mut self, signal: &Signal) -> Result<(), String> {
        self.applied.push(signal.kind);
        Ok(())
    }
    async fn take_candidates(&mut self) -> Vec<String> {
        std::mem::take(&mut self.candidates)
    }
    async fn stop(&mut self) {
        if self.running {
            self.stops += 1;
        }
        self.running = false;
    }
}

fn kinds(fake: &Shared) -> Vec<SignalKind> {
    fake.lock()
        .unwrap()
        .from_app
        .iter()
        .map(|s| s.kind)
        .collect()
}

#[tokio::test]
async fn 감독관이_열_때만_송출하고_닫으면_멈춘다() {
    let fake: Shared = Arc::default();
    let api = api(serve(fake.clone()).await);
    let mut link = LiveLink::new();
    let mut peer = FakePeer::default();

    // 열기 전: 아무것도 하지 않는다.
    assert_eq!(
        link.tick(&api, ASSIGNMENT, &mut peer).await.unwrap(),
        LiveStatus::Idle
    );
    assert_eq!(peer.starts, 0);
    assert!(kinds(&fake).is_empty());

    // 감독관이 열었다 → 피어를 만들고 offer, 내 ICE 후보를 보낸다.
    {
        let mut f = fake.lock().unwrap();
        f.session = Some("s1".into());
        f.open = true;
    }
    assert_eq!(
        link.tick(&api, ASSIGNMENT, &mut peer).await.unwrap(),
        LiveStatus::Streaming
    );
    assert_eq!(peer.starts, 1);
    assert_eq!(kinds(&fake), vec![SignalKind::Offer, SignalKind::Ice]);

    // 감독관 answer·ICE → 피어에 넣는다(한 번씩만).
    fake.lock().unwrap().to_app = vec![
        Signal {
            seq: 1,
            kind: SignalKind::Answer,
            data: "v=0 answer".into(),
        },
        Signal {
            seq: 2,
            kind: SignalKind::Ice,
            data: "{}".into(),
        },
    ];
    link.tick(&api, ASSIGNMENT, &mut peer).await.unwrap();
    link.tick(&api, ASSIGNMENT, &mut peer).await.unwrap();
    assert_eq!(peer.applied, vec![SignalKind::Answer, SignalKind::Ice]);
    assert_eq!(peer.starts, 1, "같은 세션이면 다시 만들지 않는다");

    // 감독관이 닫았다(요청이 사라짐) → 송출을 멈춘다.
    fake.lock().unwrap().session = None;
    assert_eq!(
        link.tick(&api, ASSIGNMENT, &mut peer).await.unwrap(),
        LiveStatus::Idle
    );
    assert_eq!(peer.stops, 1);
    assert!(!peer.running);
}

#[tokio::test]
async fn 세션이_닫히거나_bye를_받으면_멈추고_응시가_끝나면_bye를_보낸다() {
    let fake: Shared = Arc::default();
    let api = api(serve(fake.clone()).await);
    let mut link = LiveLink::new();
    let mut peer = FakePeer::default();
    {
        let mut f = fake.lock().unwrap();
        f.session = Some("s1".into());
        f.open = true;
    }
    link.tick(&api, ASSIGNMENT, &mut peer).await.unwrap();
    // 시그널 폴링이 닫힘(감독관 연결 끊김으로 서버가 닫았는데 요청은 아직 보이는 사이).
    fake.lock().unwrap().open = false;
    assert_eq!(
        link.tick(&api, ASSIGNMENT, &mut peer).await.unwrap(),
        LiveStatus::Idle
    );
    assert!(!peer.running);

    // 다시 열리면 새로 만들고, bye를 받으면 멈춘다.
    {
        let mut f = fake.lock().unwrap();
        f.session = Some("s2".into());
        f.open = true;
        f.to_app = vec![Signal {
            seq: 1,
            kind: SignalKind::Bye,
            data: String::new(),
        }];
    }
    assert_eq!(
        link.tick(&api, ASSIGNMENT, &mut peer).await.unwrap(),
        LiveStatus::Idle
    );
    assert_eq!(peer.starts, 2);
    assert!(!peer.running);

    // 응시가 끝날 때 송출 중이면 bye.
    {
        let mut f = fake.lock().unwrap();
        f.session = Some("s3".into());
        f.to_app.clear();
        f.from_app.clear();
    }
    assert_eq!(
        link.tick(&api, ASSIGNMENT, &mut peer).await.unwrap(),
        LiveStatus::Streaming
    );
    link.finish(&api, ASSIGNMENT, &mut peer).await;
    assert_eq!(link.status(), LiveStatus::Idle);
    assert!(!peer.running);
    assert_eq!(kinds(&fake).last(), Some(&SignalKind::Bye));
}

#[tokio::test]
async fn 카메라를_못_쓰면_송출하지_않고_감독관에게_끝을_알린다() {
    let fake: Shared = Arc::default();
    let api = api(serve(fake.clone()).await);
    let mut link = LiveLink::new();
    let mut peer = FakePeer {
        fail_start: true,
        ..FakePeer::default()
    };
    {
        let mut f = fake.lock().unwrap();
        f.session = Some("s1".into());
        f.open = true;
    }
    assert_eq!(
        link.tick(&api, ASSIGNMENT, &mut peer).await.unwrap(),
        LiveStatus::Idle
    );
    assert_eq!(kinds(&fake), vec![SignalKind::Bye]);
}
