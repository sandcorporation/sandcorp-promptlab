//! TURN 시간 제한 자격(#14) — TURN REST API 방식(coturn `use-auth-secret`).
//!
//! 서버와 coturn이 공유 비밀 하나를 나눠 갖고, 서버가 세션마다
//! `username = "<만료 유닉스 초>:<꼬리표>"`, `credential = base64(HMAC-SHA1(비밀, username))`를 만들어 준다.
//! coturn은 같은 계산으로 확인하고 만료가 지난 username을 거부한다. 장기 비밀번호는 없고, 공유 비밀은 응답·로그에
//! 싣지 않는다(`Debug`도 가린다).
//!
//! 설정(env): `PROMPTLAB_TURN_SECRET`(없으면 실시간 감독이 꺼진다), `PROMPTLAB_TURN_URLS`(쉼표 구분, 없으면
//! 공개 주소의 호스트로 `turn:<호스트>:3478?transport=udp`·`…tcp`), `PROMPTLAB_TURN_TTL_SECS`(기본 1시간 = 세션 최대 길이).

use base64::Engine;
use hmac::{Hmac, Mac};
use promptlab_shared::live::{IceServer, MAX_SESSION_SECS};
use sha1::Sha1;

/// TURN 설정. 공유 비밀은 밖으로 꺼내는 메서드가 없다.
#[derive(Clone)]
pub struct TurnConfig {
    secret: String,
    urls: Vec<String>,
    ttl_secs: i64,
}

impl std::fmt::Debug for TurnConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurnConfig")
            .field("secret", &"<숨김>")
            .field("urls", &self.urls)
            .field("ttl_secs", &self.ttl_secs)
            .finish()
    }
}

impl TurnConfig {
    pub fn new(secret: String, urls: Vec<String>, ttl_secs: i64) -> Result<Self, String> {
        if secret.trim().len() < 16 {
            return Err("PROMPTLAB_TURN_SECRET 은 16자 이상이어야 한다".into());
        }
        if urls.is_empty()
            || urls.iter().any(|u| {
                !(u.starts_with("turn:") || u.starts_with("turns:") || u.starts_with("stun:"))
            })
        {
            return Err("PROMPTLAB_TURN_URLS 는 turn:/turns:/stun: 주소여야 한다".into());
        }
        if !(60..=24 * 60 * 60).contains(&ttl_secs) {
            return Err("PROMPTLAB_TURN_TTL_SECS 는 60~86400 이어야 한다".into());
        }
        Ok(Self {
            secret,
            urls,
            ttl_secs,
        })
    }

    /// env에서 읽는다. 비밀이 없으면 `Ok(None)`(실시간 감독 꺼짐), 값이 잘못되면 `Err`.
    pub fn from_lookup(
        lookup: impl Fn(&str) -> Option<String>,
        public_url: &str,
    ) -> Result<Option<Self>, String> {
        let Some(secret) = lookup("PROMPTLAB_TURN_SECRET") else {
            return Ok(None);
        };
        let urls = match lookup("PROMPTLAB_TURN_URLS") {
            Some(list) => list
                .split(',')
                .map(|u| u.trim().to_owned())
                .filter(|u| !u.is_empty())
                .collect(),
            None => {
                let host = url::Url::parse(public_url)
                    .ok()
                    .and_then(|u| u.host_str().map(str::to_owned))
                    .ok_or("PROMPTLAB_PUBLIC_URL 에서 TURN 호스트를 알 수 없다 — PROMPTLAB_TURN_URLS 를 준다")?;
                vec![
                    format!("turn:{host}:3478?transport=udp"),
                    format!("turn:{host}:3478?transport=tcp"),
                ]
            }
        };
        let ttl_secs = match lookup("PROMPTLAB_TURN_TTL_SECS") {
            Some(v) => v
                .trim()
                .parse()
                .map_err(|_| "PROMPTLAB_TURN_TTL_SECS 가 숫자가 아니다")?,
            None => MAX_SESSION_SECS,
        };
        Self::new(secret, urls, ttl_secs).map(Some)
    }

    /// 자격 수명(초).
    pub fn ttl_secs(&self) -> i64 {
        self.ttl_secs
    }

    /// `now`부터 수명만큼 쓰는 자격. `label`은 coturn 로그에서 세션을 가르는 꼬리표(개인정보를 넣지 않는다).
    pub fn ice_server(&self, now: i64, label: &str) -> IceServer {
        let username = format!("{}:{label}", now + self.ttl_secs);
        IceServer {
            urls: self.urls.clone(),
            credential: credential(&self.secret, &username),
            username,
        }
    }
}

/// base64(HMAC-SHA1(secret, username)) — coturn이 확인하는 것과 같은 계산.
pub fn credential(secret: &str, username: &str) -> String {
    // HMAC은 어떤 길이의 키도 받는다.
    let mut mac = Hmac::<Sha1>::new_from_slice(secret.as_bytes()).expect("HMAC 키 길이 제한 없음");
    mac.update(username.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}
