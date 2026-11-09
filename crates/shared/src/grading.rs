//! 채점 리포트(spec #93): 「문제 + 제출 코드 → 통과 수」. 판정기(`promptlab-judge`)가 코드를 돌리고 채점 워커가
//! 히든 기대값과 비교해([`crate::coding::values_match`]) 이 리포트를 남긴다.
//!
//! 밖(앱·대시보드·API)으로는 **언어·통과 수·전체 수만** 나간다. 케이스별 결과·컴파일 메시지·stdout·stderr는
//! 응시자에게 보이지 않는다(히든 케이스를 짐작하는 통로가 된다). 컴파일 오류는 응시자 탓이라 채점 실패가 아니라
//! `passed = 0`이다 — 채점 실패([`crate::api::SubmissionState::Failed`])는 판정기 쪽 문제(규칙 밖 요청·서버 오류)뿐이다.

use serde::{Deserialize, Serialize};

use crate::coding::Language;

/// 밖으로 나가는 채점 리포트.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GradeReport {
    pub problem_id: String,
    /// 제출 코드의 언어.
    pub language: Language,
    /// 통과한 히든 케이스 수.
    pub passed: u32,
    /// 히든 케이스 수.
    pub total: u32,
}

impl GradeReport {
    /// 케이스별 통과 여부로 리포트를 만든다(`total` = 길이).
    pub fn from_cases(problem_id: &str, language: Language, case_passed: &[bool]) -> Self {
        let passed = case_passed.iter().filter(|p| **p).count();
        Self {
            problem_id: problem_id.to_owned(),
            language,
            passed: u32::try_from(passed).unwrap_or(u32::MAX),
            total: u32::try_from(case_passed.len()).unwrap_or(u32::MAX),
        }
    }

    /// 주 점수: 히든 케이스 통과율(0.0~1.0). 케이스가 없으면 0.
    pub fn pass_rate(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        f64::from(self.passed.min(self.total)) / f64::from(self.total)
    }
}
