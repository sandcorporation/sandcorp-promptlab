//! 러너 실행기 프로토콜 — 판정기(`promptlab-judge`)·응시 앱이 컨테이너와 주고받는 꼴.
//!
//! - **입력**: 컨테이너 stdin 한 덩어리 [`RunInput`] `{code, harness, cases, limits}`. `cases`는 케이스별 인자 배열뿐
//!   (기대값은 컨테이너에 넣지 않는다).
//! - **출력**: 컨테이너 stdout에 줄 하나 = [`RunEvent`] 하나. 순서: `compile` 하나 → 케이스마다 `case`(번호 순) → `done`.
//!   벽시계로 죽으면 끝난 케이스 줄까지만 남는다.
//!
//! 실행기(bin)는 이미지의 `PROMPTLAB_RUNNER_LANG`(`python`·`javascript`·`java`·`cpp`)으로 언어를 안다 — 언어·플래그를
//! 입력으로 바꿀 수 없다. 하네스 규약(fd 3 결과 줄)은 `promptlab_shared::coding::codegen` 문서.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 컨테이너 stdin 한 덩어리.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunInput {
    /// 응시자 풀이.
    pub code: String,
    /// 서명에서 만든 하네스.
    pub harness: String,
    /// 케이스별 인자 배열.
    pub cases: Vec<Vec<Value>>,
    pub limits: RunLimits,
}

/// 실행기 안 상한. 판정기가 상수로 채운다(`promptlab_judge::limits`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunLimits {
    /// 케이스당 시간(밀리초) — 앞 결과 줄(또는 준비 줄)부터 잰다.
    pub case_time_ms: u64,
    /// 풀이 메모리 상한(MB).
    pub memory_mb: u32,
    /// 컴파일(문법 검사) 시간(밀리초).
    pub compile_time_ms: u64,
    /// 풀이의 stdout·stderr 합계 상한(바이트). 넘으면 그 케이스는 출력 초과.
    pub output_bytes: usize,
}

/// 케이스 판정.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseStatus {
    /// 반환값을 냈다(맞았는지는 밖에서 비교한다).
    Ok,
    CompileError,
    RuntimeError,
    TimeLimit,
    MemoryLimit,
    OutputLimit,
    /// 돌지 못했다(벽시계 kill 뒤 남은 케이스 등).
    NotRun,
}

/// 컨테이너 stdout 줄 하나.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent {
    /// 컴파일(문법 검사) 결과. 실패면 모든 케이스가 `compile_error`.
    Compile {
        ok: bool,
        /// 컴파일러 출력(잘림).
        message: String,
        ms: u64,
    },
    Case {
        index: usize,
        status: CaseStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        ms: u64,
    },
    /// 끝. 풀이의 stdout·stderr 앞부분(앱의 로컬 실행이 보여 준다).
    Done { stdout: String, stderr: String },
    /// 입력을 읽지 못함 등 실행기 자체 오류.
    Fatal { message: String },
}

/// 실행기가 남기는 stdout·stderr 앞부분 길이(바이트).
pub const KEEP_LOG_BYTES: usize = 8 * 1024;
/// 메시지(컴파일·런타임 오류) 최대 길이(바이트).
pub const MAX_MESSAGE_BYTES: usize = 4 * 1024;

/// 문자열을 UTF-8 경계에서 `max` 바이트 안으로 자른다.
pub fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// 끝에서 `max` 바이트(UTF-8 경계).
pub fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    s[start..].to_string()
}

/// 하네스 프로세스가 결과 줄 없이 죽었을 때 그 케이스 판정 — stderr에 메모리 초과 흔적이 있거나 SIGKILL이면 메모리 초과.
pub fn classify_crash(stderr_tail: &str, signal: Option<i32>) -> CaseStatus {
    const OOM_MARKS: [&str; 5] = [
        "MemoryError",
        "OutOfMemoryError",
        "heap out of memory",
        "std::bad_alloc",
        "ERR_WORKER_OUT_OF_MEMORY",
    ];
    if signal == Some(9) || OOM_MARKS.iter().any(|m| stderr_tail.contains(m)) {
        CaseStatus::MemoryLimit
    } else {
        CaseStatus::RuntimeError
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 사건_줄_꼴() {
        let e = RunEvent::Case {
            index: 2,
            status: CaseStatus::TimeLimit,
            value: None,
            message: None,
            ms: 2000,
        };
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"type":"case","index":2,"status":"time_limit","ms":2000}"#
        );
    }

    #[test]
    fn 자르기는_utf8_경계() {
        assert_eq!(truncate("가나다", 4), "가");
        assert_eq!(tail("가나다", 4), "다");
    }

    #[test]
    fn 죽음_판정() {
        assert_eq!(classify_crash("", Some(9)), CaseStatus::MemoryLimit);
        assert_eq!(
            classify_crash(
                "FATAL ERROR: Reached heap limit Allocation failed - JavaScript heap out of memory",
                None
            ),
            CaseStatus::MemoryLimit
        );
        assert_eq!(classify_crash("", Some(11)), CaseStatus::RuntimeError);
    }
}
