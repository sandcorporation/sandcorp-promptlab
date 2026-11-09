//! TCP → 유닉스 소켓 중계. 요청마다 [`crate::rules`]로 거르고, 통과한 것만 소켓에 새 연결로 넘긴다.
//!
//! - 본문: 만들기만 읽어 검사한다([`rules::MAX_BODY`]). 다른 호출은 본문이 있으면 거부.
//! - attach: 양쪽 HTTP 업그레이드(101) 뒤 바이트를 양방향으로 복사한다 — 한쪽 쓰기 닫힘(stdin 끝)을 반대쪽에 그대로
//!   전한다(러너가 stdin EOF로 입력 끝을 안다).
//! - 만들기 응답(201)의 `Id`와 질의의 이름을 [`Owned`]에 적고, 컨테이너 대상 호출은 거기 있는 것만 넘긴다.
//! - 로그: 거부마다 한 줄(메서드·경로·까닭). 본문(코드·케이스가 지나간다)은 남기지 않는다.

use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full, Limited};
use hyper::body::Incoming;
use hyper::header::{self, HeaderMap, HeaderValue};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, UnixStream};

use crate::rules::{self, Deny, Route};

/// 응답 본문.
pub type Body = BoxBody<Bytes, hyper::Error>;

/// 이 프록시로 만든 컨테이너(이름·id). 판정 한 번(만들기 지연 + 벽시계 최대 60초 + kill)보다 넉넉히 둔 뒤 잊는다.
pub struct Owned {
    ttl: Duration,
    refs: Mutex<HashMap<String, Instant>>,
}

/// [`Owned`] 기본 수명.
pub const OWNED_TTL: Duration = Duration::from_secs(15 * 60);

impl Owned {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            refs: Mutex::new(HashMap::new()),
        }
    }

    pub fn add(&self, name: &str, id: &str) {
        let now = Instant::now();
        let mut m = self.refs.lock().expect("owned");
        m.retain(|_, t| now.duration_since(*t) < self.ttl);
        m.insert(name.to_owned(), now);
        m.insert(id.to_owned(), now);
    }

    pub fn owns(&self, r: &str) -> bool {
        let m = self.refs.lock().expect("owned");
        m.get(r).is_some_and(|t| t.elapsed() < self.ttl)
    }
}

/// 프록시 상태.
pub struct Proxy {
    socket: PathBuf,
    owned: Owned,
}

impl Proxy {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
            owned: Owned::new(OWNED_TTL),
        }
    }

    pub fn owned(&self) -> &Owned {
        &self.owned
    }
}

/// 받기 고리. 연결마다 작업 하나.
pub async fn serve(listener: TcpListener, proxy: Arc<Proxy>) -> std::io::Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let proxy = proxy.clone();
        tokio::spawn(async move {
            let svc = hyper::service::service_fn(move |req| {
                let proxy = proxy.clone();
                async move { Ok::<_, Infallible>(handle(&proxy, req).await) }
            });
            if let Err(e) = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), svc)
                .with_upgrades()
                .await
            {
                tracing::debug!(error = %e, "연결 끝");
            }
        });
    }
}

fn full(status: StatusCode, body: Bytes) -> Response<Body> {
    let mut r = Response::new(Full::new(body).map_err(|n| match n {}).boxed());
    *r.status_mut() = status;
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    r
}

fn message(status: StatusCode, msg: &str) -> Response<Body> {
    let b = serde_json::to_vec(&serde_json::json!({ "message": msg })).expect("json");
    full(status, Bytes::from(b))
}

fn refuse(method: &str, path: &str, d: &Deny) -> Response<Body> {
    tracing::warn!(method, path, reason = %d, "거부");
    message(
        StatusCode::FORBIDDEN,
        &format!("promptlab 소켓 프록시: {d}"),
    )
}

fn bad_gateway(method: &str, path: &str, e: &dyn std::fmt::Display) -> Response<Body> {
    tracing::error!(method, path, error = %e, "docker 소켓에 넘기지 못했다");
    message(
        StatusCode::BAD_GATEWAY,
        "promptlab 소켓 프록시: docker 소켓 오류",
    )
}

/// 넘기지 않는 머리(연결 단위). attach는 업그레이드 머리를 다시 붙인다.
const HOP: [header::HeaderName; 7] = [
    header::CONNECTION,
    header::UPGRADE,
    header::TRANSFER_ENCODING,
    header::TE,
    header::TRAILER,
    header::PROXY_AUTHORIZATION,
    header::HOST,
];

fn copy_headers(from: &HeaderMap, to: &mut HeaderMap) {
    for (k, v) in from {
        if HOP.contains(k) || k == header::CONTENT_LENGTH || k.as_str() == "keep-alive" {
            continue;
        }
        to.append(k.clone(), v.clone());
    }
}

/// 요청 하나.
pub async fn handle(proxy: &Proxy, mut req: Request<Incoming>) -> Response<Body> {
    let method = req.method().as_str().to_owned();
    let path = req.uri().path().to_owned();
    let route = match rules::classify(&method, &path) {
        Ok(r) => r,
        Err(d) => return refuse(&method, &path, &d),
    };
    if let Some(c) = route.container()
        && !proxy.owned.owns(c)
    {
        return refuse(
            &method,
            &path,
            &Deny("이 프록시가 만든 컨테이너가 아니다".into()),
        );
    }
    if req.headers().contains_key(header::CONTENT_ENCODING) {
        return refuse(
            &method,
            &path,
            &Deny("Content-Encoding은 받지 않는다".into()),
        );
    }
    let wants_upgrade = req.headers().contains_key(header::UPGRADE);
    if wants_upgrade && !route.is_attach() {
        return refuse(&method, &path, &Deny("업그레이드는 attach만".into()));
    }
    let client_upgrade = route.is_attach().then(|| hyper::upgrade::on(&mut req));

    let (parts, body) = req.into_parts();
    let body = match Limited::new(body, rules::MAX_BODY).collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => {
            return refuse(
                &method,
                &path,
                &Deny("본문이 너무 크거나 읽지 못했다".into()),
            );
        }
    };
    let created_name = if route == Route::Create {
        let ct = parts
            .headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok());
        match rules::check_create(parts.uri.query(), ct, &body) {
            Ok(name) => Some(name),
            Err(d) => return refuse(&method, &path, &d),
        }
    } else {
        if !body.is_empty() {
            return refuse(&method, &path, &Deny("본문은 만들기에만 받는다".into()));
        }
        None
    };

    // 넘길 요청 — 경로·질의는 받은 그대로(판 접두 포함), 머리는 연결 단위를 뺀다.
    let mut out = Request::builder()
        .method(parts.method.clone())
        .uri(
            parts
                .uri
                .path_and_query()
                .map(|p| p.as_str())
                .unwrap_or("/"),
        )
        .body(Full::new(body.clone()))
        .expect("요청");
    copy_headers(&parts.headers, out.headers_mut());
    out.headers_mut()
        .insert(header::HOST, HeaderValue::from_static("docker"));
    if !body.is_empty() {
        out.headers_mut()
            .insert(header::CONTENT_LENGTH, HeaderValue::from(body.len()));
    }
    if route.is_attach() && wants_upgrade {
        out.headers_mut()
            .insert(header::CONNECTION, HeaderValue::from_static("Upgrade"));
        if let Some(u) = parts.headers.get(header::UPGRADE) {
            out.headers_mut().insert(header::UPGRADE, u.clone());
        }
    }

    let mut resp = match send(&proxy.socket, out).await {
        Ok(r) => r,
        Err(e) => return bad_gateway(&method, &path, &e),
    };

    if let Some(name) = created_name {
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = match Limited::new(resp.into_body(), rules::MAX_BODY)
            .collect()
            .await
        {
            Ok(b) => b.to_bytes(),
            Err(e) => return bad_gateway(&method, &path, &e),
        };
        if status == StatusCode::CREATED {
            match serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|v| v.get("Id").and_then(|i| i.as_str()).map(str::to_owned))
            {
                Some(id) => proxy.owned.add(&name, &id),
                None => tracing::warn!("만들기 응답에 Id가 없다"),
            }
        }
        let mut r = full(status, bytes);
        copy_headers(&headers, r.headers_mut());
        return r;
    }

    if let Some(client_upgrade) = client_upgrade
        && resp.status() == StatusCode::SWITCHING_PROTOCOLS
    {
        let upstream_upgrade = hyper::upgrade::on(&mut resp);
        tokio::spawn(async move {
            match tokio::try_join!(client_upgrade, upstream_upgrade) {
                Ok((c, u)) => {
                    let mut c = TokioIo::new(c);
                    let mut u = TokioIo::new(u);
                    if let Err(e) = tokio::io::copy_bidirectional(&mut c, &mut u).await {
                        tracing::debug!(error = %e, "attach 스트림 끝");
                    }
                }
                Err(e) => tracing::warn!(error = %e, "attach 업그레이드 실패"),
            }
        });
        let (p, _) = resp.into_parts();
        // 101에는 업그레이드 머리(Connection·Upgrade)가 있어야 한다 — docker가 준 그대로 둔다.
        return Response::from_parts(p, Empty::new().map_err(|n| match n {}).boxed());
    }

    let (mut p, b) = resp.into_parts();
    for h in [
        header::CONNECTION,
        header::TRANSFER_ENCODING,
        header::UPGRADE,
    ] {
        p.headers.remove(h);
    }
    p.headers.remove("keep-alive");
    Response::from_parts(p, b.boxed())
}

/// 유닉스 소켓에 새 연결로 요청 하나.
pub async fn send<B>(socket: &std::path::Path, req: Request<B>) -> Result<Response<Incoming>, Error>
where
    B: hyper::body::Body + Send + 'static,
    B::Data: Send,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let stream = UnixStream::connect(socket).await.map_err(Error::Connect)?;
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(Error::Http)?;
    tokio::spawn(async move {
        if let Err(e) = conn.with_upgrades().await {
            tracing::debug!(error = %e, "docker 연결 끝");
        }
    });
    sender.send_request(req).await.map_err(Error::Http)
}

/// 소켓 쪽 오류.
#[derive(Debug)]
pub enum Error {
    Connect(std::io::Error),
    Http(hyper::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Connect(e) => write!(f, "소켓 연결: {e}"),
            Error::Http(e) => write!(f, "HTTP: {e}"),
        }
    }
}

impl std::error::Error for Error {}
