//! 코딩 문제 형식(spec #93 1절) — 서명·검증·비교·틀/하네스 생성. 틀이 실제로 컴파일되는지는 판정기 docker 시험
//! (`crates/judge/tests/docker.rs`, `--ignored`)이 이미지 안에서 본다.

use promptlab_shared::coding::{
    self, CodingError, CodingProblem, Language, Limits, MAX_SAFE_INT, Param, Signature, TestCase,
    ValueType, values_match,
};
use serde_json::{Value, json};

fn two_sum() -> Signature {
    Signature {
        name: "two_sum".into(),
        params: vec![
            Param {
                name: "nums".into(),
                ty: ValueType::IntArray,
            },
            Param {
                name: "target".into(),
                ty: ValueType::Int,
            },
        ],
        returns: ValueType::IntArray,
    }
}

fn case(args: Value, expected: Value) -> TestCase {
    TestCase {
        args: args.as_array().unwrap().clone(),
        expected,
    }
}

fn problem() -> CodingProblem {
    CodingProblem {
        id: "two-sum".into(),
        title: "두 수의 합".into(),
        description: "합이 target인 두 칸".into(),
        signature: two_sum(),
        examples: vec![case(json!([[2, 7, 11, 15], 9]), json!([0, 1]))],
        hidden: (0..5)
            .map(|_| case(json!([[1, 2, 3, 4], 3]), json!([0, 1])))
            .collect(),
        reference_solution: "def two_sum(nums, target):\n    return [0, 1]\n".into(),
        reference_language: Language::Python,
        limits: Limits::default(),
    }
}

#[test]
fn 표본_문제는_검증을_통과한다() {
    problem().validate().unwrap();
}

#[test]
fn 타입_표기는_닫힌_집합() {
    for t in ValueType::ALL {
        let s = serde_json::to_string(&t).unwrap();
        assert_eq!(s, format!("\"{}\"", t.as_str()));
        assert_eq!(serde_json::from_str::<ValueType>(&s).unwrap(), t);
    }
    // 집합 밖 타입은 역직렬화에서 막힌다(서버 422).
    for bad in ["\"long\"", "\"int[][][]\"", "\"double[][]\"", "\"map\""] {
        assert!(serde_json::from_str::<ValueType>(bad).is_err(), "{bad}");
    }
}

#[test]
fn 값_타입_검사() {
    assert!(ValueType::Int.accepts(&json!(MAX_SAFE_INT)));
    assert!(!ValueType::Int.accepts(&json!(MAX_SAFE_INT + 1)));
    assert!(!ValueType::Int.accepts(&json!(-MAX_SAFE_INT - 1)));
    assert!(!ValueType::Int.accepts(&json!(1.0)));
    assert!(ValueType::Double.accepts(&json!(1)));
    assert!(ValueType::Double.accepts(&json!(-0.5)));
    assert!(!ValueType::Double.accepts(&json!("1")));
    assert!(ValueType::StringMatrix.accepts(&json!([["a"], []])));
    assert!(!ValueType::StringMatrix.accepts(&json!(["a"])));
    assert!(!ValueType::IntArray.accepts(&json!([1, "2"])));
    assert!(ValueType::BoolArray.accepts(&json!([])));
}

#[test]
fn 검증은_규칙_밖을_막는다() {
    let mut p = problem();
    p.signature.name = "Class".into();
    assert!(matches!(p.validate(), Err(CodingError::BadIdentifier(_))));
    let mut p = problem();
    p.signature.name = "class".into();
    assert!(matches!(p.validate(), Err(CodingError::BadIdentifier(_))));
    let mut p = problem();
    p.signature.params[1].name = "nums".into();
    assert!(matches!(p.validate(), Err(CodingError::DuplicateParam(_))));
    let mut p = problem();
    p.hidden.truncate(4);
    assert_eq!(p.validate(), Err(CodingError::HiddenCount(4)));
    let mut p = problem();
    p.examples.clear();
    assert_eq!(p.validate(), Err(CodingError::ExampleCount(0)));
    let mut p = problem();
    p.hidden[2].args[1] = json!(MAX_SAFE_INT + 1);
    assert!(matches!(p.validate(), Err(CodingError::ArgType { .. })));
    let mut p = problem();
    p.hidden[0].args.pop();
    assert!(matches!(p.validate(), Err(CodingError::ArgCount { .. })));
    let mut p = problem();
    p.hidden[0].expected = json!("0,1");
    assert!(matches!(
        p.validate(),
        Err(CodingError::ExpectedType { .. })
    ));
    let mut p = problem();
    p.hidden[0].args[0] = json!(vec![1; 40_000]);
    assert!(matches!(
        p.validate(),
        Err(CodingError::CaseTooLarge { .. })
    ));
    let mut p = problem();
    for c in &mut p.hidden {
        c.args[0] = json!(vec![1; 12_000]);
    }
    for _ in 0..40 {
        let c = p.hidden[0].clone();
        p.hidden.push(c);
    }
    assert_eq!(p.validate(), Err(CodingError::TotalTooLarge));
    let mut p = problem();
    p.limits.case_time_ms = 60_000;
    assert_eq!(p.validate(), Err(CodingError::CaseTime(60_000)));
    let mut p = problem();
    p.reference_solution = " ".into();
    assert_eq!(p.validate(), Err(CodingError::MissingReference));
}

#[test]
fn 비교는_json_같음_double은_1e6() {
    use ValueType::*;
    assert!(values_match(Int, &json!(3), &json!(3)));
    assert!(!values_match(Int, &json!(3), &json!(3.0)));
    assert!(values_match(Double, &json!(0.1), &json!(0.1000000001)));
    assert!(values_match(Double, &json!(1e9), &json!(1e9 + 0.5)));
    assert!(!values_match(Double, &json!(1.0), &json!(1.001)));
    assert!(values_match(Double, &json!(2), &json!(2.0)));
    assert!(!values_match(Double, &json!(2), &Value::Null));
    assert!(values_match(
        DoubleArray,
        &json!([1.0, 2.0]),
        &json!([1.0000001, 2])
    ));
    assert!(!values_match(
        DoubleArray,
        &json!([1.0]),
        &json!([1.0, 2.0])
    ));
    assert!(values_match(
        IntMatrix,
        &json!([[1], []]),
        &json!([[1], []])
    ));
    assert!(!values_match(IntMatrix, &json!([[1], []]), &json!([[1]])));
    assert!(values_match(String, &json!("가"), &json!("가")));
    assert!(!values_match(Bool, &json!(true), &json!(1)));
}

#[test]
fn 틀은_언어별_함수_모양() {
    let sig = two_sum();
    assert_eq!(
        coding::solution_template(Language::Python, &sig),
        "def two_sum(nums: list[int], target: int) -> list[int]:\n    answer = []\n    return answer\n"
    );
    assert_eq!(
        coding::solution_template(Language::Javascript, &sig),
        "/**\n * @param {number[]} nums\n * @param {number} target\n * @return {number[]}\n */\nfunction two_sum(nums, target) {\n  let answer = [];\n  return answer;\n}\n"
    );
    assert_eq!(
        coding::solution_template(Language::Java, &sig),
        "class Solution {\n    public long[] two_sum(long[] nums, long target) {\n        long[] answer = new long[]{};\n        return answer;\n    }\n}\n"
    );
    assert_eq!(
        coding::solution_template(Language::Cpp, &sig),
        "#include <string>\n#include <vector>\n\nusing namespace std;\n\nvector<long long> two_sum(vector<long long> nums, long long target) {\n    vector<long long> answer = {};\n    return answer;\n}\n"
    );
}

#[test]
fn 하네스는_언어_타입마다_자리표시가_남지_않는다() {
    for lang in Language::ALL {
        for t in ValueType::ALL {
            let sig = Signature {
                name: "echo_value".into(),
                params: vec![
                    Param {
                        name: "x".into(),
                        ty: t,
                    },
                    Param {
                        name: "y".into(),
                        ty: ValueType::StringMatrix,
                    },
                ],
                returns: t,
            };
            let h = coding::harness(lang, &sig);
            for ph in [
                "__NAME__",
                "__PARAMS__",
                "__RETURNS__",
                "__CONVERT__",
                "__ARGS__",
                "__RET__",
                "__MEMORY__",
            ] {
                assert!(!h.contains(ph), "{lang} {t}: {ph}");
            }
            assert!(h.contains("echo_value"), "{lang} {t}");
            assert!(h.contains("ready"), "{lang} {t}: 준비 줄");
            let tpl = coding::solution_template(lang, &sig);
            assert!(tpl.contains("echo_value"), "{lang} {t}");
        }
    }
}

#[test]
fn 공개_뷰는_히든_케이스와_모범_답안을_싣지_않는다() {
    let mut p = problem();
    p.hidden[0].args[1] = json!(424242);
    p.reference_solution = "HIDDEN_REFERENCE_MARK".into();
    let v = p.public_view();
    let s = serde_json::to_string(&v).unwrap();
    assert!(!s.contains("424242"));
    assert!(!s.contains("HIDDEN_REFERENCE_MARK"));
    assert!(!s.contains("hidden"));
    assert_eq!(
        v.templates.iter().map(|t| t.language).collect::<Vec<_>>(),
        Language::ALL.to_vec()
    );
    assert_eq!(v.examples, p.examples);
}

#[test]
fn 러너_이미지는_언어마다_하나() {
    for lang in Language::ALL {
        let img = coding::runner_image(lang);
        assert_eq!(
            img.name,
            format!("ghcr.io/sandcorporation/promptlab-runner-{lang}")
        );
        match img.digest {
            None => assert_eq!(
                img.reference(),
                format!("{}:{}", img.name, coding::RUNNER_TAG)
            ),
            Some(d) => assert_eq!(img.reference(), format!("{}@{d}", img.name)),
        }
    }
}
