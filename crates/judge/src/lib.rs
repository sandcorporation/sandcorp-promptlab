//! 판정기 — 풀이 하나를 언어의 고정 러너 이미지에서 돌려 케이스별 결과를 낸다(spec #93 3절).
//!
//! - 이미지: 언어 → [`promptlab_shared::coding::RUNNER_IMAGES`]의 고정 참조. 요청으로 바꿀 수 없다.
//! - 컨테이너 상한: [`limits`]의 상수만([`docker_run_args`]). 이미지 pull·볼륨·네트워크 생성·exec·bind 마운트를 쓰지
//!   않는다 — 입력은 stdin, 작업 공간은 tmpfs. 그래서 docker CLI가 `DOCKER_HOST=tcp://<소켓 프록시>`로도 돈다
//!   (운영은 본문 검사 프록시를 거친다, #95).
//! - 벽시계: 케이스 수 × 케이스 시간 + 컴파일 10초, 최대 60초([`limits::wall_clock`]). 넘으면 `docker kill`, 끝난
//!   케이스까지만 인정하고 나머지는 실패.
//! - 동시 실행: [`Judge`] 안 세마포어(기본 [`limits::MAX_CONCURRENT`]).
//!
//! 응시 앱은 HTTP 없이 이 lib을 그대로 쓴다(`default-features = false`).

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use promptlab_runner::{CaseStatus, RunEvent, RunInput, RunLimits};
use promptlab_shared::coding::{self, Language, Signature, ValueType};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use tokio::process::Command;
use tokio::sync::Semaphore;

pub use promptlab_runner::{CaseStatus as Status, RunEvent as Event, tail as tail_text};

/// docker CLI를 띄울 때 Windows 콘솔 창이 뜨지 않게 한다(응시 앱은 콘솔 없는 GUI 프로세스, #97).
pub fn hide_console(cmd: &mut Command) {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// 컨테이너 상한 상수(spec #93 3절 값). 바꾸면 spec과 `docs`·PR에 함께 적는다.
pub mod limits {
    use std::time::Duration;

    /// `--network`
    pub const NETWORK: &str = "none";
    /// `--cpus`
    pub const CPUS: &str = "1";
    /// `--memory`·`--memory-swap`(스왑 없음)
    pub const MEMORY: &str = "512m";
    /// `--pids-limit`
    pub const PIDS: u32 = 64;
    /// `--tmpfs` 작업 폴더(실행 파일을 둔다 — `exec`)
    pub const TMPFS_WORK: &str = "/work:size=64m,exec,mode=1777";
    /// `--tmpfs` 임시 폴더
    pub const TMPFS_TMP: &str = "/tmp:size=16m,mode=1777";
    /// `--user`(nobody)
    pub const USER: &str = "65534:65534";
    /// `--ulimit nofile`
    pub const NOFILE: u32 = 64;
    /// 풀이 출력(stdout·stderr 합계) 상한 — 넘으면 출력 초과.
    pub const OUTPUT_BYTES: usize = 1024 * 1024;
    /// 컴파일 시간.
    pub const COMPILE_TIME_MS: u64 = 10_000;
    /// 제출 전체 벽시계 상한의 최댓값.
    pub const MAX_WALL: Duration = Duration::from_secs(60);
    /// 판정기 안 동시 실행 수.
    pub const MAX_CONCURRENT: usize = 2;
    /// 러너 stdout(결과 줄)을 읽는 최대 바이트 — 판정기 메모리 보호.
    pub const MAX_RUNNER_STDOUT: usize = 8 * 1024 * 1024;
    /// 풀이 코드 최대 크기.
    pub const MAX_CODE_BYTES: usize = 64 * 1024;
    /// 한 번에 돌리는 케이스 최대 수(예제 + 히든).
    pub const MAX_CASES: usize =
        promptlab_shared::coding::MAX_HIDDEN + promptlab_shared::coding::MAX_EXAMPLES;

    /// 제출 전체 벽시계 = 케이스 수 × 케이스 시간 + 컴파일 시간, 최대 [`MAX_WALL`].
    pub fn wall_clock(cases: usize, case_time_ms: u64) -> Duration {
        let ms = (cases as u64)
            .saturating_mul(case_time_ms)
            .saturating_add(COMPILE_TIME_MS);
        Duration::from_millis(ms).min(MAX_WALL)
    }
}

/// `docker run` 인자(`docker` 뒤). 이미지·플래그는 여기 상수뿐이다.
pub fn docker_run_args(image: &str, container_name: &str) -> Vec<String> {
    use limits::*;
    [
        "run",
        "--rm",
        "-i",
        "--pull",
        "never",
        "--name",
        container_name,
        "--network",
        NETWORK,
        "--cpus",
        CPUS,
        "--memory",
        MEMORY,
        "--memory-swap",
        MEMORY,
        "--pids-limit",
        &PIDS.to_string(),
        "--read-only",
        "--tmpfs",
        TMPFS_WORK,
        "--tmpfs",
        TMPFS_TMP,
        "--user",
        USER,
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--ulimit",
        &format!("nofile={NOFILE}:{NOFILE}"),
        image,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// 실행 요청 — HTTP `POST /run` 본문이자 앱의 로컬 실행 입력.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRequest {
    pub language: Language,
    pub code: String,
    /// 하네스를 만들 서명.
    pub signature: Signature,
    /// 케이스별 인자 배열(기대값은 넣지 않는다 — 비교는 부른 쪽이 [`passed`]로).
    pub cases: Vec<Vec<Value>>,
    /// 케이스당 시간. 없으면 기본 2초.
    #[serde(default)]
    pub case_time_ms: Option<u64>,
}

/// 컴파일 결과.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompileInfo {
    pub ok: bool,
    pub message: String,
}

/// 케이스 하나의 결과.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaseResult {
    pub status: CaseStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub ms: u64,
}

/// 실행 결과 — 케이스는 요청 순서 그대로(빠짐 없음).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunOutcome {
    pub compile: Option<CompileInfo>,
    pub cases: Vec<CaseResult>,
    /// 풀이의 stdout·stderr 앞부분.
    pub stdout: String,
    pub stderr: String,
    /// 벽시계 상한으로 컨테이너를 죽였는가.
    pub wall_killed: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum JudgeError {
    /// 요청이 규칙 밖(HTTP 422).
    #[error("{0}")]
    Invalid(String),
    /// docker를 부르지 못했거나 러너가 출력 없이 끝남(HTTP 503).
    #[error("docker: {0}")]
    Docker(String),
}

impl RunRequest {
    pub fn validate(&self) -> Result<(), JudgeError> {
        self.signature
            .validate()
            .map_err(|e| JudgeError::Invalid(e.to_string()))?;
        if self.code.len() > limits::MAX_CODE_BYTES {
            return Err(JudgeError::Invalid(format!(
                "코드는 {}바이트까지다",
                limits::MAX_CODE_BYTES
            )));
        }
        if self.cases.is_empty() || self.cases.len() > limits::MAX_CASES {
            return Err(JudgeError::Invalid(format!(
                "케이스는 1~{}개다",
                limits::MAX_CASES
            )));
        }
        let t = self.case_time();
        if !(coding::MIN_CASE_TIME_MS..=coding::MAX_CASE_TIME_MS).contains(&t) {
            return Err(JudgeError::Invalid(format!(
                "케이스당 시간 {t}ms는 범위 밖이다"
            )));
        }
        for (i, args) in self.cases.iter().enumerate() {
            let ok = args.len() == self.signature.params.len()
                && self
                    .signature
                    .params
                    .iter()
                    .zip(args)
                    .all(|(p, v)| p.ty.accepts(v));
            if !ok {
                return Err(JudgeError::Invalid(format!(
                    "케이스 {}의 인자가 서명과 맞지 않는다",
                    i + 1
                )));
            }
        }
        let total: usize = self
            .cases
            .iter()
            .map(|a| {
                serde_json::to_string(a)
                    .map(|s| s.len())
                    .unwrap_or(usize::MAX)
            })
            .sum();
        if total > coding::MAX_TOTAL_CASE_BYTES {
            return Err(JudgeError::Invalid("케이스 합계가 너무 크다".into()));
        }
        Ok(())
    }

    pub fn case_time(&self) -> u64 {
        self.case_time_ms.unwrap_or(coding::DEFAULT_CASE_TIME_MS)
    }

    /// 컨테이너 stdin에 넣을 덩어리.
    pub fn run_input(&self) -> RunInput {
        RunInput {
            code: self.code.clone(),
            harness: coding::harness(self.language, &self.signature),
            cases: self.cases.clone(),
            limits: RunLimits {
                case_time_ms: self.case_time(),
                memory_mb: coding::MEMORY_LIMIT_MB,
                compile_time_ms: limits::COMPILE_TIME_MS,
                output_bytes: limits::OUTPUT_BYTES,
            },
        }
    }
}

/// 러너 stdout 줄들을 결과로 모은다. 빠진 케이스: 벽시계로 죽였으면 처음 빠진 것이 시간 초과, 그 뒤는 돌지 못함;
/// 아니면 처음 빠진 것이 런타임 오류.
pub fn collect(events: &[RunEvent], cases: usize, wall_killed: bool) -> RunOutcome {
    let mut out = RunOutcome {
        compile: None,
        cases: (0..cases)
            .map(|_| CaseResult {
                status: CaseStatus::NotRun,
                value: None,
                message: None,
                ms: 0,
            })
            .collect(),
        stdout: String::new(),
        stderr: String::new(),
        wall_killed,
    };
    let mut seen = vec![false; cases];
    let mut fatal = None;
    for ev in events {
        match ev {
            RunEvent::Compile { ok, message, .. } => {
                out.compile = Some(CompileInfo {
                    ok: *ok,
                    message: message.clone(),
                })
            }
            RunEvent::Case {
                index,
                status,
                value,
                message,
                ms,
            } if *index < cases => {
                out.cases[*index] = CaseResult {
                    status: *status,
                    value: value.clone(),
                    message: message.clone(),
                    ms: *ms,
                };
                seen[*index] = true;
            }
            RunEvent::Case { .. } => {}
            RunEvent::Done { stdout, stderr } => {
                out.stdout = stdout.clone();
                out.stderr = stderr.clone();
            }
            RunEvent::Fatal { message } => fatal = Some(message.clone()),
        }
    }
    if let Some(first) = seen.iter().position(|s| !s) {
        let c = &mut out.cases[first];
        if wall_killed {
            c.status = CaseStatus::TimeLimit;
        } else {
            c.status = CaseStatus::RuntimeError;
            c.message = Some(fatal.unwrap_or_else(|| "러너가 결과 없이 끝났다".into()));
        }
    }
    out
}

/// 케이스별 통과 — 반환값이 있고 기대값과 같으면 통과([`coding::values_match`]).
pub fn passed(returns: ValueType, expected: &[Value], outcome: &RunOutcome) -> Vec<bool> {
    expected
        .iter()
        .zip(&outcome.cases)
        .map(|(e, c)| {
            c.status == CaseStatus::Ok
                && c.value
                    .as_ref()
                    .is_some_and(|v| coding::values_match(returns, e, v))
        })
        .collect()
}

/// docker CLI로 러너를 돌리는 판정기. `DOCKER_HOST` 등 docker 환경변수는 그대로 따른다.
#[derive(Clone)]
pub struct Judge {
    docker: String,
    permits: Arc<Semaphore>,
}

impl Default for Judge {
    fn default() -> Self {
        Self::new("docker", limits::MAX_CONCURRENT)
    }
}

impl Judge {
    /// `docker`: docker CLI 경로, `concurrency`: 동시 실행 수.
    pub fn new(docker: impl Into<String>, concurrency: usize) -> Self {
        Self {
            docker: docker.into(),
            permits: Arc::new(Semaphore::new(concurrency.max(1))),
        }
    }

    /// 풀이를 돌린다(요청 검증 → 세마포어 → `docker run` → 벽시계 → 결과).
    pub async fn run(&self, req: &RunRequest) -> Result<RunOutcome, JudgeError> {
        req.validate()?;
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| JudgeError::Docker("판정기가 닫혔다".into()))?;
        let image = coding::runner_image(req.language).reference();
        let input = serde_json::to_vec(&req.run_input()).expect("입력 직렬화");
        let wall = limits::wall_clock(req.cases.len(), req.case_time());
        self.run_container(&image, input, req.cases.len(), wall)
            .await
    }

    async fn run_container(
        &self,
        image: &str,
        input: Vec<u8>,
        cases: usize,
        wall: Duration,
    ) -> Result<RunOutcome, JudgeError> {
        let name = format!("promptlab-run-{}", uuid::Uuid::new_v4().simple());
        let mut cmd = Command::new(&self.docker);
        hide_console(&mut cmd);
        let mut child = cmd
            .args(docker_run_args(image, &name))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| JudgeError::Docker(format!("docker를 띄우지 못했다: {e}")))?;
        let mut stdin = child.stdin.take().expect("stdin");
        tokio::spawn(async move {
            let _ = stdin.write_all(&input).await;
            let _ = stdin.shutdown().await;
        });
        let mut stderr = child.stderr.take().expect("stderr");
        let err_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = (&mut stderr).take(64 * 1024).read_to_end(&mut buf).await;
            String::from_utf8_lossy(&buf).to_string()
        });
        let stdout = child.stdout.take().expect("stdout");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<RunEvent>();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout.take(limits::MAX_RUNNER_STDOUT as u64)).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(ev) = serde_json::from_str::<RunEvent>(&line)
                    && tx.send(ev).is_err()
                {
                    return;
                }
            }
        });
        // 결과 줄을 읽는 동안 벽시계를 잰다 — 넘으면 kill, 그때까지 읽은 줄만 쓴다.
        let deadline = tokio::time::Instant::now() + wall;
        let mut events = Vec::new();
        let mut wall_killed = false;
        loop {
            tokio::select! {
                ev = rx.recv() => match ev {
                    Some(ev) => events.push(ev),
                    None => break,
                },
                _ = tokio::time::sleep_until(deadline), if !wall_killed => {
                    wall_killed = true;
                    self.kill(&name).await;
                }
                _ = tokio::time::sleep_until(deadline + Duration::from_secs(10)), if wall_killed => break,
            }
        }
        let status = match tokio::time::timeout(Duration::from_secs(10), child.wait()).await {
            Ok(s) => s.ok(),
            Err(_) => {
                let _ = child.kill().await;
                None
            }
        };
        let docker_err = err_task.await.unwrap_or_default();
        if events.is_empty() && !wall_killed {
            let code = status.and_then(|s| s.code());
            return Err(JudgeError::Docker(format!(
                "러너가 출력 없이 끝났다(종료 코드 {code:?}): {}",
                promptlab_runner::tail(docker_err.trim(), 1000)
            )));
        }
        Ok(collect(&events, cases, wall_killed))
    }

    async fn kill(&self, name: &str) {
        let mut cmd = Command::new(&self.docker);
        hide_console(&mut cmd);
        let _ = cmd
            .args(["kill", name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 벽시계는_케이스_시간_합에_컴파일을_더하고_60초에서_자른다() {
        assert_eq!(limits::wall_clock(5, 2000), Duration::from_secs(20));
        assert_eq!(limits::wall_clock(40, 2000), Duration::from_secs(60));
    }

    #[test]
    fn docker_인자는_상수만() {
        let a = docker_run_args("img:1", "n").join(" ");
        for want in [
            "--pull never",
            "--network none",
            "--cpus 1",
            "--memory 512m",
            "--memory-swap 512m",
            "--pids-limit 64",
            "--read-only",
            "--tmpfs /work:size=64m,exec,mode=1777",
            "--tmpfs /tmp:size=16m,mode=1777",
            "--user 65534:65534",
            "--cap-drop ALL",
            "--security-opt no-new-privileges",
            "--ulimit nofile=64:64",
        ] {
            assert!(a.contains(want), "{want} 없음: {a}");
        }
        // 바인드 마운트·볼륨·특권·네트워크 생성은 쓰지 않는다(소켓 프록시가 막는다).
        for banned in [
            "-v ",
            "--volume",
            "--mount",
            "--privileged",
            "--cap-add",
            "--network host",
        ] {
            assert!(!a.contains(banned), "{banned} 있음");
        }
        assert!(a.ends_with("img:1"));
    }

    #[test]
    fn 빠진_케이스는_벽시계면_시간_초과_뒤는_돌지_못함() {
        let ev = vec![RunEvent::Case {
            index: 0,
            status: CaseStatus::Ok,
            value: Some(Value::from(1)),
            message: None,
            ms: 3,
        }];
        let o = collect(&ev, 3, true);
        assert_eq!(o.cases[0].status, CaseStatus::Ok);
        assert_eq!(o.cases[1].status, CaseStatus::TimeLimit);
        assert_eq!(o.cases[2].status, CaseStatus::NotRun);
        let o = collect(&ev, 2, false);
        assert_eq!(o.cases[1].status, CaseStatus::RuntimeError);
    }
}
