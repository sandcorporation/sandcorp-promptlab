//! 문제 편집 화면의 입력 상태(#96): 칸마다의 글·선택을 모아 둔 [`Draft`]와 즉시 검증.
//!
//! UI 없이 시험한다. 규칙은 서버와 같은 shared 검증([`ProblemSource::validate`]·[`Signature::validate`]) 그대로다 —
//! wasm에서 서버를 거치지 않고 서명·케이스가 맞는지 바로 보인다. 점검(모범 답안 판정·언어별 틀 컴파일)은 서버 몫.

use promptlab_shared::authoring::ProblemSource;
use promptlab_shared::coding::{
    Language, Limits, Param, Signature, TestCase, ValueType, solution_template,
};

/// 편집 칸의 글·선택 그대로의 문제 원본. 케이스는 JSON 글이라 깨진 채로도 들고 있다.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub title: String,
    pub description: String,
    pub fn_name: String,
    pub params: Vec<Param>,
    pub returns: ValueType,
    /// 예제 케이스 JSON 배열 글(`[{"args": [...], "expected": ...}, …]`).
    pub examples: String,
    /// 히든 케이스 JSON 배열 글.
    pub hidden: String,
    pub reference_language: Language,
    pub reference_solution: String,
    /// 케이스당 시간(밀리초) 글.
    pub case_time_ms: String,
}

fn cases_json(cases: &[TestCase]) -> String {
    serde_json::to_string_pretty(cases).unwrap_or_default()
}

impl Draft {
    pub fn from_source(s: &ProblemSource) -> Self {
        Self {
            title: s.title.clone(),
            description: s.description.clone(),
            fn_name: s.signature.name.clone(),
            params: s.signature.params.clone(),
            returns: s.signature.returns,
            examples: cases_json(&s.examples),
            hidden: cases_json(&s.hidden),
            reference_language: s.reference_language,
            reference_solution: s.reference_solution.clone(),
            case_time_ms: s.limits.case_time_ms.to_string(),
        }
    }

    pub fn signature(&self) -> Signature {
        Signature {
            name: self.fn_name.trim().to_owned(),
            params: self
                .params
                .iter()
                .map(|p| Param {
                    name: p.name.trim().to_owned(),
                    ty: p.ty,
                })
                .collect(),
            returns: self.returns,
        }
    }

    fn parse_cases(label: &str, text: &str) -> Result<Vec<TestCase>, String> {
        serde_json::from_str(text).map_err(|e| format!("{label} JSON을 읽지 못했습니다: {e}"))
    }

    /// 예제 케이스 개수(JSON이 읽히면).
    pub fn example_count(&self) -> Option<usize> {
        Self::parse_cases("예제", &self.examples)
            .ok()
            .map(|c| c.len())
    }

    /// 히든 케이스 개수(JSON이 읽히면).
    pub fn hidden_count(&self) -> Option<usize> {
        Self::parse_cases("히든", &self.hidden)
            .ok()
            .map(|c| c.len())
    }

    /// 원본으로 읽는다. 읽거나 검증하지 못하면 고칠 곳을 사람이 읽을 말로 모두 돌려준다.
    pub fn to_source(&self) -> Result<ProblemSource, Vec<String>> {
        let mut issues = Vec::new();
        let examples = Self::parse_cases("예제", &self.examples)
            .map_err(|e| issues.push(e))
            .ok();
        let hidden = Self::parse_cases("히든", &self.hidden)
            .map_err(|e| issues.push(e))
            .ok();
        let case_time_ms = match self.case_time_ms.trim().parse::<u64>() {
            Ok(v) => Some(v),
            Err(_) => {
                issues.push("케이스 시간은 밀리초 숫자입니다".to_owned());
                None
            }
        };
        let (Some(examples), Some(hidden), Some(case_time_ms)) = (examples, hidden, case_time_ms)
        else {
            if let Err(e) = self.signature().validate() {
                issues.push(format!("서명: {e}"));
            }
            return Err(issues);
        };
        let source = ProblemSource {
            title: self.title.clone(),
            description: self.description.clone(),
            signature: self.signature(),
            examples,
            hidden,
            reference_solution: self.reference_solution.clone(),
            reference_language: self.reference_language,
            limits: Limits { case_time_ms },
        };
        source.validate().map_err(|e| vec![e])?;
        Ok(source)
    }

    /// 지금 입력의 문제 목록(비어 있으면 저장할 수 있다).
    pub fn issues(&self) -> Vec<String> {
        self.to_source().err().unwrap_or_default()
    }

    /// 서명이 맞으면 언어 틀(서명이 틀렸으면 `None` — 틀을 만들 수 없다).
    pub fn template(&self, language: Language) -> Option<String> {
        let sig = self.signature();
        sig.validate().ok()?;
        Some(solution_template(language, &sig))
    }
}

/// 서명 줄의 타입 글(`int[]` 등) → 타입.
pub fn parse_type(s: &str) -> Option<ValueType> {
    ValueType::ALL.into_iter().find(|t| t.as_str() == s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::problems::blank_source;

    fn draft() -> Draft {
        Draft::from_source(&blank_source())
    }

    #[test]
    fn 뼈대는_그대로_원본으로_돌아온다() {
        let d = draft();
        assert_eq!(d.to_source().unwrap(), blank_source());
        assert!(d.issues().is_empty());
        assert_eq!((d.example_count(), d.hidden_count()), (Some(1), Some(5)));
    }

    #[test]
    fn 깨진_케이스_json과_서명_오류를_함께_보인다() {
        let mut d = draft();
        d.hidden = "[{".into();
        d.fn_name = "1bad".into();
        let issues = d.issues();
        assert!(
            issues.iter().any(|i| i.starts_with("히든 JSON")),
            "{issues:?}"
        );
        assert!(issues.iter().any(|i| i.starts_with("서명")), "{issues:?}");
        assert_eq!(d.hidden_count(), None);
    }

    #[test]
    fn 서명과_맞지_않는_케이스는_shared_검증이_막는다() {
        let mut d = draft();
        d.examples = r#"[{"args": ["x", 2], "expected": 3}]"#.into();
        assert_eq!(d.issues().len(), 1, "{:?}", d.issues());
        d.examples = r#"[]"#.into();
        assert!(!d.issues().is_empty());
        let mut d = draft();
        d.case_time_ms = "abc".into();
        assert!(d.issues()[0].contains("밀리초"));
        d.case_time_ms = "100".into();
        assert!(!d.issues().is_empty());
    }

    #[test]
    fn 틀_미리보기는_서명이_맞을_때만_나온다() {
        let mut d = draft();
        for lang in Language::ALL {
            assert!(d.template(lang).unwrap().contains("add"), "{lang:?}");
        }
        d.fn_name = "".into();
        assert!(d.template(Language::Python).is_none());
    }

    #[test]
    fn 타입_이름으로_고른다() {
        for t in ValueType::ALL {
            assert_eq!(parse_type(t.as_str()), Some(t));
        }
        assert_eq!(parse_type("float"), None);
    }
}
