//! 4개 언어 solution 틀·하네스 생성기 — 서명에서 만든다(앱·서버·판정기가 같이 쓴다).
//!
//! ## 하네스 규약
//!
//! 러너 실행기(`promptlab-runner`, 이미지 안)가 작업 폴더에 풀이·하네스를 [`files`]의 이름으로 쓰고,
//! 컴파일(또는 문법 검사)한 뒤 하네스를 띄운다.
//!
//! - **입력(stdin)**: 케이스들의 인자 배열을 JSON 하나로 — `[[인자1, 인자2, …], …]`. 기대값은 넣지 않는다
//!   (비교는 러너 밖에서 [`super::values_match`]로 한다 — 히든 기대값은 컨테이너에 들어가지 않는다).
//! - **결과(fd 3)**: 줄 하나에 JSON 하나, 줄마다 바로 flush한다(벽시계로 죽어도 끝난 케이스까지는 남는다).
//!   1. 입력을 읽고 풀이를 올린 뒤 `{"ready":true}`
//!   2. 케이스마다 `{"ok":<반환값>}` · `{"error":"runtime","message":"…"}` · `{"error":"memory"}`
//! - 풀이의 `print`(stdout·stderr)는 fd 1·2 그대로 — 결과 줄과 섞이지 않는다.
//! - 케이스마다 예외를 잡는다 — 한 케이스의 런타임 오류는 그 케이스에만 걸린다. 풀이를 올리다 난 오류(Python 모듈
//!   최상위 예외 등)는 모든 케이스의 런타임 오류다. 프로세스가 죽으면(메모리 초과로 죽은 Node·세그폴트·시간 초과로
//!   죽임) 실행기가 그 케이스를 판정하고 남은 케이스로 하네스를 다시 띄운다.
//! - 인자는 케이스마다 새로 만든다(풀이가 인자를 고쳐도 다음 케이스에 번지지 않는다). Java는 케이스마다 `new Solution()`.
//! - 깊은 재귀: Python 재귀 한도 10^6·64MB 스레드, JavaScript 워커 스택 64MB, Java 256MB 스레드, C++는 실행기의
//!   `RLIMIT_STACK`.
//!
//! 언어별 함수 모양: Python `def name(a: list[int]) -> int`, JavaScript `function name(a)`(JSDoc 타입),
//! Java `class Solution { public long name(long[] a) }`, C++ `long long name(vector<long long> a)`(전역 함수, 값으로 받는다).
//! `int`는 64비트(Java `long`, C++ `long long`).

use super::{Language, Signature, ValueType, cpp, java, javascript, python};

/// 언어별 (풀이 파일, 하네스 파일) 이름 — 러너 실행기가 같은 이름을 쓴다.
pub fn files(language: Language) -> (&'static str, &'static str) {
    match language {
        Language::Python => ("solution.py", "harness.py"),
        Language::Javascript => ("solution.js", "harness.js"),
        Language::Java => ("Solution.java", "PromptlabMain.java"),
        Language::Cpp => ("solution.cpp", "harness.cpp"),
    }
}

/// 빈 함수 틀(응시자가 채운다).
pub fn solution_template(language: Language, sig: &Signature) -> String {
    match language {
        Language::Python => python::template(sig),
        Language::Javascript => javascript::template(sig),
        Language::Java => java::template(sig),
        Language::Cpp => cpp::template(sig),
    }
}

/// 하네스 소스(위 규약).
pub fn harness(language: Language, sig: &Signature) -> String {
    match language {
        Language::Python => python::harness(sig),
        Language::Javascript => javascript::harness(sig),
        Language::Java => java::harness(sig),
        Language::Cpp => cpp::harness(sig),
    }
}

/// 문자열 리터럴로 넣을 수 있는 서명 타입 목록(`"int[]", "int"`).
pub(super) fn quoted_types<'a>(types: impl Iterator<Item = &'a ValueType>) -> String {
    types
        .map(|t| format!("\"{}\"", t.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}
