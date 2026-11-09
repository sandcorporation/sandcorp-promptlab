//! 코딩 문제 형식(spec #93 1절) — 규칙의 정본 한 곳. 응시 앱·대시보드(wasm32)·서버·판정기가 같이 쓴다.
//!
//! - 함수 서명([`Signature`]): 함수 이름 + 인자(이름·[`ValueType`]) + 반환 타입. 타입은 닫힌 집합이다.
//! - 테스트 케이스([`TestCase`]): `{"args": [...], "expected": ...}`. 비교는 JSON 같음, `double`은 상대·절대 1e-6([`values_match`]).
//! - 문제([`CodingProblem`]): 히든 테스트가 들어 있어 **직렬화할 수 없다**. 밖으로는 [`CodingProblem::public_view`]의
//!   [`PublicCodingProblem`](설명·서명·예제·4개 언어 틀)만 보낸다.
//! - 4개 언어 solution 틀·하네스는 서명에서 자동 생성한다([`solution_template`]·[`harness`]). 하네스 규약은 [`codegen`] 문서.
//! - 러너 이미지 판은 [`RUNNER_IMAGES`] 한 곳.
//!
//! ```compile_fail
//! // 히든 테스트가 든 문제는 직렬화할 수 없다.
//! fn leak(p: &promptlab_shared::coding::CodingProblem) -> String {
//!     serde_json::to_string(p).unwrap()
//! }
//! ```

pub mod codegen;
mod cpp;
mod java;
mod javascript;
mod python;

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use codegen::{harness, solution_template};

/// 풀이 언어. 판·컴파일러는 러너 이미지가 고정한다(Python 3.12·Node 22·Java 21·C++17 g++).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    Python,
    Javascript,
    Java,
    Cpp,
}

impl Language {
    pub const ALL: [Language; 4] = [Self::Python, Self::Javascript, Self::Java, Self::Cpp];

    /// API·이미지 이름에 쓰는 이름(`python`·`javascript`·`java`·`cpp`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::Javascript => "javascript",
            Self::Java => "java",
            Self::Cpp => "cpp",
        }
    }

    /// 화면에 보이는 이름.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Python => "Python 3.12",
            Self::Javascript => "JavaScript (Node 22)",
            Self::Java => "Java 21",
            Self::Cpp => "C++17",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.as_str() == s)
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 인자·반환 타입 — 닫힌 집합. `int`는 64비트(JavaScript 때문에 안전 정수 범위 안만 받는다).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum ValueType {
    #[serde(rename = "int")]
    Int,
    #[serde(rename = "double")]
    Double,
    #[serde(rename = "bool")]
    Bool,
    #[serde(rename = "string")]
    String,
    #[serde(rename = "int[]")]
    IntArray,
    #[serde(rename = "double[]")]
    DoubleArray,
    #[serde(rename = "bool[]")]
    BoolArray,
    #[serde(rename = "string[]")]
    StringArray,
    #[serde(rename = "int[][]")]
    IntMatrix,
    #[serde(rename = "string[][]")]
    StringMatrix,
}

/// 타입의 원소(차원을 벗긴 것).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scalar {
    Int,
    Double,
    Bool,
    String,
}

impl ValueType {
    pub const ALL: [ValueType; 10] = [
        Self::Int,
        Self::Double,
        Self::Bool,
        Self::String,
        Self::IntArray,
        Self::DoubleArray,
        Self::BoolArray,
        Self::StringArray,
        Self::IntMatrix,
        Self::StringMatrix,
    ];

    pub fn scalar(self) -> Scalar {
        match self {
            Self::Int | Self::IntArray | Self::IntMatrix => Scalar::Int,
            Self::Double | Self::DoubleArray => Scalar::Double,
            Self::Bool | Self::BoolArray => Scalar::Bool,
            Self::String | Self::StringArray | Self::StringMatrix => Scalar::String,
        }
    }

    /// 배열 차원(0·1·2).
    pub fn dims(self) -> u8 {
        match self {
            Self::Int | Self::Double | Self::Bool | Self::String => 0,
            Self::IntArray | Self::DoubleArray | Self::BoolArray | Self::StringArray => 1,
            Self::IntMatrix | Self::StringMatrix => 2,
        }
    }

    /// 서명 표기(`int`·`int[]`·`string[][]` …).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Int => "int",
            Self::Double => "double",
            Self::Bool => "bool",
            Self::String => "string",
            Self::IntArray => "int[]",
            Self::DoubleArray => "double[]",
            Self::BoolArray => "bool[]",
            Self::StringArray => "string[]",
            Self::IntMatrix => "int[][]",
            Self::StringMatrix => "string[][]",
        }
    }

    /// 이 타입의 가장 단순한 값(`0`·`0.0`·`false`·`""`·빈 배열) — 틀 컴파일 점검(#96)이 케이스 인자로 쓴다.
    pub fn default_value(self) -> Value {
        if self.dims() > 0 {
            return Value::Array(Vec::new());
        }
        match self.scalar() {
            Scalar::Int => Value::from(0),
            Scalar::Double => Value::from(0.0),
            Scalar::Bool => Value::Bool(false),
            Scalar::String => Value::String(String::new()),
        }
    }

    /// 값이 이 타입에 맞는가(`int`는 JS 안전 정수 범위, `double`은 유한한 수).
    pub fn accepts(self, v: &Value) -> bool {
        fn scalar_ok(s: Scalar, v: &Value) -> bool {
            match s {
                Scalar::Int => v
                    .as_i64()
                    .is_some_and(|n| (-MAX_SAFE_INT..=MAX_SAFE_INT).contains(&n)),
                Scalar::Double => v.as_f64().is_some_and(f64::is_finite),
                Scalar::Bool => v.is_boolean(),
                Scalar::String => v.is_string(),
            }
        }
        let s = self.scalar();
        match self.dims() {
            0 => scalar_ok(s, v),
            1 => v
                .as_array()
                .is_some_and(|a| a.iter().all(|x| scalar_ok(s, x))),
            _ => v.as_array().is_some_and(|rows| {
                rows.iter().all(|r| {
                    r.as_array()
                        .is_some_and(|a| a.iter().all(|x| scalar_ok(s, x)))
                })
            }),
        }
    }
}

impl fmt::Display for ValueType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// JavaScript 안전 정수 상한(2^53 − 1). `int` 값은 이 범위 안만 받는다.
pub const MAX_SAFE_INT: i64 = (1 << 53) - 1;

/// 인자 하나.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Param {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: ValueType,
}

/// 함수 서명.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Signature {
    /// 함수 이름 — 4개 언어에서 그대로 쓴다.
    pub name: String,
    pub params: Vec<Param>,
    pub returns: ValueType,
}

/// 테스트 케이스 하나. 예제 케이스는 공개, 히든 케이스는 서버·워커·판정기 밖으로 나가지 않는다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TestCase {
    pub args: Vec<Value>,
    pub expected: Value,
}

/// 케이스당 시간 기본값(밀리초) — 언어 보정 없이 하나(spec 1절).
pub const DEFAULT_CASE_TIME_MS: u64 = 2_000;
/// 문제가 고를 수 있는 케이스당 시간 범위.
pub const MIN_CASE_TIME_MS: u64 = 500;
pub const MAX_CASE_TIME_MS: u64 = 10_000;
/// 풀이 프로세스의 메모리 상한(MB). 언어 런타임이 이 값으로 상한을 건다(`MemoryError`·`-Xmx`·`--max-old-space-size`·`RLIMIT_AS`).
pub const MEMORY_LIMIT_MB: u32 = 256;
/// 예제 케이스 최소 개수.
pub const MIN_EXAMPLES: usize = 1;
/// 히든 케이스 최소·최대 개수.
pub const MIN_HIDDEN: usize = 5;
pub const MAX_HIDDEN: usize = 50;
/// 예제 케이스 최대 개수.
pub const MAX_EXAMPLES: usize = 10;
/// 케이스 하나의 입력(`args` JSON) 상한(바이트).
pub const MAX_CASE_INPUT_BYTES: usize = 64 * 1024;
/// 모든 케이스(입력 + 기대값) JSON 합계 상한(바이트).
pub const MAX_TOTAL_CASE_BYTES: usize = 1024 * 1024;
/// 제출 코드 하나의 상한(바이트) — 넘으면 서버가 `422`(판정기 상한과 같다).
pub const MAX_CODE_BYTES: usize = 64 * 1024;
/// 인자 최대 개수.
pub const MAX_PARAMS: usize = 8;
/// 식별자(함수·인자 이름) 최대 길이.
pub const MAX_IDENT_LEN: usize = 40;
/// `double` 비교 허용 오차(상대·절대).
pub const DOUBLE_TOLERANCE: f64 = 1e-6;

/// 실행 제한.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Limits {
    /// 케이스당 시간(밀리초).
    #[serde(default = "default_case_time")]
    pub case_time_ms: u64,
}

fn default_case_time() -> u64 {
    DEFAULT_CASE_TIME_MS
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            case_time_ms: DEFAULT_CASE_TIME_MS,
        }
    }
}

/// 코딩 문제 — 히든 테스트와 모범 답안이 들어 있어 `Serialize`가 없다(위 compile_fail 문서 시험).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CodingProblem {
    pub id: String,
    pub title: String,
    /// 마크다운 설명.
    pub description: String,
    pub signature: Signature,
    pub examples: Vec<TestCase>,
    pub hidden: Vec<TestCase>,
    /// 모범 답안 하나([`CodingProblem::reference_language`]로 쓴 것).
    pub reference_solution: String,
    /// 모범 답안의 언어(기본 Python).
    #[serde(default)]
    pub reference_language: Language,
    #[serde(default)]
    pub limits: Limits,
}

/// 언어 하나의 solution 틀.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LanguageTemplate {
    pub language: Language,
    pub code: String,
}

/// 밖으로 보내는 문제 — 설명·서명·예제·제한·4개 언어 틀만.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PublicCodingProblem {
    pub id: String,
    pub title: String,
    pub description: String,
    pub signature: Signature,
    pub examples: Vec<TestCase>,
    pub limits: Limits,
    pub templates: Vec<LanguageTemplate>,
}

impl CodingProblem {
    pub fn public_view(&self) -> PublicCodingProblem {
        PublicCodingProblem {
            id: self.id.clone(),
            title: self.title.clone(),
            description: self.description.clone(),
            signature: self.signature.clone(),
            examples: self.examples.clone(),
            limits: self.limits,
            templates: templates(&self.signature),
        }
    }

    /// 출제 규칙 검증 — 서명·케이스 개수·값 타입·크기·제한.
    pub fn validate(&self) -> Result<(), CodingError> {
        self.signature.validate()?;
        if self.reference_solution.trim().is_empty() {
            return Err(CodingError::MissingReference);
        }
        if !(MIN_CASE_TIME_MS..=MAX_CASE_TIME_MS).contains(&self.limits.case_time_ms) {
            return Err(CodingError::CaseTime(self.limits.case_time_ms));
        }
        validate_cases(&self.signature, &self.examples, &self.hidden)
    }
}

/// 4개 언어 틀(언어 순서 [`Language::ALL`]).
pub fn templates(sig: &Signature) -> Vec<LanguageTemplate> {
    Language::ALL
        .into_iter()
        .map(|language| LanguageTemplate {
            language,
            code: solution_template(language, sig),
        })
        .collect()
}

/// 검증 실패 — 서버는 422로 돌려준다.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CodingError {
    #[error(
        "이름 「{0}」은(는) 쓸 수 없다 — 영문 소문자로 시작하는 영문·숫자·밑줄 {MAX_IDENT_LEN}자 이하, 언어 예약어 아님"
    )]
    BadIdentifier(String),
    #[error("인자 이름 「{0}」이(가) 겹친다")]
    DuplicateParam(String),
    #[error("인자는 {MAX_PARAMS}개까지다")]
    TooManyParams,
    #[error("예제 케이스는 {MIN_EXAMPLES}~{MAX_EXAMPLES}개다(지금 {0}개)")]
    ExampleCount(usize),
    #[error("히든 케이스는 {MIN_HIDDEN}~{MAX_HIDDEN}개다(지금 {0}개)")]
    HiddenCount(usize),
    #[error("{case}: 인자 수가 서명과 다르다(서명 {expected}개, 케이스 {actual}개)")]
    ArgCount {
        case: String,
        expected: usize,
        actual: usize,
    },
    #[error(
        "{case}: 「{name}」 값이 {ty} 타입이 아니다(int는 ±(2^53−1) 안 정수, double은 유한한 수)"
    )]
    ArgType {
        case: String,
        name: String,
        ty: ValueType,
    },
    #[error("{case}: 기대값이 반환 타입 {ty}이(가) 아니다")]
    ExpectedType { case: String, ty: ValueType },
    #[error("{case}: 입력이 {MAX_CASE_INPUT_BYTES}바이트를 넘는다")]
    CaseTooLarge { case: String },
    #[error("케이스 합계가 {MAX_TOTAL_CASE_BYTES}바이트를 넘는다")]
    TotalTooLarge,
    #[error("모범 답안이 없다")]
    MissingReference,
    #[error("케이스당 시간 {0}ms는 범위({MIN_CASE_TIME_MS}~{MAX_CASE_TIME_MS}ms) 밖이다")]
    CaseTime(u64),
}

/// 4개 언어 예약어·하네스가 쓰는 이름 — 함수·인자 이름으로 받지 않는다.
const RESERVED: &[&str] = &[
    // Python
    "and",
    "as",
    "assert",
    "async",
    "await",
    "break",
    "class",
    "continue",
    "def",
    "del",
    "elif",
    "else",
    "except",
    "finally",
    "for",
    "from",
    "global",
    "if",
    "import",
    "in",
    "is",
    "lambda",
    "nonlocal",
    "not",
    "or",
    "pass",
    "raise",
    "return",
    "try",
    "while",
    "with",
    "yield",
    "match",
    "case",
    "type",
    "print",
    "input",
    "len",
    "list",
    "dict",
    "set",
    "str",
    "int",
    "float",
    "bool",
    "object",
    "range",
    "sum",
    "min",
    "max",
    "map",
    "filter",
    "sorted",
    "abs",
    "open",
    "exec",
    "eval",
    "id",
    "hash",
    "iter",
    "next",
    "self",
    "sys",
    "json",
    "os",
    "math",
    // JavaScript
    "arguments",
    "await",
    "case",
    "catch",
    "const",
    "debugger",
    "default",
    "delete",
    "do",
    "enum",
    "eval",
    "export",
    "extends",
    "function",
    "implements",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "package",
    "private",
    "protected",
    "public",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "typeof",
    "var",
    "void",
    "undefined",
    "require",
    "module",
    "process",
    "console",
    "globalThis",
    "fs",
    // Java
    "abstract",
    "boolean",
    "byte",
    "char",
    "double",
    "final",
    "float",
    "goto",
    "long",
    "native",
    "short",
    "strictfp",
    "synchronized",
    "throws",
    "transient",
    "volatile",
    "record",
    "sealed",
    "permits",
    "yield",
    "var",
    "main",
    "Solution",
    "String",
    "Object",
    "Math",
    // C++
    "alignas",
    "alignof",
    "asm",
    "auto",
    "bitand",
    "bitor",
    "compl",
    "concept",
    "consteval",
    "constexpr",
    "constinit",
    "const_cast",
    "co_await",
    "co_return",
    "co_yield",
    "decltype",
    "dynamic_cast",
    "explicit",
    "extern",
    "friend",
    "inline",
    "mutable",
    "namespace",
    "noexcept",
    "not_eq",
    "nullptr",
    "operator",
    "or_eq",
    "register",
    "reinterpret_cast",
    "requires",
    "signed",
    "sizeof",
    "static_assert",
    "static_cast",
    "struct",
    "template",
    "thread_local",
    "typedef",
    "typeid",
    "typename",
    "union",
    "unsigned",
    "using",
    "virtual",
    "wchar_t",
    "xor",
    "xor_eq",
    "std",
    "vector",
    "string",
    "true",
    "false",
];

fn ident_ok(s: &str) -> bool {
    let mut chars = s.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_lowercase());
    first_ok
        && s.len() <= MAX_IDENT_LEN
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !s.starts_with("pl_")
        && !s.contains("__")
        && !RESERVED.contains(&s)
}

impl Signature {
    pub fn validate(&self) -> Result<(), CodingError> {
        if !ident_ok(&self.name) {
            return Err(CodingError::BadIdentifier(self.name.clone()));
        }
        if self.params.len() > MAX_PARAMS {
            return Err(CodingError::TooManyParams);
        }
        for (i, p) in self.params.iter().enumerate() {
            if !ident_ok(&p.name) || p.name == self.name {
                return Err(CodingError::BadIdentifier(p.name.clone()));
            }
            if self.params[..i].iter().any(|q| q.name == p.name) {
                return Err(CodingError::DuplicateParam(p.name.clone()));
            }
        }
        Ok(())
    }
}

/// 케이스 개수·타입·크기 검증.
pub fn validate_cases(
    sig: &Signature,
    examples: &[TestCase],
    hidden: &[TestCase],
) -> Result<(), CodingError> {
    if !(MIN_EXAMPLES..=MAX_EXAMPLES).contains(&examples.len()) {
        return Err(CodingError::ExampleCount(examples.len()));
    }
    if !(MIN_HIDDEN..=MAX_HIDDEN).contains(&hidden.len()) {
        return Err(CodingError::HiddenCount(hidden.len()));
    }
    let mut total = 0usize;
    let labelled = examples
        .iter()
        .enumerate()
        .map(|(i, c)| (format!("예제 {}", i + 1), c))
        .chain(
            hidden
                .iter()
                .enumerate()
                .map(|(i, c)| (format!("히든 {}", i + 1), c)),
        );
    for (case, c) in labelled {
        if c.args.len() != sig.params.len() {
            return Err(CodingError::ArgCount {
                case,
                expected: sig.params.len(),
                actual: c.args.len(),
            });
        }
        for (p, v) in sig.params.iter().zip(&c.args) {
            if !p.ty.accepts(v) {
                return Err(CodingError::ArgType {
                    case,
                    name: p.name.clone(),
                    ty: p.ty,
                });
            }
        }
        if !sig.returns.accepts(&c.expected) {
            return Err(CodingError::ExpectedType {
                case,
                ty: sig.returns,
            });
        }
        let input = json_len(&Value::Array(c.args.clone()));
        if input > MAX_CASE_INPUT_BYTES {
            return Err(CodingError::CaseTooLarge { case });
        }
        total += input + json_len(&c.expected);
    }
    if total > MAX_TOTAL_CASE_BYTES {
        return Err(CodingError::TotalTooLarge);
    }
    Ok(())
}

fn json_len(v: &Value) -> usize {
    serde_json::to_string(v)
        .map(|s| s.len())
        .unwrap_or(usize::MAX)
}

/// 풀이 출력이 기대값과 같은가 — JSON 같음. `double`(원소)은 상대 또는 절대 오차 [`DOUBLE_TOLERANCE`] 안이면 같다.
/// `int`는 정수끼리만 같다(`3`과 `3.0`은 다르다).
pub fn values_match(ty: ValueType, expected: &Value, actual: &Value) -> bool {
    fn scalar_eq(s: Scalar, e: &Value, a: &Value) -> bool {
        match s {
            Scalar::Double => match (e.as_f64(), a.as_f64()) {
                (Some(e), Some(a)) if e.is_finite() && a.is_finite() => {
                    let diff = (e - a).abs();
                    diff <= DOUBLE_TOLERANCE || diff <= DOUBLE_TOLERANCE * e.abs()
                }
                _ => false,
            },
            Scalar::Int => match (e.as_i64(), a.as_i64()) {
                (Some(e), Some(a)) => e == a,
                _ => false,
            },
            Scalar::Bool | Scalar::String => e == a,
        }
    }
    fn arr_eq(s: Scalar, e: &Value, a: &Value, f: fn(Scalar, &Value, &Value) -> bool) -> bool {
        match (e.as_array(), a.as_array()) {
            (Some(e), Some(a)) => e.len() == a.len() && e.iter().zip(a).all(|(x, y)| f(s, x, y)),
            _ => false,
        }
    }
    fn row_eq(s: Scalar, e: &Value, a: &Value) -> bool {
        arr_eq(s, e, a, scalar_eq)
    }
    let s = ty.scalar();
    match ty.dims() {
        0 => scalar_eq(s, expected, actual),
        1 => arr_eq(s, expected, actual, scalar_eq),
        _ => arr_eq(s, expected, actual, row_eq),
    }
}

/// 러너 이미지 하나 — 판정기·앱은 [`RunnerImage::reference`]로 받는다(digest가 있으면 digest).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunnerImage {
    pub language: Language,
    /// 저장소 이름(`ghcr.io/sandcorporation/promptlab-runner-<언어>`).
    pub name: &'static str,
    pub tag: &'static str,
    /// 멀티아키 매니페스트 목록 digest(`sha256:…`). 첫 push 전에는 `None`(#95가 채운다).
    pub digest: Option<&'static str>,
}

impl RunnerImage {
    /// `docker run`에 넘길 참조 — `name@digest`, 아직 없으면 `name:tag`.
    pub fn reference(&self) -> String {
        match self.digest {
            Some(d) => format!("{}@{}", self.name, d),
            None => format!("{}:{}", self.name, self.tag),
        }
    }
}

/// 러너 이미지 판(태그). 이미지를 바꾸면 올리고 `scripts/build-runners.sh`로 다시 굽는다.
pub const RUNNER_TAG: &str = "v1";

/// 언어별 러너 이미지 — 판의 정본 한 곳.
pub const RUNNER_IMAGES: [RunnerImage; 4] = [
    RunnerImage {
        language: Language::Python,
        name: "ghcr.io/sandcorporation/promptlab-runner-python",
        tag: RUNNER_TAG,
        digest: Some("sha256:3bce463c6163288bc759f8d746b0ca841d5e2566a952d2df96cc8c2b2bc9a6ff"),
    },
    RunnerImage {
        language: Language::Javascript,
        name: "ghcr.io/sandcorporation/promptlab-runner-javascript",
        tag: RUNNER_TAG,
        digest: Some("sha256:55d546fc6272aa541adcd74682187410922fb7af8d4e9f5a6005b186265c3943"),
    },
    RunnerImage {
        language: Language::Java,
        name: "ghcr.io/sandcorporation/promptlab-runner-java",
        tag: RUNNER_TAG,
        digest: Some("sha256:02359a983563a0dce0dd7bdb2718bc8574197869fbcc4c0a96d15bc23eeff762"),
    },
    RunnerImage {
        language: Language::Cpp,
        name: "ghcr.io/sandcorporation/promptlab-runner-cpp",
        tag: RUNNER_TAG,
        digest: Some("sha256:109e6d54d7e456ec36fd2d6a409bccab87eaab6ea3dc134204049c22f37ddeb1"),
    },
];

/// 언어의 러너 이미지.
pub fn runner_image(language: Language) -> &'static RunnerImage {
    RUNNER_IMAGES
        .iter()
        .find(|i| i.language == language)
        .expect("RUNNER_IMAGES에 모든 언어가 있다")
}
