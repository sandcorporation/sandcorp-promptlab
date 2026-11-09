//! sandcorp-auth 액세스 토큰 검증. 규칙의 정본은 sandcorp-auth `docs/resource-server-token-validation.md`이고,
//! 이 파일은 그 규칙을 Rust로 옮긴 것이다(파이썬 `sandcorp-authkit`을 쓸 수 없어서). 규칙 이름(T1·A2·J3 …)을
//! 그대로 쓰고, `tests/token_validation.rs`가 규칙마다 통과·거부를 확인한다.
//!
//! 일반 사용자(member)를 받는 사이트이므로 3절의 T1~T10, T11 대신 10절의 A1~A3, 그다음 S1을 순서대로 본다.
//! 기계 토큰(`acct=machine`, 12절)은 A1~A3 대신 M1~M3으로 본다 — 허용 목록([`TokenRules::machine_client_ids`])이
//! 비면 기계 토큰은 전부 거부된다.
//! 토큰 받기(B1·B2)는 [`bearer_token`]. JWKS 캐시는 4절(J1~J4).

use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::{Map, Value};
use tokio::sync::Mutex;

/// 4절 고정 값.
pub const JWKS_TTL_SECS: i64 = 600;
pub const UNKNOWN_KID_REFETCH_SECS: i64 = 60;
pub const FETCH_FAILURE_BACKOFF_SECS: i64 = 60;
pub const EXP_LEEWAY_SECS: i64 = 30;

/// 지금 시각(유닉스 초). 시험은 고정 시계를 넣는다.
pub trait Clock: Send + Sync {
    fn now(&self) -> i64;
}

/// 시스템 시계.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
            .unwrap_or(0)
    }
}

/// JWKS 조회 실패(네트워크·타임아웃·2xx 아님). 본문 JSON이 깨진 것도 실패로 친다(J4).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("JWKS 조회 실패: {0}")]
pub struct FetchError(pub String);

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// JWKS 문서(JSON 본문 그대로)를 받아 온다.
pub trait JwksFetcher: Send + Sync {
    fn fetch(&self) -> BoxFuture<'_, Result<String, FetchError>>;
}

/// HTTP로 JWKS를 받는다(타임아웃 5초, 2xx만).
pub struct HttpJwksFetcher {
    http: reqwest::Client,
    url: String,
}

impl HttpJwksFetcher {
    pub fn new(url: impl Into<String>) -> Result<Self, reqwest::Error> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()?,
            url: url.into(),
        })
    }
}

impl JwksFetcher for HttpJwksFetcher {
    fn fetch(&self) -> BoxFuture<'_, Result<String, FetchError>> {
        Box::pin(async move {
            let response = self
                .http
                .get(&self.url)
                .send()
                .await
                .map_err(|e| FetchError(e.to_string()))?;
            if !response.status().is_success() {
                return Err(FetchError(format!("상태 {}", response.status())));
            }
            response.text().await.map_err(|e| FetchError(e.to_string()))
        })
    }
}

/// 검증 설정(1절 값 + 이 사이트의 값).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRules {
    /// `iss`와 정확히 같아야 한다(T8).
    pub issuer: String,
    /// 이 사이트의 리소스 URL. `aud`와 정확히 같아야 한다(T9).
    pub audience: String,
    /// 대표 계정 username(A1).
    pub owner_sub: String,
    /// 받기로 한 기계 클라이언트 `client_id`(M3, 12절). 비면 기계 토큰을 받지 않는다.
    pub machine_client_ids: BTreeSet<String>,
}

/// `PROMPTLAB_SMOKE_CLIENT_IDS` 꼴(쉼표 구분, 앞뒤 공백·빈 조각 무시)을 허용 목록으로.
pub fn parse_client_ids(raw: &str) -> BTreeSet<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// 기계 토큰의 `sub` 머리(12절 M2: `sub` = `"client:"` + `client_id`).
pub const MACHINE_SUB_PREFIX: &str = "client:";

/// 계정 종류(10절).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountKind {
    /// 대표(`acct` 없음, `sub`=`owner_sub`).
    Owner,
    /// 일반 사용자(`acct=member`, `sub`=`m_`+ULID).
    Member,
    /// 기계 클라이언트(`acct=machine`, `sub`=`client:`+`client_id`, 12절). promptlab에서는 「스모크」 조직의
    /// 응시자로만 행동한다(`crate::auth::machine_may`).
    Machine,
}

/// 검증을 통과한 토큰의 주인.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedToken {
    pub sub: String,
    pub kind: AccountKind,
    pub client_id: String,
}

/// 거부. 응답에는 규칙을 싣지 않고 서버 로그에만 남긴다(5절).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    /// 401 `invalid_token`. 값은 어긴 규칙 이름(`"T5"`, `"A2"` …).
    InvalidToken(&'static str),
    /// 403 `insufficient_scope`(S1).
    InsufficientScope,
}

#[derive(Default)]
struct CacheState {
    keys: HashMap<String, VerifyingKey>,
    last_success: Option<i64>,
    last_failure: Option<i64>,
    last_unknown_kid_refetch: Option<i64>,
}

/// 프로세스에 하나 두는 검증기(J1: `kid → 공개 키` 캐시 하나).
pub struct TokenValidator {
    rules: TokenRules,
    fetcher: Arc<dyn JwksFetcher>,
    clock: Arc<dyn Clock>,
    cache: Mutex<CacheState>,
}

fn b64(part: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(part).ok()
}

fn json_object(bytes: &[u8]) -> Option<Map<String, Value>> {
    match serde_json::from_slice(bytes).ok()? {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

/// A3: `^m_[0-7][0-9A-HJKMNP-TV-Z]{25}$`(Crockford base32 ULID, 대소문자 구분).
pub fn is_member_sub(sub: &str) -> bool {
    let Some(ulid) = sub.strip_prefix("m_") else {
        return false;
    };
    let bytes = ulid.as_bytes();
    bytes.len() == 26
        && (b'0'..=b'7').contains(&bytes[0])
        && bytes[1..].iter().all(|&c| {
            c.is_ascii_digit()
                || (c.is_ascii_uppercase() && !matches!(c, b'I' | b'L' | b'O' | b'U'))
        })
}

/// S1: 공백 한 칸으로 나눠 빈 조각을 버린 집합에 원소로 있는가.
pub fn has_scope(scope: &str, required: &str) -> bool {
    scope
        .split(' ')
        .filter(|s| !s.is_empty())
        .any(|s| s == required)
}

impl TokenValidator {
    pub fn new(rules: TokenRules, fetcher: Arc<dyn JwksFetcher>, clock: Arc<dyn Clock>) -> Self {
        Self {
            rules,
            fetcher,
            clock,
            cache: Mutex::new(CacheState::default()),
        }
    }

    pub fn rules(&self) -> &TokenRules {
        &self.rules
    }

    /// 액세스 토큰을 3절·10절 순서대로 검증하고 `required_scope`(S1)를 확인한다.
    pub async fn validate(
        &self,
        token: &str,
        required_scope: &str,
    ) -> Result<VerifiedToken, Rejection> {
        let claims = self.verified_claims(token).await?;
        let invalid = Rejection::InvalidToken;

        // T6: 필수 클레임.
        const REQUIRED: [&str; 8] = [
            "iss",
            "sub",
            "aud",
            "client_id",
            "scope",
            "iat",
            "exp",
            "jti",
        ];
        if REQUIRED.iter().any(|k| !claims.contains_key(*k)) {
            return Err(invalid("T6"));
        }
        // T7: 타입. iat·exp는 정수, 나머지는 문자열(aud 배열 거부).
        let int = |k: &str| {
            claims[k]
                .as_i64()
                .filter(|_| claims[k].is_i64() || claims[k].is_u64())
        };
        let text = |k: &str| claims[k].as_str();
        let (Some(_iat), Some(exp)) = (int("iat"), int("exp")) else {
            return Err(invalid("T7"));
        };
        let (Some(iss), Some(sub), Some(aud), Some(client_id), Some(scope), Some(_jti)) = (
            text("iss"),
            text("sub"),
            text("aud"),
            text("client_id"),
            text("scope"),
            text("jti"),
        ) else {
            return Err(invalid("T7"));
        };
        // T8·T9: 문자열 완전 일치.
        if iss != self.rules.issuer {
            return Err(invalid("T8"));
        }
        if aud != self.rules.audience {
            return Err(invalid("T9"));
        }
        // T10: now > exp + 30 이면 만료. iat·nbf는 시각 비교를 하지 않는다.
        if self.clock.now() > exp.saturating_add(EXP_LEEWAY_SECS) {
            return Err(invalid("T10"));
        }
        // A1~A3(T11 대신). `acct=machine`이면 A2 대신 M1~M3(12절).
        let kind = match claims.get("acct") {
            None => {
                if sub != self.rules.owner_sub {
                    return Err(invalid("A1"));
                }
                AccountKind::Owner
            }
            Some(Value::String(acct)) if acct == "member" => {
                if !is_member_sub(sub) {
                    return Err(invalid("A3"));
                }
                AccountKind::Member
            }
            // M1: `acct`가 정확히 "machine"(대소문자 차이·다른 타입은 아래 A2로 거부된다).
            Some(Value::String(acct)) if acct == "machine" => {
                // M2: sub = "client:" + client_id.
                if sub.strip_prefix(MACHINE_SUB_PREFIX) != Some(client_id) {
                    return Err(invalid("M2"));
                }
                // M3: 이 사이트가 받기로 한 기계 클라이언트.
                if !self.rules.machine_client_ids.contains(client_id) {
                    return Err(invalid("M3"));
                }
                AccountKind::Machine
            }
            Some(_) => return Err(invalid("A2")),
        };
        // S1.
        if !has_scope(scope, required_scope) {
            return Err(Rejection::InsufficientScope);
        }
        Ok(VerifiedToken {
            sub: sub.to_owned(),
            kind,
            client_id: client_id.to_owned(),
        })
    }

    /// T1~T5를 지나 서명이 맞는 토큰의 클레임(아직 내용 검사 전).
    pub async fn verified_claims(&self, token: &str) -> Result<Map<String, Value>, Rejection> {
        let invalid = Rejection::InvalidToken;
        // T1: JWS 세 부분, 헤더 base64·JSON.
        let parts: Vec<&str> = token.split('.').collect();
        let [header_b64, payload_b64, signature_b64] = parts[..] else {
            return Err(invalid("T1"));
        };
        let header = b64(header_b64)
            .and_then(|h| json_object(&h))
            .ok_or(invalid("T1"))?;
        // T2: alg는 정확히 EdDSA.
        if header.get("alg").and_then(Value::as_str) != Some("EdDSA") {
            return Err(invalid("T2"));
        }
        // T3: kid 문자열.
        let kid = header
            .get("kid")
            .and_then(Value::as_str)
            .ok_or(invalid("T3"))?;
        // T4: JWKS 캐시에서 키.
        let key = self.key_for(kid).await.ok_or(invalid("T4"))?;
        // T5: 서명.
        let signature = b64(signature_b64)
            .and_then(|s| Signature::from_slice(&s).ok())
            .ok_or(invalid("T5"))?;
        let signing_input = format!("{header_b64}.{payload_b64}");
        key.verify_strict(signing_input.as_bytes(), &signature)
            .map_err(|_| invalid("T5"))?;
        // 서명이 맞는데 본문이 JSON 객체가 아니면 모양이 깨진 것(T1).
        b64(payload_b64)
            .and_then(|p| json_object(&p))
            .ok_or(invalid("T1"))
    }

    /// J1·J2·J4: `kid`의 공개 키. 캐시가 오래됐거나 비었으면 다시 받고, 신선한데 모르는 `kid`면
    /// 60초에 한 번만 다시 받는다. 조회가 실패하면 60초 동안 다시 시도하지 않는다.
    async fn key_for(&self, kid: &str) -> Option<VerifyingKey> {
        let mut cache = self.cache.lock().await;
        let now = self.clock.now();
        let backing_off = cache
            .last_failure
            .is_some_and(|t| now - t < FETCH_FAILURE_BACKOFF_SECS);
        let stale =
            cache.last_success.is_none_or(|t| now - t >= JWKS_TTL_SECS) || cache.keys.is_empty();
        let mut refreshed = false;
        if stale && !backing_off {
            self.refresh(&mut cache, now).await;
            refreshed = true;
        }
        if let Some(key) = cache.keys.get(kid) {
            return Some(*key);
        }
        let backing_off = cache
            .last_failure
            .is_some_and(|t| now - t < FETCH_FAILURE_BACKOFF_SECS);
        let recently_refetched = cache
            .last_unknown_kid_refetch
            .is_some_and(|t| now - t < UNKNOWN_KID_REFETCH_SECS);
        if !refreshed && !backing_off && !recently_refetched {
            cache.last_unknown_kid_refetch = Some(now);
            self.refresh(&mut cache, now).await;
        }
        cache.keys.get(kid).copied()
    }

    async fn refresh(&self, cache: &mut CacheState, now: i64) {
        match self
            .fetcher
            .fetch()
            .await
            .and_then(|body| parse_jwks(&body))
        {
            Ok(keys) => {
                // J3: 성공한 조회는 캐시를 통째로 바꾼다.
                cache.keys = keys;
                cache.last_success = Some(now);
                cache.last_failure = None;
            }
            Err(e) => {
                tracing::warn!(error = %e, "JWKS 조회 실패 — 기존 캐시를 쓴다");
                cache.last_failure = Some(now);
            }
        }
    }
}

/// J3: `kty=OKP`, `crv=Ed25519`, (`alg` 있으면 `EdDSA`), (`use` 있으면 `sig`)인 키만 쓴다.
fn parse_jwks(body: &str) -> Result<HashMap<String, VerifyingKey>, FetchError> {
    let doc: Value = serde_json::from_str(body).map_err(|e| FetchError(format!("JSON: {e}")))?;
    let keys = doc
        .get("keys")
        .and_then(Value::as_array)
        .ok_or_else(|| FetchError("keys 배열 없음".into()))?;
    let mut out = HashMap::new();
    for jwk in keys {
        let field = |k: &str| jwk.get(k).and_then(Value::as_str);
        if field("kty") != Some("OKP") || field("crv") != Some("Ed25519") {
            continue;
        }
        if jwk.get("alg").is_some() && field("alg") != Some("EdDSA") {
            continue;
        }
        if jwk.get("use").is_some() && field("use") != Some("sig") {
            continue;
        }
        let (Some(kid), Some(x)) = (field("kid"), field("x")) else {
            continue;
        };
        let Some(bytes) = b64(x).and_then(|b| <[u8; 32]>::try_from(b).ok()) else {
            continue;
        };
        if let Ok(key) = VerifyingKey::from_bytes(&bytes) {
            out.insert(kid.to_owned(), key);
        }
    }
    Ok(out)
}

/// B1·B2: `Authorization: Bearer <토큰>`(스킴 대소문자 무시). 헤더가 없거나 스킴이 다르면 `None`(토큰 없음).
pub fn bearer_token(authorization: Option<&str>) -> Option<&str> {
    let value = authorization?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}
