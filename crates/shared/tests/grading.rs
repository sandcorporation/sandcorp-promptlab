//! 채점 리포트(spec #93): 밖으로는 언어·통과 수만.

use promptlab_shared::coding::Language;
use promptlab_shared::grading::GradeReport;

#[test]
fn 케이스별_통과로_통과_수와_통과율을_낸다() {
    let r = GradeReport::from_cases("two-sum", Language::Java, &[true, false, true, true]);
    assert_eq!((r.passed, r.total), (3, 4));
    assert_eq!(r.pass_rate(), 0.75);
    assert_eq!(r.language, Language::Java);
}

#[test]
fn 케이스가_없거나_컴파일_오류면_0점() {
    assert_eq!(
        GradeReport::from_cases("p", Language::Cpp, &[]).pass_rate(),
        0.0
    );
    let compile_error = GradeReport::from_cases("p", Language::Cpp, &[false; 5]);
    assert_eq!((compile_error.passed, compile_error.pass_rate()), (0, 0.0));
}

#[test]
fn 직렬화에는_언어와_통과_수만_있다() {
    let r = GradeReport::from_cases("p", Language::Python, &[true, false]);
    assert_eq!(
        serde_json::to_value(&r).unwrap(),
        serde_json::json!({"problem_id": "p", "language": "python", "passed": 1, "total": 2})
    );
}
