//! 과정 로그(#9): 응시 문항 하나의 시간순 사건을 해시 체인 + Ed25519 서명으로 묶는다.
//!
//! - **단위**는 시험 응시의 문항 하나(배정 id + 문항 번호, [`Attempt`])다. 체인은 문항마다 따로 이어진다.
//! - **사건 하나**([`SignedEvent`])는 서명 대상 원문 `body`(JSON 문자열 — [`EventBody`]를 직렬화한 것)와
//!   그 원문 바이트에 대한 서명 `sig`다. 서버는 받은 원문 바이트를 그대로 검증·저장한다(다시 직렬화하지 않으므로
//!   필드 순서·공백 같은 표현 차이로 서명이 깨지지 않는다).
//! - **해시 체인**: 사건의 해시는 `sha256(body 바이트)`(소문자 hex). 각 원문은 앞 사건의 해시(`prev`, 첫 사건은
//!   [`GENESIS`])와 순번(`seq`, 0부터)을 담는다. 원문에 배정 id·문항 번호가 들어 있어 다른 응시로 옮겨 붙일 수 없다.
//! - **키**: 앱 설치마다 Ed25519 키 쌍 하나([`LogSigner`]). 공개키(`key`, base64url)는 문항마다 서버에 등록하고,
//!   원문에도 실어 어느 키로 서명했는지 밝힌다. 키를 잃거나 앱을 다시 설치하면 새 키를 등록하고 서버가 아는 머리
//!   (마지막 해시·다음 순번)부터 체인을 잇는다 — 다시보기에 「키가 바뀌었다」 신호가 뜬다([`LogSignal::KeyChanged`]).
//! - **검증 정책**: 서명·체인이 맞지 않는 업로드는 **거부**하고(저장하지 않는다 — 저장된 로그는 언제나 검증된 체인이다),
//!   거부된 횟수·까닭을 **표시**한다. 끝이 비거나 키가 바뀐 것처럼 거부할 수 없는 누락은 **신호로만** 표시한다
//!   — 의심의 판단은 고객사가 한다(spec).
//!
//! 사건 종류([`LogEvent`])는 뒤 티켓(#13 웹캠 등)이 더한다(#12가 포커스·전체화면·붙여넣기를 더했다). 모르는 종류는 [`LogEvent::Unknown`]으로
//! 읽힌다(서명 검증은 원문 바이트로 하므로 종류를 몰라도 체인은 검증된다).
//!
//! 히든 테스트는 이 로그에 들 수 없다 — 앱은 공개 보기([`crate::coding::PublicCodingProblem`])만 받는다.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::coding::Language;

/// 원문 형식 판. 2 = 코딩 응시 사건(spec #93) — 판 1(프롬프팅 대화 사건)은 받지 않는다(출시 전, 호환 없음).
pub const LOG_FORMAT: u32 = 2;
/// 첫 사건의 `prev`.
pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";
/// 사건 원문 하나의 최대 바이트(코드 스냅숏 64KB + 원문 머리를 넉넉히 담는다).
pub const MAX_EVENT_BYTES: usize = 128 * 1024;
/// 업로드 한 번에 싣는 최대 사건 수. 앱은 이보다 많이 쌓였으면 나눠 올린다.
pub const MAX_EVENTS_PER_UPLOAD: usize = 100;
/// 문항 하나에 등록할 수 있는 최대 키 수(다시 설치를 몇 번 견딘다).
pub const MAX_KEYS_PER_ITEM: usize = 5;
/// 문항 하나의 최대 사건 수.
pub const MAX_EVENTS_PER_ITEM: u64 = 20_000;

/// 로그가 속한 응시 문항.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Attempt {
    /// 배정(초대) id.
    pub assignment_id: String,
    /// 문항 번호(0부터).
    pub position: u32,
}

/// 응시 중 일어난 사건 하나(판 2 — 코딩 응시, spec #93).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LogEvent {
    /// 이 설치에서 문항을 시작했다(다시 설치·키 분실 뒤 이어 쓸 때도 이것으로 시작한다).
    Started,
    /// 풀이 언어를 골랐다(바꿀 때마다).
    LanguageChosen { language: Language },
    /// 편집기 코드의 스냅숏(앱이 정한 주기·사건마다, [`MAX_SNAPSHOT_BYTES`] 이하).
    CodeSnapshot { code: String },
    /// 응시자가 AI 도우미에게 보낸 메시지.
    AiMessage { text: String },
    /// AI 도우미의 답(도구 호출 포함) — 끝까지 만든 답 하나마다. 응시자가 생성을 멈췄으면(#116 취소) 그때까지
    /// 화면에 보인 글(감춘 도구 호출 조각은 빼고)과 `stopped = true`, 도구 호출은 비고 실행하지 않았다. 멈춘 답의 글이
    /// 비면 모델 대화에 assistant 차례를 넣지 않았다(다음 응시자 메시지가 바로 이어진다).
    AiReply {
        text: String,
        #[serde(default)]
        tool_calls: Vec<ToolCall>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        stopped: bool,
    },
    /// AI가 코드 편집을 제안했다(`proposal`은 문항 안에서 겹치지 않는 번호).
    EditProposed {
        proposal: u32,
        range: EditRange,
        text: String,
    },
    /// 제안을 받아들였다.
    EditAccepted { proposal: u32 },
    /// 제안을 버렸다.
    EditRejected { proposal: u32 },
    /// 로컬 실행(직접 실행 또는 예제 실행). 예제 실행이면 통과 수.
    LocalRun {
        kind: LocalRunKind,
        passed: Option<u32>,
        total: Option<u32>,
        /// 짧은 요약(「예제 2/3 통과」·「컴파일 오류」 등).
        summary: String,
    },
    /// 붙여넣기를 막았다(편집기 밖에서 들어온 글, 글자 수만).
    PasteBlocked { chars: u32 },
    /// 이 코드로 냈다(제출 요청 직전에 남긴다 — 서버가 제출 본문과 맞춰 본다).
    Submitted {
        language: Language,
        /// 제출 코드 UTF-8 바이트의 sha256(소문자 hex, [`code_sha256`]).
        code_sha256: String,
    },
    /// 앱 창이 포커스를 잃었다(Alt+Tab 등 — 막지 못하고 기록한다, #12).
    FocusLost,
    /// 앱 창이 포커스를 되찾았다. 비운 시간(앱 시계).
    FocusReturned { away_ms: u64 },
    /// 전체화면 응시 모드에서 빠져나왔다(앱이 다시 전체화면으로 돌린다, #12).
    FullscreenExited,
    /// 웹캠(#13): 얼굴이 보이지 않는다. 그 순간 스냅샷의 참조(올린 사진의 `client_ref`).
    NoFace { snapshot: Option<String> },
    /// 웹캠: 얼굴이 둘 이상 보인다.
    MultipleFaces {
        count: u32,
        snapshot: Option<String>,
    },
    /// 웹캠: 얼굴이 화면을 보지 않는다(옆·아래).
    GazeAway { snapshot: Option<String> },
    /// 웹캠: 주기 스냅샷을 찍었다.
    Snapshot { snapshot: String },
    /// 웹캠을 쓸 수 없게 됐다(장치 분리·권한 거부 등).
    CameraUnavailable { reason: String },
    /// 이 판이 모르는 사건 종류(뒤 판의 앱이 더한 것).
    #[serde(other)]
    Unknown,
}

/// AI 도우미의 도구 호출 하나(이름 + 인자 JSON).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
}

/// 편집 범위(1부터, Monaco 규약 — 끝 열은 포함하지 않는다).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditRange {
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
}

/// 로컬 실행 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalRunKind {
    /// 응시자가 직접 넣은 입력으로 실행.
    Run,
    /// 공개 예제로 실행.
    Examples,
}

/// 코드 스냅숏 하나의 상한(바이트) — 제출 코드 상한과 같다.
pub const MAX_SNAPSHOT_BYTES: usize = crate::coding::MAX_CODE_BYTES;

/// 제출 코드의 sha256(소문자 hex) — [`LogEvent::Submitted`]의 `code_sha256`.
pub fn code_sha256(code: &str) -> String {
    event_hash(code)
}

impl LogEvent {
    /// 화면에 보이는 짧은 이름.
    pub fn label(&self) -> &'static str {
        match self {
            LogEvent::Started => "문항 시작",
            LogEvent::LanguageChosen { .. } => "언어 고름",
            LogEvent::CodeSnapshot { .. } => "코드 스냅숏",
            LogEvent::AiMessage { .. } => "AI에게 메시지",
            LogEvent::AiReply { .. } => "AI 답",
            LogEvent::EditProposed { .. } => "편집 제안",
            LogEvent::EditAccepted { .. } => "제안 수락",
            LogEvent::EditRejected { .. } => "제안 거절",
            LogEvent::LocalRun { .. } => "로컬 실행",
            LogEvent::PasteBlocked { .. } => "붙여넣기 막음",
            LogEvent::Submitted { .. } => "제출",
            LogEvent::FocusLost => "포커스 이탈",
            LogEvent::FocusReturned { .. } => "포커스 복귀",
            LogEvent::FullscreenExited => "전체화면 이탈",
            LogEvent::NoFace { .. } => "얼굴 없음",
            LogEvent::MultipleFaces { .. } => "여러 명",
            LogEvent::GazeAway { .. } => "시선 이탈",
            LogEvent::Snapshot { .. } => "주기 스냅샷",
            LogEvent::CameraUnavailable { .. } => "웹캠 끊김",
            LogEvent::Unknown => "알 수 없는 사건",
        }
    }
}

/// 서명 대상 원문. 직렬화한 JSON 문자열이 [`SignedEvent::body`]다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventBody {
    /// 형식 판([`LOG_FORMAT`]).
    pub v: u32,
    pub assignment_id: String,
    pub position: u32,
    /// 순번(0부터, 빈틈 없이).
    pub seq: u64,
    /// 앞 사건의 해시(첫 사건은 [`GENESIS`]).
    pub prev: String,
    /// 서명한 키(Ed25519 공개키, base64url).
    pub key: String,
    /// 앱 시계의 유닉스 밀리초. PC 시계가 틀릴 수 있으므로 서버는 받은 시각을 따로 남긴다.
    pub at_ms: i64,
    pub event: LogEvent,
}

/// 서명된 사건 하나 — 앱이 쌓고 서버로 올리는 단위.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedEvent {
    /// [`EventBody`]의 JSON 원문.
    pub body: String,
    /// `body` 바이트에 대한 Ed25519 서명(base64url).
    pub sig: String,
}

/// 원문의 해시(sha256, 소문자 hex). 다음 사건의 `prev`가 된다.
pub fn event_hash(body: &str) -> String {
    let digest = Sha256::digest(body.as_bytes());
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// 앱 설치 하나의 서명 키. 개인키는 밖으로 내지 않는다(`Debug`도 공개키만 보인다).
pub struct LogSigner {
    key: SigningKey,
}

impl std::fmt::Debug for LogSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogSigner")
            .field("public_key", &self.public_key())
            .finish_non_exhaustive()
    }
}

impl LogSigner {
    /// 32바이트 씨앗(무작위)에서 키를 만든다. 씨앗이 곧 개인키다.
    pub fn from_seed(seed: [u8; 32]) -> LogSigner {
        LogSigner {
            key: SigningKey::from_bytes(&seed),
        }
    }

    /// 보관용 씨앗(개인키). 파일에만 쓴다.
    pub fn seed(&self) -> [u8; 32] {
        self.key.to_bytes()
    }

    /// 공개키(base64url). 서버에 등록하고 원문에 싣는다.
    pub fn public_key(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.key.verifying_key().as_bytes())
    }

    /// 사건 하나를 체인에 이어 서명한다. `prev`는 앞 사건의 해시(첫 사건은 [`GENESIS`]).
    pub fn seal(
        &self,
        attempt: &Attempt,
        seq: u64,
        prev: &str,
        at_ms: i64,
        event: LogEvent,
    ) -> SignedEvent {
        let body = EventBody {
            v: LOG_FORMAT,
            assignment_id: attempt.assignment_id.clone(),
            position: attempt.position,
            seq,
            prev: prev.to_owned(),
            key: self.public_key(),
            at_ms,
            event,
        };
        let body = serde_json::to_string(&body).expect("EventBody는 언제나 직렬화된다");
        let sig = URL_SAFE_NO_PAD.encode(self.key.sign(body.as_bytes()).to_bytes());
        SignedEvent { body, sig }
    }
}

/// 로그를 받지 않는 까닭. 메시지는 응시자·담당자에게 보여도 된다(숨은 정보 없음).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LogError {
    #[error("사건 원문이 {MAX_EVENT_BYTES}바이트를 넘는다")]
    TooLarge,
    #[error("사건 원문을 읽지 못했다")]
    Malformed,
    #[error("모르는 로그 형식 판 {0}")]
    Version(u32),
    #[error("다른 응시 문항의 사건이다")]
    WrongAttempt,
    #[error("등록되지 않은 키로 서명했다")]
    UnknownKey,
    #[error("서명이 맞지 않는다")]
    BadSignature,
    #[error("순번 {got}이 왔는데 {expected}를 기다린다(빠진 사건)")]
    Gap { expected: u64, got: u64 },
    #[error("순번 {seq}의 앞 해시가 체인과 맞지 않는다")]
    BrokenChain { seq: u64 },
    #[error("순번 {seq}은 이미 다른 내용으로 받았다")]
    Conflict { seq: u64 },
}

/// 서명까지 확인한 사건.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    pub body: EventBody,
    /// 이 사건의 해시([`event_hash`]).
    pub hash: String,
}

/// 원문을 읽고 응시 문항·키·서명을 확인한다(체인 위치는 [`Chain`]이 본다). `keys`는 이 문항에 등록된 공개키.
pub fn open(event: &SignedEvent, attempt: &Attempt, keys: &[String]) -> Result<Opened, LogError> {
    if event.body.len() > MAX_EVENT_BYTES {
        return Err(LogError::TooLarge);
    }
    let body: EventBody = serde_json::from_str(&event.body).map_err(|_| LogError::Malformed)?;
    if body.v != LOG_FORMAT {
        return Err(LogError::Version(body.v));
    }
    if body.assignment_id != attempt.assignment_id || body.position != attempt.position {
        return Err(LogError::WrongAttempt);
    }
    if !keys.contains(&body.key) {
        return Err(LogError::UnknownKey);
    }
    let key: [u8; 32] = URL_SAFE_NO_PAD
        .decode(&body.key)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or(LogError::UnknownKey)?;
    let key = VerifyingKey::from_bytes(&key).map_err(|_| LogError::UnknownKey)?;
    let sig: [u8; 64] = URL_SAFE_NO_PAD
        .decode(&event.sig)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or(LogError::BadSignature)?;
    key.verify(event.body.as_bytes(), &Signature::from_bytes(&sig))
        .map_err(|_| LogError::BadSignature)?;
    let hash = event_hash(&event.body);
    Ok(Opened { body, hash })
}

/// 공개키가 Ed25519 공개키 모양(base64url 32바이트, 곡선 위의 점)인가.
pub fn is_valid_public_key(key: &str) -> bool {
    URL_SAFE_NO_PAD
        .decode(key)
        .ok()
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
        .is_some_and(|b| VerifyingKey::from_bytes(&b).is_ok())
}

/// 체인의 머리: 다음에 올 순번과 마지막 해시.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chain {
    pub next_seq: u64,
    pub head: String,
}

impl Default for Chain {
    fn default() -> Self {
        Chain {
            next_seq: 0,
            head: GENESIS.to_owned(),
        }
    }
}

impl Chain {
    /// 서명을 확인한 사건이 이 머리 바로 뒤에 붙는지 보고 머리를 옮긴다.
    pub fn append(&mut self, opened: &Opened) -> Result<(), LogError> {
        let seq = opened.body.seq;
        if seq != self.next_seq {
            return Err(LogError::Gap {
                expected: self.next_seq,
                got: seq,
            });
        }
        if opened.body.prev != self.head {
            return Err(LogError::BrokenChain { seq });
        }
        self.next_seq += 1;
        self.head = opened.hash.clone();
        Ok(())
    }
}

/// 다시보기에 붙는 신호. 판단은 고객사가 한다 — 신호는 「살펴볼 것」일 뿐이다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LogSignal {
    /// 순번 `seq`부터 다른 키(다른 설치)로 서명했다 — 앱을 다시 설치했거나 키를 잃었다. 그 앞에서 올리지 못한
    /// 사건은 빠졌을 수 있다.
    KeyChanged { seq: u64 },
    /// 서명·체인이 맞지 않아 거부한 업로드가 있다.
    Rejected {
        count: u32,
        last_reason: String,
        last_at: i64,
    },
    /// 제출했는데 제출 사건이 아직 오지 않았다(로그가 끝까지 올라오지 않았다 — 오프라인으로 끝났을 수 있다).
    Incomplete,
    /// 저장된 체인을 다시 검증하니 순번 `seq`에서 맞지 않는다(저장 뒤 바뀌었다).
    Tampered { seq: u64 },
}

impl LogSignal {
    /// 화면에 보이는 설명.
    pub fn describe(&self) -> String {
        match self {
            LogSignal::KeyChanged { seq } => format!(
                "순번 {seq}부터 다른 설치의 키로 서명했다(앱 재설치·키 분실). 그 앞의 올리지 못한 사건이 빠졌을 수 있다"
            ),
            LogSignal::Rejected {
                count, last_reason, ..
            } => format!("검증에 실패해 거부한 업로드 {count}회 — 마지막 까닭: {last_reason}"),
            LogSignal::Incomplete => {
                "제출했지만 제출 사건까지 로그가 올라오지 않았다(오프라인으로 끝났을 수 있다)"
                    .to_owned()
            }
            LogSignal::Tampered { seq } => {
                format!("저장된 로그가 순번 {seq}에서 검증되지 않는다(저장 뒤 변조 의심)")
            }
        }
    }
}
