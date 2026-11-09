//! 출제 원본(spec #93): 코딩 문제 원본의 검증과 채점용 문제로 바꾸기.

use promptlab_shared::authoring::{MAX_TITLE_CHARS, ProblemSource};
use promptlab_shared::coding::Language;
use serde_json::json;

fn source() -> ProblemSource {
    serde_json::from_value(json!({
        "title": "두 수 더하기",
        "description": "`a + b`를 돌려준다.",
        "signature": {"name": "add", "params": [{"name": "a", "type": "int"}, {"name": "b", "type": "int"}], "returns": "int"},
        "examples": [{"args": [1, 2], "expected": 3}],
        "hidden": [
            {"args": [0, 0], "expected": 0},
            {"args": [-1, 1], "expected": 0},
            {"args": [5, 7], "expected": 12},
            {"args": [100, 1], "expected": 101},
            {"args": [-3, -4], "expected": -7}
        ],
        "reference_solution": "def add(a, b):\n    return a + b\n"
    }))
    .unwrap()
}

#[test]
fn 올바른_원본은_통과하고_기본_언어는_python() {
    let s = source();
    assert_eq!(s.validate(), Ok(()));
    assert_eq!(s.reference_language, Language::Python);
    assert_eq!(s.limits.case_time_ms, 2_000);
    let p = s.to_problem("org-x");
    assert_eq!(p.id, "org-x");
    assert_eq!(p.hidden.len(), 5);
    assert_eq!(p.reference_solution, s.reference_solution);
}

#[test]
fn 제목_설명_모범_답안_케이스_규칙을_어기면_까닭을_준다() {
    let mut s = source();
    s.title = " ".into();
    assert!(s.validate().unwrap_err().contains("제목"));

    let mut s = source();
    s.title = "가".repeat(MAX_TITLE_CHARS + 1);
    assert!(s.validate().is_err());

    let mut s = source();
    s.description = String::new();
    assert!(s.validate().unwrap_err().contains("설명"));

    let mut s = source();
    s.reference_solution = "  ".into();
    assert!(s.validate().is_err());

    let mut s = source();
    s.hidden.truncate(4);
    assert!(s.validate().is_err(), "히든은 5개 이상");

    let mut s = source();
    s.hidden[0].expected = json!("문자열");
    assert!(s.validate().is_err(), "기대값 타입이 반환 타입과 다르다");
}

#[test]
fn 원본은_json으로_왕복한다() {
    let s = source();
    let back: ProblemSource = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(back, s);
}
