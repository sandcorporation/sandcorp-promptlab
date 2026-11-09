//! 잠금·무결성(#12): 응시 시작의 허용 목록, 대시보드의 의심 신호.
//!
//! ## 허용 목록
//!
//! 응시 앱은 문항을 시작할 때(`POST /assignments/{id}/items/{n}/start`) 앱 버전과 자기가 잰 모델 파일 sha256을
//! 보낸다([`ClientIntegrity`]). 서버는 [`check`]로 대조해 목록에 없으면 시작을 거부하고(시작 시각을 남기지 않는다)
//! 거부를 「모델 무결성 실패」 신호로 남긴다.
//!
//! - **정본은 이 파일의 상수다.** 모델은 [`ALLOWED_MODEL_SHA256`] = [`crate::inference::MODELS`] 후보 표의 sha256
//!   전부(#131 — 앱이 PC 사양에 맞춰 크기를 고른다. 후보를 바꾸면 `inference.rs`만 고치면 목록도 따라간다). 앱 버전은 [`ALLOWED_APP_VERSIONS`] — 배포된 서버가 받는 앱 판. 바꾸는 길은 PR → `main` 병합 →
//!   배포(코드 리뷰·이력이 남고, 운영 `.env`를 손대지 않는다). 지금 판이 목록에 있는지는 시험이 건다.
//! - **운영자 화면에서 덧씌운다(#19).** 운영자가 대시보드에서 값을 더하거나(allow) 막거나(block) 상수 기본값으로
//!   되돌린다(reset). 서버가 받는 목록 = **상수 ∪ 더한 값 − 막은 값**([`Allowlist::with_overrides`]). 바꿀 때마다
//!   누가·언제·무엇을·왜를 DB에 덧붙이기만 하는 이력으로 남긴다(PR 이력 대신). 상수는 그대로 기본값이다 — 새 판은
//!   여전히 PR로 상수에 더하고(시험이 건다), 화면은 배포 없이 급히 막거나 배포 전 판을 미리 받는 데 쓴다.
//!   모델은 **막기만** 된다([`validate_change`]): 받을 모델은 후보 표에 고정한 것뿐이다(채점은 모델 없이 판정기가 한다).
//! - **한계**: 두 값 모두 앱이 **신고**하는 값이다. 고친 앱은 거짓말을 할 수 있다 — 이 검사는 낡은 앱·다른 모델을
//!   그대로 쓰는 경우를 막는다.
//!
use serde::{Deserialize, Serialize};

use crate::inference::MODELS;
use crate::log::LogSignal;

/// 서버가 받는 앱 판. 워크스페이스 판(`Cargo.toml`)을 올리면 여기에도 더한다(시험이 건다). 낡은 판을 막으려면 뺀다.
pub const ALLOWED_APP_VERSIONS: &[&str] = &["0.1.0"];

/// 서버가 받는 모델 파일 sha256 — 후보 표([`MODELS`])의 sha256 전부(큰 것부터).
pub const ALLOWED_MODEL_SHA256: &[&str] = &{
    let mut out = [""; MODELS.len()];
    let mut i = 0;
    while i < MODELS.len() {
        out[i] = MODELS[i].sha256;
        i += 1;
    }
    out
};

/// 앱이 문항 시작 때 신고하는 값. 응시 앱은 `env!("CARGO_PKG_VERSION")`과 시작 때 잰 모델 sha256을 보낸다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientIntegrity {
    pub app_version: String,
    /// 모델 파일 sha256(소문자 16진).
    pub model_sha256: String,
}

/// 허용 목록 대조 결과(통과하지 못한 것).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntegrityFailure {
    pub app_ok: bool,
    pub model_ok: bool,
}

impl IntegrityFailure {
    /// 응시자에게 보이는 이유.
    pub fn message(&self) -> &'static str {
        match (self.app_ok, self.model_ok) {
            (false, false) => {
                "허용되지 않은 앱 판·모델이다 — 최신 응시 앱을 다시 설치하라(모델은 설치 때 다시 받는다)"
            }
            (false, true) => "허용되지 않은 앱 판이다 — 최신 응시 앱을 설치하라",
            _ => "허용되지 않은 모델 파일이다 — 응시 앱을 다시 설치해 모델을 다시 받아라",
        }
    }
}

/// 신고값을 상수 허용 목록과 대조한다. 신고가 없으면(낡은 앱) 둘 다 실패다.
/// 서버는 운영자 변경을 합친 [`Allowlist::check`]를 쓴다.
pub fn check(report: Option<&ClientIntegrity>) -> Result<(), IntegrityFailure> {
    Allowlist::constants().check(report)
}

/// 허용 목록의 칸.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllowKind {
    /// 앱 판(`0.1.0` 꼴).
    AppVersion,
    /// 모델 파일 sha256(소문자 16진 64자).
    ModelSha256,
}

impl AllowKind {
    pub const ALL: [AllowKind; 2] = [AllowKind::AppVersion, AllowKind::ModelSha256];

    pub fn as_str(self) -> &'static str {
        match self {
            AllowKind::AppVersion => "app_version",
            AllowKind::ModelSha256 => "model_sha256",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// 이 칸의 상수 기본값.
    pub fn constants(self) -> &'static [&'static str] {
        match self {
            AllowKind::AppVersion => ALLOWED_APP_VERSIONS,
            AllowKind::ModelSha256 => ALLOWED_MODEL_SHA256,
        }
    }
}

/// 운영자의 허용 목록 변경 하나.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllowAction {
    /// 상수에 없는 값을 더한다.
    Allow,
    /// 막는다(상수에 있어도).
    Block,
    /// 운영자 변경을 지우고 상수 기본값으로 되돌린다.
    Reset,
}

impl AllowAction {
    pub fn as_str(self) -> &'static str {
        match self {
            AllowAction::Allow => "allow",
            AllowAction::Block => "block",
            AllowAction::Reset => "reset",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [AllowAction::Allow, AllowAction::Block, AllowAction::Reset]
            .into_iter()
            .find(|a| a.as_str() == s)
    }
}

/// 운영자 변경 값의 최대 길이.
pub const MAX_ALLOW_VALUE_CHARS: usize = 64;

/// 변경 요청을 검사하고 저장할 값(모델 해시는 소문자)을 준다.
pub fn validate_change(
    kind: AllowKind,
    value: &str,
    action: AllowAction,
) -> Result<String, String> {
    let value = value.trim();
    match kind {
        AllowKind::AppVersion => {
            if value.is_empty()
                || value.chars().count() > MAX_ALLOW_VALUE_CHARS
                || !value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
            {
                return Err("앱 판은 1~64자의 영문·숫자·`.`·`-`·`+`다".into());
            }
            Ok(value.to_owned())
        }
        AllowKind::ModelSha256 => {
            if action == AllowAction::Allow {
                return Err(
                    "모델은 막기만 된다 — 받는 모델은 후보 표에 고정한 것뿐이다(후보는 inference.rs의 MODELS를 고치는 PR로 바꾼다)"
                        .into(),
                );
            }
            let value = value.to_ascii_lowercase();
            if value.len() != 64 || !value.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err("모델 sha256은 16진 64자다".into());
            }
            Ok(value)
        }
    }
}

/// 서버가 지금 받는 앱 판·모델 sha256.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Allowlist {
    pub app_versions: Vec<String>,
    pub model_sha256: Vec<String>,
}

impl Allowlist {
    /// 상수 기본값만.
    pub fn constants() -> Self {
        Self::with_overrides(&[])
    }

    /// 상수 ∪ 더한 값 − 막은 값. `overrides`는 값마다 **마지막** 변경이다([`AllowAction::Reset`]이면 상수 그대로).
    pub fn with_overrides(overrides: &[(AllowKind, String, AllowAction)]) -> Self {
        let column = |kind: AllowKind| {
            let mut values: Vec<String> =
                kind.constants().iter().map(|v| (*v).to_owned()).collect();
            for (k, value, action) in overrides {
                if *k != kind {
                    continue;
                }
                match action {
                    AllowAction::Allow => {
                        if !values.contains(value) {
                            values.push(value.clone());
                        }
                    }
                    AllowAction::Block => values.retain(|v| v != value),
                    AllowAction::Reset => {}
                }
            }
            values
        };
        Allowlist {
            app_versions: column(AllowKind::AppVersion),
            model_sha256: column(AllowKind::ModelSha256),
        }
    }

    /// 신고값을 이 목록과 대조한다. 신고가 없으면(낡은 앱) 둘 다 실패다.
    pub fn check(&self, report: Option<&ClientIntegrity>) -> Result<(), IntegrityFailure> {
        let failure = match report {
            None => IntegrityFailure {
                app_ok: false,
                model_ok: false,
            },
            Some(r) => IntegrityFailure {
                app_ok: self.app_versions.contains(&r.app_version),
                model_ok: self
                    .model_sha256
                    .iter()
                    .any(|v| *v == r.model_sha256.to_ascii_lowercase()),
            },
        };
        if failure.app_ok && failure.model_ok {
            Ok(())
        } else {
            Err(failure)
        }
    }
}

/// 신고값을 저장·표시할 때 자르는 길이(거짓 신고로 큰 값을 밀어 넣지 못하게).
pub const MAX_REPORTED_CHARS: usize = 80;

/// 저장·표시용으로 신고값을 자른다.
pub fn clip_reported(s: &str) -> String {
    s.chars().take(MAX_REPORTED_CHARS).collect()
}

/// 대시보드 의심 신호의 시각이 어느 시계인가.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalClock {
    /// 앱 시계(과정 로그 사건의 `at_ms`). PC 시계가 틀릴 수 있다.
    App,
    /// 서버 시계.
    Server,
}

/// 의심 신호 하나. 판단은 고객사가 한다 — 「살펴볼 것」일 뿐이다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Suspicion {
    /// 유닉스 밀리초. 언제인지 모르는 신호(로그 끝 누락 등)는 없다.
    pub at_ms: Option<i64>,
    pub clock: SignalClock,
    /// 문항 번호(0부터).
    pub position: u32,
    pub kind: SuspicionKind,
}

/// 의심 신호의 종류.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SuspicionKind {
    /// 응시 창이 포커스를 잃었다(Alt+Tab 등). 돌아왔으면 비운 시간.
    FocusLost { seq: u64, away_ms: Option<u64> },
    /// 전체화면에서 빠져나왔다.
    FullscreenExited { seq: u64 },
    /// 편집기 밖에서 들어온 붙여넣기를 막았다.
    PasteBlocked { seq: u64, chars: u32 },
    /// 허용 목록에 없는 앱 판·모델로 문항 시작을 시도해 거부했다.
    IntegrityRejected {
        app_version: String,
        model_sha256: String,
        app_ok: bool,
        model_ok: bool,
        count: u32,
    },
    /// 과정 로그 검증 신호(#9).
    Log { signal: LogSignal },
    /// 웹캠 감지(#13): 얼굴 없음·여러 명·시선 이탈. 그 순간 스냅샷 참조.
    Webcam {
        seq: u64,
        reason: crate::webcam::SnapshotReason,
        count: Option<u32>,
        snapshot: Option<String>,
    },
    /// 응시 중 웹캠을 쓸 수 없게 됐다(#13).
    CameraUnavailable { seq: u64, reason: String },
}

impl SuspicionKind {
    /// 짧은 이름.
    pub fn label(&self) -> &'static str {
        match self {
            SuspicionKind::FocusLost { .. } => "포커스 이탈",
            SuspicionKind::FullscreenExited { .. } => "전체화면 이탈",
            SuspicionKind::PasteBlocked { .. } => "붙여넣기 막음",
            SuspicionKind::IntegrityRejected { .. } => "모델 무결성 실패",
            SuspicionKind::Log { .. } => "과정 로그",
            SuspicionKind::Webcam { reason, .. } => reason.label(),
            SuspicionKind::CameraUnavailable { .. } => "웹캠 끊김",
        }
    }

    /// 화면에 보이는 설명.
    pub fn describe(&self) -> String {
        match self {
            SuspicionKind::FocusLost { away_ms, .. } => match away_ms {
                Some(ms) => format!("응시 창을 떠났다가 {}초 뒤 돌아왔다", ms.div_ceil(1000)),
                None => "응시 창을 떠난 뒤 돌아온 기록이 없다".to_owned(),
            },
            SuspicionKind::FullscreenExited { .. } => {
                "전체화면 응시 모드에서 빠져나왔다(앱이 다시 전체화면으로 돌린다)".to_owned()
            }
            SuspicionKind::Webcam { reason, count, .. } => match (reason, count) {
                (crate::webcam::SnapshotReason::MultipleFaces, Some(n)) => {
                    format!("웹캠에 얼굴이 {n}개 보였다")
                }
                (crate::webcam::SnapshotReason::NoFace, _) => {
                    "웹캠에 얼굴이 보이지 않았다".to_owned()
                }
                (crate::webcam::SnapshotReason::GazeAway, _) => {
                    "얼굴이 화면이 아닌 곳(옆·아래)을 향했다".to_owned()
                }
                _ => "웹캠 감지".to_owned(),
            },
            SuspicionKind::CameraUnavailable { reason, .. } => {
                format!("응시 중 웹캠을 쓸 수 없게 됐다: {reason}")
            }
            SuspicionKind::PasteBlocked { chars, .. } => {
                format!("편집기 밖에서 {chars}자를 붙여넣으려 해 막았다")
            }
            SuspicionKind::IntegrityRejected {
                app_version,
                model_sha256,
                app_ok,
                model_ok,
                count,
            } => {
                let mut what = Vec::new();
                if !app_ok {
                    what.push(format!("앱 판 「{app_version}」"));
                }
                if !model_ok {
                    let short: String = model_sha256.chars().take(12).collect();
                    what.push(format!("모델 sha256 「{short}…」"));
                }
                format!(
                    "허용 목록에 없는 {}로 문항 시작을 {count}번 시도해 거부했다",
                    what.join("·")
                )
            }
            SuspicionKind::Log { signal } => signal.describe(),
        }
    }
}
