//! 발행 전 점검(spec #93, #96이 넓힌다): 저장한 코딩 문제를 시험·연습에 내기 전에 **모범 답안**
//! ([`crate::coding::CodingProblem::reference_solution`], 언어 `reference_language`)을 판정기로 예제 + 히든 케이스에
//! 돌려 **모두 통과**하는지 본다. 하나라도 틀리면 문제(기대값·서명)나 모범 답안이 잘못된 것이다.
//!
//! 점검 결과([`CheckResult`])에는 통과 수와 짧은 이유만 싣는다 — 케이스 입력·출력·컴파일 메시지는 없다
//! (출제자 화면에서도 히든 기대값을 다시 보이지 않는다. 원본은 편집 API가 따로 준다).
//!
//! **#96**: 점검은 둘이다 — ① 모범 답안이 예제 + 히든을 모두 통과, ② 서명에서 생성한 **4개 언어 빈 틀이 모두 컴파일**
//! ([`TemplateCheck`] 언어별 — 틀은 기본값을 돌려주는 빈 함수라 판정기로 한 케이스만 돌려 컴파일 결과만 본다).
//! 둘 다 통과해야 [`CheckResult::all_passed`](발행 가능).

use serde::{Deserialize, Serialize};

use crate::coding::Language;

/// 모범 답안 판정 한 번의 결과(워커가 남긴다).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CheckResult {
    /// 통과한 케이스 수(예제 + 히든).
    pub passed: u32,
    /// 케이스 수(예제 + 히든).
    pub total: u32,
    /// 모범 답안이 컴파일되었는가(Python·JavaScript는 늘 `true`).
    pub compiled: bool,
    /// 언어별 빈 틀 컴파일 결과(언어 순서 [`Language::ALL`], #96). 비어 있으면(옛 점검) 발행할 수 없다.
    #[serde(default)]
    pub templates: Vec<TemplateCheck>,
}

/// 언어 하나의 빈 틀이 컴파일되었는가(Python·JavaScript는 문법 오류가 없는가).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TemplateCheck {
    pub language: Language,
    pub compiled: bool,
}

impl CheckResult {
    /// 모두 통과했는가(케이스가 하나도 없으면 아니다).
    pub fn all_passed(&self) -> bool {
        self.compiled && self.total > 0 && self.passed == self.total && self.templates_ok()
    }

    /// 4개 언어 틀이 모두 컴파일되었는가(빠진 언어가 있으면 아니다).
    pub fn templates_ok(&self) -> bool {
        Language::ALL.iter().all(|l| {
            self.templates
                .iter()
                .any(|t| t.language == *l && t.compiled)
        })
    }

    /// 실패면 출제자에게 보이는 짧은 이유(케이스 내용 없음).
    pub fn reason(&self) -> Option<String> {
        if !self.compiled {
            Some("모범 답안이 컴파일되지 않습니다".into())
        } else if self.total == 0 {
            Some("케이스가 없습니다".into())
        } else if self.passed < self.total {
            Some(format!(
                "모범 답안이 케이스 {} / {}만 통과합니다 — 기대값·서명·모범 답안을 확인하세요",
                self.passed, self.total
            ))
        } else if !self.templates_ok() {
            let failed: Vec<&str> = Language::ALL
                .iter()
                .filter(|l| {
                    !self
                        .templates
                        .iter()
                        .any(|t| t.language == **l && t.compiled)
                })
                .map(|l| l.display_name())
                .collect();
            Some(format!(
                "{} 틀이 컴파일되지 않습니다 — 함수·인자 이름을 확인하세요",
                failed.join("·")
            ))
        } else {
            None
        }
    }
}
