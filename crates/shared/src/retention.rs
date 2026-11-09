//! 데이터 보유·자동 삭제(#54). 규칙의 정본은 여기 한 곳이다 — 서버(삭제 주기 작업·설정 API)와 대시보드가 같이 쓴다.
//!
//! - **보관 기간**: 시험별 값 → 조직 기본값 → [`DEFAULT_DAYS`](365일) 순으로 정한다([`effective_days`]).
//!   허용 범위는 [`MIN_DAYS`]~[`MAX_DAYS`]. 조직 기본값은 구성원 관리(관리자), 시험별은 시험 관리(관리자·출제자)가 바꾼다.
//!   바꿀 때마다 이력(누가·언제·전·후)을 남긴다.
//! - **응시 종료 시각**은 배정(응시 하나)마다 잰다: 그 배정의 마지막 활동(초대·수락·문항 시작·제출, 받지 않은 초대는 링크 만료) 시각과
//!   시험의 응시 가능 시간 끝(#14 `window_ends_at`) 중 늦은 것. 삭제 시각 = 종료 + 보관 기간([`delete_at`]).
//!   응시 가능 시간이 끝나기 전에는 지우지 않고, 같은 시험을 오래 쓰는 사내 진단에서도 먼저 끝난 응시는 제때 지운다.
//! - **지우는 것**: 배정(이메일·응시자 `sub`)과 그 제출(프롬프트·채점 결과)·문항 시작·과정 로그(사건·키·거부)·
//!   무결성 거부·공개 예시 대조·웹캠 동의·사진·AI 리포트. 비용 기록은 숫자(토큰·비용·조직·시험)만 남기고 배정·제출 연결을
//!   끊는다. 시험 구성(문항·제한)은 남기고, 시험의 응시 수는 익명 숫자(`exams.erased_attempts`)로 이어 센다.
//! - **개인 연습**(#18): 마지막 연습 제출 뒤 [`PRACTICE_INACTIVE_DAYS`]일 동안 새 연습이 없으면 그 사람의 연습 기록 전부,
//!   sandcorp-auth 탈퇴 알림(`user.deleted` 웹훅)을 받으면 즉시 지운다.

use serde::{Deserialize, Serialize};

/// 기본 보관 기간(일) — 대표 결정: 시험 종료 뒤 1년.
pub const DEFAULT_DAYS: u32 = 365;
/// 가장 짧은 보관 기간. 웹캠 사진 보유(30일, #13)보다 짧으면 기록이 사진보다 먼저 사라져 감독 검토가 어긋난다.
pub const MIN_DAYS: u32 = 30;
/// 가장 긴 보관 기간(3년). 기본의 세 배까지 — 개인정보를 무기한 쌓지 않는다.
pub const MAX_DAYS: u32 = 1095;
/// 개인 연습 기록을 지우는 무활동 기간(일, 대표 결정 q8).
pub const PRACTICE_INACTIVE_DAYS: u32 = 365;
/// 자동 삭제 주기(초). 서버가 이만큼마다 지난 데이터를 지운다.
pub const PURGE_INTERVAL_SECS: u64 = 3600;

const DAY_SECS: i64 = 86_400;

/// 실제로 쓰는 보관 기간: 시험별 → 조직 기본 → [`DEFAULT_DAYS`].
pub fn effective_days(exam_days: Option<u32>, org_days: Option<u32>) -> u32 {
    exam_days.or(org_days).unwrap_or(DEFAULT_DAYS)
}

/// 허용 범위 안인가. 밖이면 사람에게 보일 까닭.
pub fn validate_days(days: u32) -> Result<(), String> {
    if (MIN_DAYS..=MAX_DAYS).contains(&days) {
        Ok(())
    } else {
        Err(format!(
            "보관 기간은 {MIN_DAYS}~{MAX_DAYS}일이어야 한다(받은 값 {days})"
        ))
    }
}

/// 응시 종료 시각(유닉스 초)과 보관 기간으로 삭제 시각. 이 시각이 되면(같거나 지나면) 지운다.
pub fn delete_at(ended_at: i64, days: u32) -> i64 {
    ended_at + i64::from(days) * DAY_SECS
}

/// 지금부터 삭제까지 남은 날(올림). 이미 지났으면 0.
pub fn days_left(delete_at: i64, now: i64) -> u32 {
    let left = delete_at - now;
    if left <= 0 {
        0
    } else {
        u32::try_from((left + DAY_SECS - 1) / DAY_SECS).unwrap_or(u32::MAX)
    }
}

/// `PUT /orgs/{id}/retention`·`PUT /orgs/{id}/exams/{exam}/retention`. `None`이면 설정을 지운다(위 단계로 돌아간다).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetRetentionRequest {
    pub days: Option<u32>,
}

/// 보관 기간 변경 한 번.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionChange {
    /// 바꾼 시각(유닉스 초).
    pub at: i64,
    pub by_sub: String,
    pub by_name: Option<String>,
    /// 시험별 설정이면 그 시험, 조직 기본값이면 `None`.
    pub exam_id: Option<String>,
    pub old_days: Option<u32>,
    pub new_days: Option<u32>,
}

/// `GET /orgs/{id}/retention`·`GET /orgs/{id}/exams/{exam}/retention`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionView {
    /// 조직 기본값(없으면 [`DEFAULT_DAYS`]를 쓴다).
    pub org_days: Option<u32>,
    /// 시험별 값(조직 보기에서는 언제나 `None`).
    pub exam_days: Option<u32>,
    /// 실제로 쓰는 값([`effective_days`]).
    pub effective_days: u32,
    pub default_days: u32,
    pub min_days: u32,
    pub max_days: u32,
    /// 바뀐 이력(새것부터). 조직 보기는 조직 기본값과 모든 시험의 변경, 시험 보기는 그 시험과 조직 기본값의 변경.
    pub history: Vec<RetentionChange>,
}

/// 한 번의 삭제에서 지운 건수(종류별). 로그에는 이 숫자만 남긴다.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Erased {
    /// 배정(초대) — 이메일·응시자 `sub`.
    pub assignments: u64,
    /// 제출(코드·채점 결과). 개인 연습 제출 포함.
    pub submissions: u64,
    pub item_starts: u64,
    /// 과정 로그 사건·등록 키·업로드 거부.
    pub log_events: u64,
    pub log_keys: u64,
    pub log_rejections: u64,
    pub integrity_rejections: u64,
    pub webcam_photos: u64,
    pub webcam_consents: u64,
    pub reports: u64,
    /// 배정·제출 연결을 끊은 비용 기록(숫자는 남는다).
    pub usage_unlinked: u64,
}

impl Erased {
    /// 지운 것이 하나도 없는가.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// 두 결과를 더한다.
    pub fn add(&mut self, other: &Erased) {
        self.assignments += other.assignments;
        self.submissions += other.submissions;
        self.item_starts += other.item_starts;
        self.log_events += other.log_events;
        self.log_keys += other.log_keys;
        self.log_rejections += other.log_rejections;
        self.integrity_rejections += other.integrity_rejections;
        self.webcam_photos += other.webcam_photos;
        self.webcam_consents += other.webcam_consents;
        self.reports += other.reports;
        self.usage_unlinked += other.usage_unlinked;
    }
}
