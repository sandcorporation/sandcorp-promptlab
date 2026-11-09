//! 과정 로그 앱 쪽(#9): 오프라인 중 쌓인 사건이 재연결 뒤 순서대로 올라가고, 앱을 다시 켜도 이어지며,
//! 다시 설치하면 새 키로 서버 머리 위에 잇는다. 서버는 같은 검증 규칙(`shared::log`)을 쓰는 가짜다.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use promptlab_app::client::{ApiClient, ClientError, Config};
use promptlab_app::proclog::{FlushError, LogBook, ProcessLog, flush, load_or_create_signer};
use promptlab_shared::api::{ErrorResponse, LogHead, RegisterLogKeyRequest, UploadLogRequest};
use promptlab_shared::coding::Language;
use promptlab_shared::log::{
    Attempt, Chain, EventBody, LogEvent, LogSigner, SignedEvent, code_sha256, open,
};

const ASSIGNMENT: &str = "0b5c1e4e-0000-4000-8000-000000000001";

fn attempt() -> Attempt {
    Attempt {
        assignment_id: ASSIGNMENT.into(),
        position: 0,
    }
}

/// 받은 사건을 서버처럼 검증해 쌓는 가짜 서버.
#[derive(Default)]
struct FakeServer {
    keys: Vec<String>,
    events: Vec<SignedEvent>,
    chain: Chain,
    uploads: usize,
}

type Shared = Arc<Mutex<FakeServer>>;

async fn register(State(s): State<Shared>, Json(r): Json<RegisterLogKeyRequest>) -> Json<LogHead> {
    let mut s = s.lock().unwrap();
    if !s.keys.contains(&r.public_key) {
        s.keys.push(r.public_key);
    }
    Json(s.chain.clone())
}

async fn upload(
    State(s): State<Shared>,
    Json(r): Json<UploadLogRequest>,
) -> Result<Json<LogHead>, (StatusCode, Json<ErrorResponse>)> {
    let mut s = s.lock().unwrap();
    s.uploads += 1;
    let reject = |status: StatusCode, e: String| (status, Json(ErrorResponse { error: e }));
    let mut chain = s.chain.clone();
    let mut fresh = Vec::new();
    for e in &r.events {
        let opened = open(e, &attempt(), &s.keys)
            .map_err(|err| reject(StatusCode::UNPROCESSABLE_ENTITY, err.to_string()))?;
        let seq = opened.body.seq as usize;
        if (opened.body.seq) < chain.next_seq && fresh.is_empty() {
            if s.events[seq] == *e {
                continue;
            }
            return Err(reject(StatusCode::CONFLICT, "다른 내용".into()));
        }
        chain
            .append(&opened)
            .map_err(|err| reject(StatusCode::CONFLICT, err.to_string()))?;
        fresh.push(e.clone());
    }
    s.events.extend(fresh);
    s.chain = chain.clone();
    Ok(Json(chain))
}

/// 주소 하나를 잡아 두고(아직 아무도 듣지 않는다 = 오프라인) 나중에 그 주소로 가짜 서버를 띄운다.
async fn reserve_addr() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap()
}

async fn serve_at(addr: std::net::SocketAddr, server: Shared) {
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let router = Router::new()
        .route(
            &format!("/assignments/{ASSIGNMENT}/items/0/log/keys"),
            post(register),
        )
        .route(
            &format!("/assignments/{ASSIGNMENT}/items/0/log"),
            post(upload),
        )
        .with_state(server);
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
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

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "promptlab-proclog-{name}-{}-{}",
        std::process::id(),
        promptlab_app::proclog::now_ms()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn kinds(events: &[SignedEvent]) -> Vec<(u64, LogEvent)> {
    events
        .iter()
        .map(|e| {
            let b: EventBody = serde_json::from_str(&e.body).unwrap();
            (b.seq, b.event)
        })
        .collect()
}

fn submitted() -> LogEvent {
    LogEvent::Submitted {
        language: Language::Python,
        code_sha256: code_sha256("def f():\n    pass\n"),
    }
}

fn edited(text: &str) -> LogEvent {
    LogEvent::AiMessage { text: text.into() }
}

#[tokio::test]
async fn 오프라인_중_쌓인_사건이_재연결_뒤_순서대로_올라가고_앱을_다시_켜도_이어진다() {
    let dir = temp_dir("offline");
    let addr = reserve_addr().await;
    let api = api(addr);

    let book = LogBook::open(&dir).unwrap();
    let log = book.attempt(attempt()).unwrap();
    for (i, event) in [LogEvent::Started, edited("하나"), edited("둘")]
        .into_iter()
        .enumerate()
    {
        log.lock().unwrap().record(1_000 + i as i64, event).unwrap();
    }
    // 서버가 없다(오프라인): 올리지 못하고 쌓인 채로 남는다.
    match book.flush_all(&api).await {
        Err(FlushError::Client(ClientError::Network(_))) => {}
        other => panic!("네트워크 오류여야 한다: {other:?}"),
    }
    assert_eq!(book.pending(), 3);

    // 앱을 껐다 켠다: 못 올린 로그를 디스크에서 다시 모으고, 같은 문항을 열면 이어서 쌓는다.
    drop((log, book));
    let book = LogBook::open(&dir).unwrap();
    assert_eq!(book.pending(), 3);
    let log = book.attempt(attempt()).unwrap();
    assert!(log.lock().unwrap().has_history());
    log.lock().unwrap().record(2_000, submitted()).unwrap();
    assert_eq!(book.pending(), 4, "같은 문항은 같은 로그다");

    // 다시 연결됐다.
    let server: Shared = Arc::default();
    serve_at(addr, server.clone()).await;
    assert_eq!(book.flush_all(&api).await.unwrap(), 4);
    assert_eq!(book.pending(), 0);
    {
        let s = server.lock().unwrap();
        assert_eq!(
            kinds(&s.events),
            vec![
                (0, LogEvent::Started),
                (1, edited("하나")),
                (2, edited("둘")),
                (3, submitted()),
            ]
        );
        assert_eq!(s.keys.len(), 1);
    }

    // 더 올릴 것이 없으면 업로드하지 않는다.
    let uploads = server.lock().unwrap().uploads;
    assert_eq!(book.flush_all(&api).await.unwrap(), 0);
    assert_eq!(server.lock().unwrap().uploads, uploads);
    log.lock().unwrap().record(2_500, edited("셋")).unwrap();
    // 그 뒤 새 사건은 이어진 순번으로 간다.
    log.lock()
        .unwrap()
        .record(3_000, LogEvent::FocusLost)
        .unwrap();
    assert_eq!(flush(&log, &api).await.unwrap(), 2);
    assert_eq!(
        kinds(&server.lock().unwrap().events)[4..],
        [(4, edited("셋")), (5, LogEvent::FocusLost)]
    );
}

#[tokio::test]
async fn 다시_설치하면_새_키로_서버_머리_위에_잇는다() {
    let addr = reserve_addr().await;
    let api = api(addr);
    let server: Shared = Arc::default();
    serve_at(addr, server.clone()).await;

    // 옛 설치가 둘을 올렸다.
    let old_dir = temp_dir("old");
    let old = Mutex::new(
        ProcessLog::open(
            &old_dir,
            Arc::new(load_or_create_signer(&old_dir).unwrap()),
            attempt(),
        )
        .unwrap(),
    );
    old.lock().unwrap().record(1, LogEvent::Started).unwrap();
    old.lock().unwrap().record(2, edited("옛")).unwrap();
    assert_eq!(flush(&old, &api).await.unwrap(), 2);

    // 새 설치: 키도 로컬 로그도 없다. 처음부터(순번 0) 쌓았다가 올리면 서버 머리 위로 다시 잇는다.
    let new_dir = temp_dir("new");
    let new = Mutex::new(
        ProcessLog::open(
            &new_dir,
            Arc::new(load_or_create_signer(&new_dir).unwrap()),
            attempt(),
        )
        .unwrap(),
    );
    assert!(!new.lock().unwrap().has_history());
    new.lock().unwrap().record(3, LogEvent::Started).unwrap();
    new.lock().unwrap().record(4, edited("새")).unwrap();
    assert_eq!(flush(&new, &api).await.unwrap(), 2);

    let s = server.lock().unwrap();
    assert_eq!(
        kinds(&s.events),
        vec![
            (0, LogEvent::Started),
            (1, edited("옛")),
            (2, LogEvent::Started),
            (3, edited("새")),
        ]
    );
    assert_eq!(s.keys.len(), 2, "키가 둘 등록됐다");
}

#[test]
fn 설치_키는_다시_읽어도_같고_깨졌으면_새로_만든다() {
    let dir = temp_dir("key");
    let first = load_or_create_signer(&dir).unwrap().public_key();
    assert_eq!(load_or_create_signer(&dir).unwrap().public_key(), first);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join("device-key.ed25519"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "다른 사용자가 읽을 수 없어야 한다");
    }
    std::fs::write(dir.join("device-key.ed25519"), "깨진 키").unwrap();
    let again = load_or_create_signer(&dir).unwrap().public_key();
    assert_ne!(again, first);
    assert!(dir.join("device-key.ed25519.broken").exists());
    // 다른 키는 쓰지 않는 서명 키다.
    assert_ne!(again, LogSigner::from_seed([0; 32]).public_key());
}
