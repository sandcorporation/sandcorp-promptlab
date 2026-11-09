//! 시험(문항 묶음)·응시 배정 초대의 규칙. 서버가 강제하고, 대시보드·앱은 같은 값으로 입력을 거르고 표시한다.
//!
//! - 시험 = 조직의 문항 묶음. 문항마다 문제은행의 코딩 문제 하나와 그 문항의 제한 시간.
//! - 용도: 채용([`Purpose::Recruiting`])은 응시자에게 「응시 완료」만 보인다. 사내 진단([`Purpose::Internal`])은
//!   시험의 `reveal_scores`가 켜졌을 때만 응시자에게 점수가 보인다([`candidate_sees_scores`]).
//! - 응시 배정 초대(#8)는 조직 구성원 초대(#7 `org_invites`)와 다르다: 받은 사람은 조직에 들어가지 않고
//!   그 시험 하나를 배정받는다.
//! - 제한은 서버에서도 강제한다: 문항 응시를 시작한 시각(서버 시계)부터 제한 시간 + [`SUBMIT_GRACE_SECS`]가
//!   지난 제출은 거부한다.

use serde::{Deserialize, Serialize};

use crate::coding::Language;

/// 문항 응시 제한(문제를 푸는 시간 — 케이스당 실행 시간은 문제의 [`crate::coding::Limits`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ItemLimits {
    /// 제한 시간(초).
    pub time_limit_secs: u32,
}

/// 시험 하나의 최대 문항 수.
pub const MAX_ITEMS: usize = 50;
/// 시험 제목 최대 길이(문자).
pub const MAX_TITLE_CHARS: usize = 100;
/// 문항 제한 시간 범위(초): 1분 ~ 2시간.
pub const TIME_LIMIT_RANGE: std::ops::RangeInclusive<u32> = 60..=7200;
/// 제한 시간이 끝난 뒤에도 받아 주는 여유(초). 앱은 시간이 다 되면 바로 제출하므로 네트워크 지연만 덮는다.
pub const SUBMIT_GRACE_SECS: i64 = 30;
/// 한 번에 초대할 수 있는 최대 이메일 수.
pub const MAX_INVITES_PER_REQUEST: usize = 100;
/// 응시 배정 초대 유효 기간(14일).
pub const EXAM_INVITE_SECS: i64 = 14 * 24 * 60 * 60;

/// 시험 용도.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    /// 채용. 결과는 고객사만 본다.
    Recruiting,
    /// 사내 진단. 시험 설정으로 응시자에게 점수를 공개할 수 있다.
    Internal,
}

impl Purpose {
    pub const ALL: [Purpose; 2] = [Purpose::Recruiting, Purpose::Internal];

    pub fn as_str(self) -> &'static str {
        match self {
            Purpose::Recruiting => "recruiting",
            Purpose::Internal => "internal",
        }
    }

    pub fn parse(s: &str) -> Option<Purpose> {
        Purpose::ALL.into_iter().find(|p| p.as_str() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            Purpose::Recruiting => "채용",
            Purpose::Internal => "사내 진단",
        }
    }
}

/// 응시자 본인이 점수·지표를 보는가. 채용은 언제나 아니다(「응시 완료」만).
pub fn candidate_sees_scores(purpose: Purpose, reveal_scores: bool) -> bool {
    purpose == Purpose::Internal && reveal_scores
}

/// 시험 문항 하나: 문제은행의 문제와 이 문항의 제한.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExamItem {
    pub problem_id: String,
    pub limits: ItemLimits,
}

/// 문항 제한이 허용 범위인가. 아니면 사람이 읽을 이유.
pub fn check_limits(limits: &ItemLimits) -> Result<(), String> {
    if !TIME_LIMIT_RANGE.contains(&limits.time_limit_secs) {
        return Err(format!(
            "제한 시간은 {}~{}분이다",
            TIME_LIMIT_RANGE.start() / 60,
            TIME_LIMIT_RANGE.end() / 60
        ));
    }
    Ok(())
}

/// 최종 점수의 숨은 케이스 비율 w(%)의 기본값(spec #69 결정 5 — 50:50).
pub const DEFAULT_SCORE_WEIGHT: u8 = 50;
/// 출제자가 더하는 대화 평가 기준표 항목의 최대 수.
pub const MAX_RUBRIC_ITEMS: usize = 2;
/// 기준표 항목 이름 최대 글자 수.
pub const MAX_RUBRIC_NAME_CHARS: usize = 30;
/// 기준표 항목 설명 최대 글자 수.
pub const MAX_RUBRIC_DESCRIPTION_CHARS: usize = 300;

/// 출제자가 더하는 대화 평가 기준표 항목(spec #69 결정 4). 평가 자체는 #72.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RubricItem {
    pub name: String,
    pub description: String,
}

/// 시험의 채점 설정: 최종 점수 비율·추가 기준표(AI 활용 평가는 #99가 다시 짓는다 — 지금 최종 = 통과율).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ExamSettings {
    /// 최종 점수 = 히든 케이스 통과율 × w + AI 활용 평가 × (100 − w), w는 0~100(%). 평가가 없으면 통과율만.
    pub score_weight: u8,
    /// 공통 4항목에 더하는 기준표 항목(최대 [`MAX_RUBRIC_ITEMS`]).
    #[serde(default)]
    pub rubric: Vec<RubricItem>,
    /// 응시자가 고를 수 있는 언어(1개 이상, 기본 4개 전부 — #96). 허용 밖 언어 제출은 서버가 `422`.
    #[serde(default = "all_languages")]
    pub allowed_languages: Vec<Language>,
}

/// 허용 언어 기본값 — 4개 전부.
pub fn all_languages() -> Vec<Language> {
    Language::ALL.to_vec()
}

impl Default for ExamSettings {
    fn default() -> Self {
        Self {
            score_weight: DEFAULT_SCORE_WEIGHT,
            rubric: Vec::new(),
            allowed_languages: all_languages(),
        }
    }
}

impl ExamSettings {
    /// 범위 검사(서버 422의 근거). 사람이 읽을 이유.
    pub fn validate(&self) -> Result<(), String> {
        if self.score_weight > 100 {
            return Err("숨은 케이스 비율은 0~100이다".into());
        }
        if self.allowed_languages.is_empty() {
            return Err("허용 언어는 1개 이상이다".into());
        }
        let mut seen = self.allowed_languages.clone();
        seen.sort();
        seen.dedup();
        if seen.len() != self.allowed_languages.len() {
            return Err("허용 언어가 겹친다".into());
        }
        if self.rubric.len() > MAX_RUBRIC_ITEMS {
            return Err(format!("추가 기준표 항목은 {MAX_RUBRIC_ITEMS}개까지다"));
        }
        for item in &self.rubric {
            if item.name.trim().is_empty() || item.description.trim().is_empty() {
                return Err("기준표 항목의 이름·설명이 비었다".into());
            }
            if item.name.chars().count() > MAX_RUBRIC_NAME_CHARS {
                return Err(format!(
                    "기준표 항목 이름은 {MAX_RUBRIC_NAME_CHARS}자까지다"
                ));
            }
            if item.description.chars().count() > MAX_RUBRIC_DESCRIPTION_CHARS {
                return Err(format!(
                    "기준표 항목 설명은 {MAX_RUBRIC_DESCRIPTION_CHARS}자까지다"
                ));
            }
        }
        Ok(())
    }
}

/// 응시 배정 초대 하나의 진행 상태(담당자가 보는 현황).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InviteStatus {
    /// 초대만 됐다(링크를 받았든 안 받았든 아직 문항을 시작하지 않았다).
    Invited,
    /// 문항을 하나 이상 시작했고 아직 전부 내지 않았다.
    InProgress,
    /// 모든 문항을 냈고 채점이 다 끝나지 않았다.
    Submitted,
    /// 모든 문항의 채점이 끝났다(실패 포함).
    Graded,
}

impl InviteStatus {
    /// 문항 수와 시작·제출·채점 끝 수로 상태를 정한다.
    pub fn from_counts(items: u32, started: u32, submitted: u32, finished: u32) -> InviteStatus {
        if items > 0 && submitted >= items {
            if finished >= items {
                InviteStatus::Graded
            } else {
                InviteStatus::Submitted
            }
        } else if started > 0 || submitted > 0 {
            InviteStatus::InProgress
        } else {
            InviteStatus::Invited
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            InviteStatus::Invited => "초대됨",
            InviteStatus::InProgress => "진행 중",
            InviteStatus::Submitted => "제출",
            InviteStatus::Graded => "채점 완료",
        }
    }
}

/// 초대 메일 발송 결과.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    /// 아직 보내지 않았다.
    Pending,
    Sent,
    /// 보내지 못했다(링크 복사로 전한다).
    Failed,
    /// 서버에 메일 설정이 없다(링크 복사로 전한다).
    NotConfigured,
}

impl Delivery {
    pub fn as_str(self) -> &'static str {
        match self {
            Delivery::Pending => "pending",
            Delivery::Sent => "sent",
            Delivery::Failed => "failed",
            Delivery::NotConfigured => "not_configured",
        }
    }

    pub fn parse(s: &str) -> Option<Delivery> {
        [
            Delivery::Pending,
            Delivery::Sent,
            Delivery::Failed,
            Delivery::NotConfigured,
        ]
        .into_iter()
        .find(|d| d.as_str() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            Delivery::Pending => "발송 대기",
            Delivery::Sent => "메일 보냄",
            Delivery::Failed => "메일 실패(링크로 전하세요)",
            Delivery::NotConfigured => "메일 꺼짐(링크로 전하세요)",
        }
    }
}

/// 이메일 주소 하나를 다듬는다(앞뒤 공백 제거, 소문자). 모양이 틀리면 `None`.
/// 사람을 이메일로 묶지는 않는다(계정은 `sub`) — 초대를 보낼 곳과 같은 시험의 중복 초대를 가리는 데만 쓴다.
pub fn normalize_email(raw: &str) -> Option<String> {
    let email = raw.trim().to_lowercase();
    let (local, domain) = email.split_once('@')?;
    let ok = !local.is_empty()
        && email.len() <= 254
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains('@')
        && !email
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | ',' | ';'));
    ok.then_some(email)
}

/// 붙여 넣은 이메일 목록(줄·쉼표·세미콜론·공백으로 나뉨)을 (올바른 것, 틀린 것)으로 나눈다. 중복은 하나로.
pub fn split_emails(text: &str) -> (Vec<String>, Vec<String>) {
    let mut valid: Vec<String> = Vec::new();
    let mut invalid = Vec::new();
    for raw in text
        .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .filter(|s| !s.is_empty())
    {
        match normalize_email(raw) {
            Some(email) if !valid.contains(&email) => valid.push(email),
            Some(_) => {}
            None => invalid.push(raw.to_owned()),
        }
    }
    (valid, invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 이메일을_나누고_다듬고_틀린_것을_가린다() {
        let (valid, invalid) =
            split_emails("A@Example.com, b@example.co.kr\n a@example.com;bad  c@d nope@x.y");
        assert_eq!(valid, ["a@example.com", "b@example.co.kr", "nope@x.y"]);
        assert_eq!(invalid, ["bad", "c@d"]);
    }

    #[test]
    fn 상태는_시작_제출_채점_수로_정해진다() {
        use InviteStatus::*;
        assert_eq!(InviteStatus::from_counts(2, 0, 0, 0), Invited);
        assert_eq!(InviteStatus::from_counts(2, 1, 0, 0), InProgress);
        assert_eq!(InviteStatus::from_counts(2, 2, 1, 1), InProgress);
        assert_eq!(InviteStatus::from_counts(2, 2, 2, 1), Submitted);
        assert_eq!(InviteStatus::from_counts(2, 2, 2, 2), Graded);
    }

    #[test]
    fn 채용은_응시자에게_점수를_보이지_않는다() {
        assert!(!candidate_sees_scores(Purpose::Recruiting, true));
        assert!(!candidate_sees_scores(Purpose::Internal, false));
        assert!(candidate_sees_scores(Purpose::Internal, true));
    }
}
