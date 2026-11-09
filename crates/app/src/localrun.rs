//! 로컬 실행(#97): 응시자 PC의 Docker Desktop에서 러너 이미지로 「실행」(응시자 입력 한 번)·「예제 테스트」를 돈다.
//!
//! 판정기(`promptlab_judge`, `default-features = false`)의 같은 상한 플래그(`docker_run_args`)·러너 이미지
//! digest(`RUNNER_IMAGES`)·하네스를 쓴다 — 로컬 결과와 서버 채점이 같은 실행 환경이다. 앱은 공개 예제만 가지므로
//! 로컬 실행에 히든 테스트는 없다.
//!
//! docker CLI 오류(데몬 꺼짐·미설치·이미지 없음·Windows 컨테이너 모드·권한·시간 초과)는 [`DockerError`]의 사람 말로
//! 바꾼다([`explain`]). 시험은 `tests/localrun.rs`(docker 없이)·`tests/local_docker.rs`(`#[ignore]`, 호스트 docker).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use promptlab_judge::{Judge, JudgeError, RunOutcome, RunRequest, Status};
use promptlab_shared::coding::{
    self, Language, Param, PublicCodingProblem, Signature, ValueType, runner_image,
};
use promptlab_shared::log::{LocalRunKind, LogEvent};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// Docker Desktop 설치 안내(Windows).
pub const INSTALL_URL: &str = "https://docs.docker.com/desktop/setup/install/windows-install/";
/// 설치 안내 옆 라이선스 한 줄(대표 결정, spec #93 대장 확인 4).
pub const LICENSE_NOTE: &str = "Docker Desktop은 개인 사용은 무료이고, 일정 규모 이상 기업의 업무용 사용은 유료 요금제가 필요합니다.";
/// Docker Desktop이 Windows에 docker CLI를 두는 곳(설치 직후 PATH가 아직 안 바뀐 경우).
pub const WINDOWS_DOCKER: &str = r"C:\Program Files\Docker\Docker\resources\bin\docker.exe";
/// `docker version`·`image inspect` 같은 짧은 호출의 시간 한도.
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
/// 「실행」 결과에 보이는 stdout·stderr 앞부분.
const OUTPUT_SHOWN: usize = 4000;

/// 로컬 실행을 못 한 까닭(사람 말).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DockerError {
    #[error("Docker가 설치되어 있지 않습니다. Docker Desktop을 설치한 뒤 다시 확인해 주세요.")]
    NotInstalled,
    #[error(
        "Docker Desktop이 실행 중이 아닙니다. Docker Desktop을 켜고 엔진이 「Running」이 될 때까지 기다린 뒤 다시 확인해 주세요."
    )]
    DaemonDown,
    #[error(
        "Docker에 접근할 권한이 없습니다. 이 계정을 docker 그룹(Windows는 docker-users)에 넣고 다시 로그인해 주세요."
    )]
    PermissionDenied,
    #[error(
        "Docker Desktop이 Windows 컨테이너 모드입니다. 트레이 아이콘에서 「Switch to Linux containers」를 눌러 주세요."
    )]
    WindowsContainers,
    #[error(
        "{} 실행 이미지가 이 PC에 없습니다. 응시 전 점검에서 받거나 「다시 확인」을 눌러 주세요.",
        .0.display_name()
    )]
    ImageMissing(Language),
    #[error("실행 이미지를 받지 못했습니다. 인터넷 연결을 확인하고 다시 시도해 주세요. ({0})")]
    PullFailed(String),
    #[error(
        "Docker가 제때 답하지 않았습니다. Docker Desktop이 바쁘거나 멈췄을 수 있습니다 — 다시 시도해 주세요."
    )]
    TimedOut,
    #[error("입력이 올바르지 않습니다: {0}")]
    BadInput(String),
    #[error("로컬 실행에 실패했습니다: {0}")]
    Failed(String),
}

/// docker CLI 출력(오류 문구)을 사람 말로 옮긴다. 이미지 이름이 보이면 그 언어.
pub fn explain(raw: &str) -> DockerError {
    let low = raw.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| low.contains(n));
    if has(&[
        "docker를 띄우지 못했다",
        "program not found",
        "no such file or directory (os error 2)",
        "cannot find the file specified. (os error 2)",
        "is not recognized as an internal or external command",
    ]) {
        return DockerError::NotInstalled;
    }
    if has(&["permission denied while trying to connect"]) {
        return DockerError::PermissionDenied;
    }
    if has(&[
        "cannot connect to the docker daemon",
        "error during connect",
        "dockerdesktoplinuxengine",
        "docker_engine",
        "is the docker daemon running",
        "docker desktop is not running",
        "check if the server supports the requested api version",
    ]) {
        return DockerError::DaemonDown;
    }
    if has(&[
        "no matching manifest for windows",
        "operating system \"linux\" cannot be used on this platform",
        "image operating system",
    ]) {
        return DockerError::WindowsContainers;
    }
    if has(&["unable to find image", "no such image"]) {
        let language = Language::ALL
            .into_iter()
            .find(|l| low.contains(&format!("promptlab-runner-{}", l.as_str())))
            .unwrap_or(Language::Python);
        return DockerError::ImageMissing(language);
    }
    DockerError::Failed(promptlab_judge::tail_text(raw.trim(), 300))
}

/// docker CLI를 찾는다: PATH → (Windows) Docker Desktop 기본 설치 경로. 못 찾으면 `None`.
pub fn find_docker() -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        "docker.exe"
    } else {
        "docker"
    };
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .map(|dir| dir.join(exe))
        .find(|p| p.is_file())
        .or_else(|| {
            let p = Path::new(WINDOWS_DOCKER);
            (cfg!(windows) && p.is_file()).then(|| p.to_path_buf())
        })
}

/// 이미지 받기 진행: docker pull 줄에서 층 수와 끝난 층 수를 센다.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PullProgress {
    layers: Vec<String>,
    done: Vec<String>,
}

impl PullProgress {
    pub fn new() -> Self {
        Self::default()
    }

    /// `docker pull` 출력 한 줄(`<층>: Pulling fs layer`·`<층>: Pull complete`·`<층>: Already exists` …).
    pub fn line(&mut self, line: &str) {
        let Some((id, status)) = line.split_once(": ") else {
            return;
        };
        let id = id.trim();
        if id.len() < 8 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
            return;
        }
        if !self.layers.iter().any(|l| l == id) {
            self.layers.push(id.to_owned());
        }
        let status = status.trim();
        if (status.starts_with("Pull complete") || status.starts_with("Already exists"))
            && !self.done.iter().any(|l| l == id)
        {
            self.done.push(id.to_owned());
        }
    }

    pub fn layers(&self) -> usize {
        self.layers.len()
    }

    pub fn done(&self) -> usize {
        self.done.len()
    }

    /// 「층 3/7」. 층을 아직 모르면 「준비 중」.
    pub fn text(&self) -> String {
        if self.layers.is_empty() {
            "준비 중".to_owned()
        } else {
            format!("층 {}/{}", self.done(), self.layers())
        }
    }
}

/// 실행 결과의 케이스 한 줄(화면 표시용).
#[derive(Debug, Clone, PartialEq)]
pub struct CaseLine {
    /// 인자(JSON).
    pub args: String,
    /// 기대 반환값(예제 테스트만).
    pub expected: Option<String>,
    /// 반환값(JSON).
    pub got: Option<String>,
    pub status: Status,
    /// 예제 테스트에서 맞았는가.
    pub ok: Option<bool>,
    pub message: Option<String>,
    pub ms: u64,
}

/// 로컬 실행 한 번의 결과.
#[derive(Debug, Clone, PartialEq)]
pub struct RunReport {
    pub kind: LocalRunKind,
    pub language: Language,
    /// 컴파일 오류 메시지(있으면 모든 케이스가 돌지 않았다).
    pub compile_error: Option<String>,
    pub cases: Vec<CaseLine>,
    pub stdout: String,
    pub stderr: String,
    /// 벽시계 상한으로 멈췄다.
    pub wall_killed: bool,
}

impl RunReport {
    /// 예제 테스트에서 맞은 수.
    pub fn passed(&self) -> u32 {
        self.cases.iter().filter(|c| c.ok == Some(true)).count() as u32
    }

    pub fn total(&self) -> u32 {
        self.cases.len() as u32
    }

    /// 짧은 요약(「예제 2/3 통과」·「컴파일 오류」·「시간 초과」·「반환값 [0,1]」).
    pub fn summary(&self) -> String {
        if self.compile_error.is_some() {
            return "컴파일 오류".to_owned();
        }
        match self.kind {
            LocalRunKind::Examples => format!("예제 {}/{} 통과", self.passed(), self.total()),
            LocalRunKind::Run => match self.cases.first() {
                Some(c) if c.status == Status::Ok => {
                    format!("반환값 {}", c.got.as_deref().unwrap_or("null"))
                }
                Some(c) => status_text(c.status).to_owned(),
                None => "결과 없음".to_owned(),
            },
        }
    }

    /// 과정 로그 사건([`LogEvent::LocalRun`]) — 종류·통과 수(예제만)·요약. 코드·입력·출력은 싣지 않는다
    /// (코드는 바로 앞 스냅숏에 있다).
    pub fn event(&self) -> LogEvent {
        let examples = self.kind == LocalRunKind::Examples;
        LogEvent::LocalRun {
            kind: self.kind,
            passed: examples.then(|| self.passed()),
            total: examples.then(|| self.total()),
            summary: self.summary(),
        }
    }
}

/// 실행 실패의 과정 로그 사건(Docker를 못 썼을 때도 「눌렀다」는 남긴다).
pub fn failed_event(kind: LocalRunKind, error: &DockerError) -> LogEvent {
    let summary = match error {
        DockerError::NotInstalled => "Docker 없음",
        DockerError::DaemonDown => "Docker 꺼짐",
        DockerError::PermissionDenied => "Docker 권한 없음",
        DockerError::WindowsContainers => "Windows 컨테이너 모드",
        DockerError::ImageMissing(_) => "이미지 없음",
        DockerError::PullFailed(_) => "이미지 받기 실패",
        DockerError::TimedOut => "Docker 응답 없음",
        DockerError::BadInput(_) => "입력 오류",
        DockerError::Failed(_) => "실행 실패",
    };
    LogEvent::LocalRun {
        kind,
        passed: None,
        total: None,
        summary: summary.to_owned(),
    }
}

/// 케이스 상태의 사람 말.
pub fn status_text(status: Status) -> &'static str {
    match status {
        Status::Ok => "반환함",
        Status::CompileError => "컴파일 오류",
        Status::RuntimeError => "실행 오류",
        Status::TimeLimit => "시간 초과",
        Status::MemoryLimit => "메모리 초과",
        Status::OutputLimit => "출력 초과",
        Status::NotRun => "돌지 못함",
    }
}

/// 「실행」 입력 칸의 글을 인자 배열로 읽는다. 인자가 하나면 값 하나만 써도 된다(`[1,2,3]` 그대로).
/// 여러 인자면 JSON 배열(`[[2,7,11,15], 9]`). 서명 타입과 맞아야 한다.
pub fn parse_input(sig: &Signature, text: &str) -> Result<Vec<Value>, DockerError> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(sig.params.iter().map(|p| p.ty.default_value()).collect());
    }
    let value: Value = serde_json::from_str(text)
        .map_err(|e| DockerError::BadInput(format!("JSON으로 읽지 못했다 — {e}")))?;
    let args = match (&value, sig.params.len()) {
        (_, 1) if sig.params[0].ty.accepts(&value) => vec![value],
        (Value::Array(items), _) => items.clone(),
        _ => {
            return Err(DockerError::BadInput(format!(
                "인자 {}개를 JSON 배열로 써 주세요(예: {})",
                sig.params.len(),
                input_example(sig)
            )));
        }
    };
    if args.len() != sig.params.len() {
        return Err(DockerError::BadInput(format!(
            "인자는 {}개입니다(받은 것 {}개, 예: {})",
            sig.params.len(),
            args.len(),
            input_example(sig)
        )));
    }
    for (p, v) in sig.params.iter().zip(&args) {
        if !p.ty.accepts(v) {
            return Err(DockerError::BadInput(format!(
                "{}은(는) {} 타입이어야 합니다",
                p.name, p.ty
            )));
        }
    }
    Ok(args)
}

/// 입력 칸 예시(서명의 기본값 배열).
pub fn input_example(sig: &Signature) -> String {
    let args: Vec<Value> = sig.params.iter().map(|p| p.ty.default_value()).collect();
    serde_json::to_string(&args).unwrap_or_default()
}

/// 판정기 결과 → 화면 줄.
pub fn report(
    kind: LocalRunKind,
    language: Language,
    cases: &[Vec<Value>],
    expected: Option<(ValueType, &[Value])>,
    outcome: &RunOutcome,
) -> RunReport {
    let ok = expected.map(|(ty, exp)| promptlab_judge::passed(ty, exp, outcome));
    let compile_error = outcome
        .compile
        .as_ref()
        .filter(|c| !c.ok)
        .map(|c| c.message.clone());
    let lines = cases
        .iter()
        .zip(&outcome.cases)
        .enumerate()
        .map(|(i, (args, c))| CaseLine {
            args: serde_json::to_string(args).unwrap_or_default(),
            expected: expected.and_then(|(_, e)| e.get(i)).map(|v| v.to_string()),
            got: c.value.as_ref().map(Value::to_string),
            status: c.status,
            ok: ok.as_ref().and_then(|o| o.get(i).copied()),
            message: c.message.clone(),
            ms: c.ms,
        })
        .collect();
    RunReport {
        kind,
        language,
        compile_error,
        cases: lines,
        stdout: promptlab_judge::tail_text(&outcome.stdout, OUTPUT_SHOWN),
        stderr: promptlab_judge::tail_text(&outcome.stderr, OUTPUT_SHOWN),
        wall_killed: outcome.wall_killed,
    }
}

/// 응시 전 점검의 「빈 하네스」 서명: `solve(x: int) -> int`.
pub fn trial_signature() -> Signature {
    Signature {
        name: "solve".to_owned(),
        params: vec![Param {
            name: "x".to_owned(),
            ty: ValueType::Int,
        }],
        returns: ValueType::Int,
    }
}

/// 응시자 PC의 docker.
#[derive(Clone)]
pub struct LocalDocker {
    docker: PathBuf,
    judge: Judge,
}

impl LocalDocker {
    /// docker CLI를 찾아 쓴다. 못 찾으면 [`DockerError::NotInstalled`].
    pub fn detect() -> Result<Self, DockerError> {
        find_docker()
            .map(Self::with_cli)
            .ok_or(DockerError::NotInstalled)
    }

    /// 이 docker CLI로(시험·고정 경로).
    pub fn with_cli(docker: PathBuf) -> Self {
        let judge = Judge::new(docker.to_string_lossy().into_owned(), 1);
        Self { docker, judge }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.docker);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        promptlab_judge::hide_console(&mut cmd);
        cmd
    }

    /// 짧은 docker 호출 하나. 성공이면 stdout.
    async fn call(&self, args: &[&str]) -> Result<String, DockerError> {
        let out = tokio::time::timeout(PROBE_TIMEOUT, self.command().args(args).output())
            .await
            .map_err(|_| DockerError::TimedOut)?
            .map_err(|e| explain(&format!("docker를 띄우지 못했다: {e}")))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
        } else {
            Err(explain(&String::from_utf8_lossy(&out.stderr)))
        }
    }

    /// 데몬이 도는지 확인한다. 성공이면 엔진 판.
    pub async fn probe(&self) -> Result<String, DockerError> {
        let os = self
            .call(&["version", "--format", "{{.Server.Os}} {{.Server.Version}}"])
            .await?;
        if os.starts_with("windows") {
            return Err(DockerError::WindowsContainers);
        }
        Ok(os.split_whitespace().nth(1).unwrap_or_default().to_owned())
    }

    /// 언어의 러너 이미지(고정 digest)가 이 PC에 있나.
    pub async fn has_image(&self, language: Language) -> Result<bool, DockerError> {
        let reference = runner_image(language).reference();
        match self
            .call(&["image", "inspect", "--format", "{{.Id}}", &reference])
            .await
        {
            Ok(_) => Ok(true),
            Err(DockerError::ImageMissing(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// 러너 이미지를 받는다(digest 고정 — docker가 내용을 digest로 확인한다). 줄마다 `progress`.
    pub async fn pull(
        &self,
        language: Language,
        mut progress: impl FnMut(&PullProgress),
    ) -> Result<(), DockerError> {
        let reference = runner_image(language).reference();
        let mut child = self
            .command()
            .args(["pull", &reference])
            .spawn()
            .map_err(|e| explain(&format!("docker를 띄우지 못했다: {e}")))?;
        let stdout = child.stdout.take().expect("stdout");
        let mut lines = BufReader::new(stdout).lines();
        let mut state = PullProgress::new();
        while let Ok(Some(line)) = lines.next_line().await {
            state.line(&line);
            progress(&state);
        }
        let out = child
            .wait_with_output()
            .await
            .map_err(|e| DockerError::Failed(e.to_string()))?;
        if out.status.success() {
            return Ok(());
        }
        match explain(&String::from_utf8_lossy(&out.stderr)) {
            DockerError::Failed(why) | DockerError::PullFailed(why) => {
                Err(DockerError::PullFailed(why))
            }
            DockerError::ImageMissing(_) => Err(DockerError::PullFailed(
                "이미지 저장소에서 찾지 못했다".to_owned(),
            )),
            other => Err(other),
        }
    }

    async fn run(&self, req: &RunRequest) -> Result<RunOutcome, DockerError> {
        match self.judge.run(req).await {
            Ok(o) => Ok(o),
            Err(JudgeError::Invalid(why)) => Err(DockerError::BadInput(why)),
            Err(JudgeError::Docker(why)) => Err(explain(&why)),
        }
    }

    /// 「예제 테스트」: 공개 예제 전부를 돌려 기대값과 맞춰 본다.
    pub async fn run_examples(
        &self,
        problem: &PublicCodingProblem,
        language: Language,
        code: &str,
    ) -> Result<RunReport, DockerError> {
        let cases: Vec<Vec<Value>> = problem.examples.iter().map(|e| e.args.clone()).collect();
        let expected: Vec<Value> = problem
            .examples
            .iter()
            .map(|e| e.expected.clone())
            .collect();
        let req = RunRequest {
            language,
            code: code.to_owned(),
            signature: problem.signature.clone(),
            cases: cases.clone(),
            case_time_ms: Some(problem.limits.case_time_ms),
        };
        let outcome = self.run(&req).await?;
        Ok(report(
            LocalRunKind::Examples,
            language,
            &cases,
            Some((problem.signature.returns, &expected)),
            &outcome,
        ))
    }

    /// 「실행」: 응시자가 넣은 인자로 한 번 돌려 반환값·출력을 보인다.
    pub async fn run_input(
        &self,
        problem: &PublicCodingProblem,
        language: Language,
        code: &str,
        input: &str,
    ) -> Result<RunReport, DockerError> {
        let args = parse_input(&problem.signature, input)?;
        let cases = vec![args];
        let req = RunRequest {
            language,
            code: code.to_owned(),
            signature: problem.signature.clone(),
            cases: cases.clone(),
            case_time_ms: Some(problem.limits.case_time_ms),
        };
        let outcome = self.run(&req).await?;
        Ok(report(LocalRunKind::Run, language, &cases, None, &outcome))
    }

    /// 응시 전 점검: 빈 하네스(서명 [`trial_signature`]의 틀)를 한 번 돌려 컴파일까지 되는지 본다.
    pub async fn trial(&self, language: Language) -> Result<(), DockerError> {
        let sig = trial_signature();
        let req = RunRequest {
            language,
            code: coding::solution_template(language, &sig),
            signature: sig,
            cases: vec![vec![Value::from(0)]],
            case_time_ms: None,
        };
        let outcome = self.run(&req).await?;
        let compiled = outcome.compile.as_ref().is_none_or(|c| c.ok)
            && outcome
                .cases
                .iter()
                .all(|c| !matches!(c.status, Status::CompileError | Status::NotRun));
        if compiled {
            Ok(())
        } else {
            Err(DockerError::Failed(
                "빈 틀을 실행하지 못했다 — 실행 이미지가 손상됐을 수 있다".to_owned(),
            ))
        }
    }
}
