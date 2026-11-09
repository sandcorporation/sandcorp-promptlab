//! 시험용 가짜 판정기 — 워커·서버 시험이 [`crate::run_once`]에 끼운다(docker 없이).
//!
//! 실제 판정기의 조건부 동작을 흉내 낸다: 러너 사건(`Compile`·`Case`·`Done`)을 만들어 실제 판정기와 같은
//! [`promptlab_judge::collect`]로 모은다 — 컴파일 오류면 모든 케이스가 `compile_error`, 벽시계 kill이면 처음 빠진
//! 케이스가 시간 초과·나머지 `not_run`. 받은 요청을 모두 기록한다(히든 기대값이 본문에 없는지 시험이 본다).

use std::sync::{Arc, Mutex};

use promptlab_judge::{Event, RunOutcome, RunRequest, Status};
use serde_json::Value;

use crate::{JudgeCallError, JudgeClient};

/// 풀이 흉내: (제출 코드, 케이스 인자) → 반환값. `None`이면 그 케이스에서 런타임 오류.
pub type Solve = dyn Fn(&str, &[Value]) -> Option<Value> + Send + Sync;

/// 가짜 판정기의 동작.
#[derive(Clone)]
pub enum Mode {
    /// 케이스마다 풀이 흉내를 부른다.
    Run,
    /// 컴파일 오류(모든 케이스 `compile_error`).
    CompileError,
    /// 이 언어만 컴파일 오류(다른 언어는 정상) — 모범 답안은 되는데 한 언어 틀만 안 되는 경우를 흉내 낸다.
    CompileErrorIn(promptlab_shared::coding::Language),
    /// 앞 `n`개 케이스만 끝내고 벽시계로 죽였다.
    WallKill(usize),
    /// HTTP 422.
    Rejected,
    /// HTTP 503.
    Unavailable,
}

/// 가짜 판정기. 복제하면 같은 기록·동작을 나눠 쓴다.
#[derive(Clone)]
pub struct FakeJudge {
    solve: Arc<Solve>,
    mode: Arc<Mutex<Mode>>,
    requests: Arc<Mutex<Vec<RunRequest>>>,
}

impl FakeJudge {
    pub fn new(solve: impl Fn(&str, &[Value]) -> Option<Value> + Send + Sync + 'static) -> Self {
        Self {
            solve: Arc::new(solve),
            mode: Arc::new(Mutex::new(Mode::Run)),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn set_mode(&self, mode: Mode) {
        *self.mode.lock().unwrap_or_else(|e| e.into_inner()) = mode;
    }

    /// 지금까지 받은 요청(받은 순서).
    pub fn requests(&self) -> Vec<RunRequest> {
        self.requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn outcome(&self, req: &RunRequest, mode: &Mode) -> RunOutcome {
        let mode = &match mode {
            Mode::CompileErrorIn(l) if *l == req.language => Mode::CompileError,
            Mode::CompileErrorIn(_) => Mode::Run,
            other => other.clone(),
        };
        let n = req.cases.len();
        let mut events = Vec::new();
        if matches!(
            req.language,
            promptlab_shared::coding::Language::Java | promptlab_shared::coding::Language::Cpp
        ) || matches!(mode, Mode::CompileError)
        {
            events.push(Event::Compile {
                ok: !matches!(mode, Mode::CompileError),
                message: if matches!(mode, Mode::CompileError) {
                    "error: expected ';'".into()
                } else {
                    String::new()
                },
                ms: 1,
            });
        }
        let ran = match mode {
            Mode::CompileError => 0,
            Mode::WallKill(k) => (*k).min(n),
            _ => n,
        };
        if matches!(mode, Mode::CompileError) {
            for index in 0..n {
                events.push(Event::Case {
                    index,
                    status: Status::CompileError,
                    value: None,
                    message: None,
                    ms: 0,
                });
            }
        }
        for (index, args) in req.cases.iter().enumerate().take(ran) {
            let (status, value) = match (self.solve)(&req.code, args) {
                Some(v) => (Status::Ok, Some(v)),
                None => (Status::RuntimeError, None),
            };
            events.push(Event::Case {
                index,
                status,
                value,
                message: None,
                ms: 1,
            });
        }
        if !matches!(mode, Mode::WallKill(_)) {
            events.push(Event::Done {
                stdout: String::new(),
                stderr: String::new(),
            });
        }
        promptlab_judge::collect(&events, n, matches!(mode, Mode::WallKill(_)))
    }
}

impl JudgeClient for FakeJudge {
    async fn run(&self, req: &RunRequest) -> Result<RunOutcome, JudgeCallError> {
        self.requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(req.clone());
        let mode = self.mode.lock().unwrap_or_else(|e| e.into_inner()).clone();
        match mode {
            Mode::Rejected => Err(JudgeCallError::Rejected("케이스가 너무 많다".into())),
            Mode::Unavailable => Err(JudgeCallError::Unavailable("HTTP 503".into())),
            other => Ok(self.outcome(req, &other)),
        }
    }
}
