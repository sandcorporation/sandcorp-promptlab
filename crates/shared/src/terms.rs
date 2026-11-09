//! 이용약관 동의(#86). 조직을 만들거나 구성원 초대를 받을 때 지금 약관 판을 보내야 한다.
//!
//! 약관 개정 때는 [`TERMS_VERSION`]·[`TERMS_URL`]만 올린다(서버·대시보드가 이 한 곳을 쓴다).
//! 출시 전이라 이미 있는 조직에 재동의를 받거나 기능을 막지 않는다.

use serde::{Deserialize, Serialize};

/// 지금 이용약관 판(개정일).
pub const TERMS_VERSION: &str = "2026-11-07";

/// 이용약관 문서 주소.
pub const TERMS_URL: &str = "https://sandcorporation.github.io/privacy-and-terms/promptlab/terms/";

/// `POST /invites/{token}/accept` 본문 — 동의한 약관 판.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptInviteRequest {
    pub terms_version: String,
}

/// 내가 그 조직에 대해 동의한 기록.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TermsAcceptance {
    pub version: String,
    /// 동의 시각(유닉스 초).
    pub accepted_at: i64,
}
