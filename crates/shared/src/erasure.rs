//! 고객사 관리자의 응시자 데이터 삭제와 감사 로그(#55). 규칙의 정본은 여기 한 곳이다.
//!
//! - **누가**: 조직 관리자(구성원 관리 권한)만. 감독관·출제자·열람자 403, 다른 조직 404.
//! - **단위**: 시험 안의 배정(응시자 한 명의 한 시험) 하나. 지우는 범위는 #54 자동 삭제와 같다
//!   ([`crate::retention::Erased`] — 배정·제출·문항 시작·과정 로그·무결성 거부·대조·웹캠 동의·사진·AI 리포트, 비용 기록은
//!   연결만 끊는다). 웹캠 사진도 30일 보유를 기다리지 않고 즉시 지운다.
//! - **진행 중·큐**: 문항을 푸는 중이거나 제출이 채점 큐에 있거나 채점 중이어도 지운다(응시자의 삭제 요청을 미루지 않는다).
//!   응시자 앱의 다음 호출은 404(배정 없음)가 되고, 워커·리포트 루프는 사라진 행을 만나면 조용히 건너뛴다.
//! - **멱등**: 이미 지운 배정을 다시 지우면 처음 지울 때의 감사 기록을 그대로 돌려준다(같은 결과, 새 기록 없음).
//! - **감사 로그**: 누가(`sub`)·언제·조직·시험·배정 id·종류별 건수·사유(선택). 이메일 원문과 지운 내용은 남기지 않고,
//!   응시자 식별자는 [`subject_hash`](조직 id로 묶은 sha256)로만 남긴다 — 나중에 같은 사람이 「지워졌나」를 물으면
//!   관리자가 그 이메일로 찾아 확인할 수 있지만, 기록만 보고는 누구인지 알 수 없고 다른 조직의 기록과 이을 수 없다.
//!   보관 기간은 [`AUDIT_RETENTION_DAYS`](3년) — #54 주기 삭제가 함께 지운다.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::retention::Erased;

/// 감사 로그 보관 기간(일). 응시 데이터의 가장 긴 보관 기간([`crate::retention::MAX_DAYS`])과 같게 —
/// 지운 데이터가 남아 있었을 수 있는 가장 긴 기간 동안 「지웠다」는 증빙을 가진다.
pub const AUDIT_RETENTION_DAYS: u32 = crate::retention::MAX_DAYS;

/// 사유의 최대 길이(글자).
pub const REASON_MAX_CHARS: usize = 500;

/// 응시자 식별자 해시: sha256(`<조직 id>:<소문자로 다듬은 이메일>`)의 16진수.
/// 조직 id를 섞어 같은 사람이라도 조직마다 다른 값이 된다(조직 사이를 잇지 못한다).
pub fn subject_hash(org_id: &str, email: &str) -> String {
    let normalized = email.trim().to_lowercase();
    let digest = Sha256::digest(format!("{org_id}:{normalized}").as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// 사유 검사(앞뒤 공백을 걷고, 비면 `None`). 길면 사람에게 보일 까닭.
pub fn normalize_reason(reason: Option<&str>) -> Result<Option<String>, String> {
    let Some(reason) = reason.map(str::trim).filter(|r| !r.is_empty()) else {
        return Ok(None);
    };
    if reason.chars().count() > REASON_MAX_CHARS {
        return Err(format!("사유는 {REASON_MAX_CHARS}자까지다"));
    }
    Ok(Some(reason.to_owned()))
}

/// `POST /orgs/{id}/exams/{exam}/invites/{invite}/erase`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EraseRequest {
    /// 사유(선택, [`REASON_MAX_CHARS`]자까지). 응시자의 이름·이메일은 적지 않는다(화면이 안내한다).
    #[serde(default)]
    pub reason: Option<String>,
}

/// 감사 기록 하나. 지운 내용·이메일 원문은 없다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErasureRecord {
    pub id: i64,
    /// 지운 시각(유닉스 초).
    pub at: i64,
    pub by_sub: String,
    pub by_name: Option<String>,
    pub exam_id: Option<String>,
    /// 시험이 남아 있으면 제목.
    pub exam_title: Option<String>,
    pub invite_id: String,
    /// [`subject_hash`].
    pub subject_hash: String,
    pub reason: Option<String>,
    /// 종류별 지운 건수.
    pub erased: Erased,
}

/// `GET /orgs/{id}/erasures`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErasureLog {
    /// 새것부터.
    pub records: Vec<ErasureRecord>,
    /// 감사 기록 보관 기간(일).
    pub retention_days: u32,
}
