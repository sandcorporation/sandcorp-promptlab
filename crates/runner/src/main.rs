//! 러너 실행기(이미지의 진입점). stdin [`RunInput`] → 파일 쓰기 → 컴파일 → 하네스 실행 → stdout [`RunEvent`] 줄.
//!
//! - 케이스 시간은 하네스의 결과 줄 사이 간격으로 잰다. 넘으면 하네스 프로세스 묶음을 죽이고 그 케이스를 시간 초과로,
//!   **남은 케이스로 하네스를 다시 띄운다**(한 케이스의 시간 초과·죽음이 다른 케이스에 번지지 않는다).
//! - 풀이의 stdout·stderr 합계가 상한을 넘으면 그 케이스는 출력 초과(같이 다시 띄운다).
//! - 하네스는 자기 프로세스 묶음(`setpgid`)에서 돌고, 끝나면 묶음 전체를 죽이고 고아를 거둔다(fork 폭탄 뒤 정리).

#[cfg(not(unix))]
fn main() {
    eprintln!("promptlab-runner는 리눅스 컨테이너 안에서만 돈다");
    std::process::exit(2);
}

#[cfg(unix)]
fn main() {
    unix::main();
}

#[cfg(unix)]
mod unix {
    use std::fs;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use promptlab_runner::{
        CaseStatus, KEEP_LOG_BYTES, MAX_MESSAGE_BYTES, RunEvent, RunInput, RunLimits,
        classify_crash, tail, truncate,
    };
    use serde_json::Value;

    /// 하네스가 준비 줄을 낼 때까지 기다리는 시간(런타임 시작·입력 읽기).
    const STARTUP: Duration = Duration::from_secs(10);
    /// 언어 런타임 몫(MB) — `RLIMIT_AS`를 쓰는 언어(Python·C++)에서 풀이 메모리 상한 위에 얹는다.
    const RUNTIME_OVERHEAD_MB: u64 = 64;
    /// 결과 줄 하나 최대 길이.
    const MAX_RESULT_LINE: usize = 4 * 1024 * 1024;

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Lang {
        Python,
        Javascript,
        Java,
        Cpp,
    }

    impl Lang {
        fn from_env() -> Option<Self> {
            match std::env::var("PROMPTLAB_RUNNER_LANG").ok()?.as_str() {
                "python" => Some(Self::Python),
                "javascript" => Some(Self::Javascript),
                "java" => Some(Self::Java),
                "cpp" => Some(Self::Cpp),
                _ => None,
            }
        }

        /// (풀이, 하네스) 파일 — `promptlab_shared::coding::codegen::files`와 같다.
        fn files(self) -> (&'static str, &'static str) {
            match self {
                Self::Python => ("solution.py", "harness.py"),
                Self::Javascript => ("solution.js", "harness.js"),
                Self::Java => ("Solution.java", "PromptlabMain.java"),
                Self::Cpp => ("solution.cpp", "harness.cpp"),
            }
        }

        fn compile(self, mem: u32) -> Vec<String> {
            let v: Vec<&str> = match self {
                Self::Python => vec![
                    "python3",
                    "-B",
                    "-c",
                    "import sys\ncompile(open('solution.py','rb').read(),'solution.py','exec')",
                ],
                Self::Javascript => vec!["node", "--check", "solution.js"],
                Self::Java => {
                    return [
                        "javac",
                        "-encoding",
                        "UTF-8",
                        "-J-XX:+UseSerialGC",
                        "-J-XX:TieredStopAtLevel=1",
                        "-J-XX:-UsePerfData",
                        &format!("-J-Xmx{mem}m"),
                        "-d",
                        "classes",
                        "Solution.java",
                        "PromptlabMain.java",
                    ]
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                }
                Self::Cpp => vec![
                    "g++",
                    "-std=c++17",
                    "-O2",
                    "-pipe",
                    "-o",
                    "main",
                    "harness.cpp",
                ],
            };
            v.into_iter().map(String::from).collect()
        }

        fn run(self, mem: u32) -> Vec<String> {
            match self {
                Self::Python => vec!["python3".into(), "-B".into(), "harness.py".into()],
                Self::Javascript => vec![
                    "node".into(),
                    format!("--max-old-space-size={mem}"),
                    "harness.js".into(),
                ],
                Self::Java => vec![
                    "java".into(),
                    format!("-Xmx{mem}m"),
                    "-XX:+UseSerialGC".into(),
                    "-XX:-UsePerfData".into(),
                    "-cp".into(),
                    "classes".into(),
                    "PromptlabMain".into(),
                ],
                Self::Cpp => vec!["./main".into()],
            }
        }

        /// `RLIMIT_AS`를 거는가(JVM·V8은 가상 메모리를 크게 잡아 못 건다 — 그 둘은 런타임 플래그).
        fn limits_address_space(self) -> bool {
            matches!(self, Self::Python | Self::Cpp)
        }
    }

    fn emit(ev: &RunEvent) {
        let mut out = std::io::stdout().lock();
        let line = serde_json::to_string(ev).expect("사건 직렬화");
        let _ = out.write_all(line.as_bytes());
        let _ = out.write_all(b"\n");
        let _ = out.flush();
    }

    fn ms(d: Duration) -> u64 {
        d.as_millis() as u64
    }

    /// 풀이의 stdout·stderr 모으기 — 앞부분만 남기고 합계를 센다.
    #[derive(Default)]
    struct Logs {
        stdout: Mutex<Vec<u8>>,
        stderr: Mutex<Vec<u8>>,
        /// 이번 하네스 실행의 stderr 끝부분(죽은 까닭 판정).
        stderr_tail: Mutex<Vec<u8>>,
        total: AtomicUsize,
        over: AtomicBool,
    }

    fn pump(mut src: impl Read + Send + 'static, logs: Arc<Logs>, is_err: bool, limit: usize) {
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                let n = match src.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                let total = logs.total.fetch_add(n, Ordering::SeqCst) + n;
                if total > limit {
                    logs.over.store(true, Ordering::SeqCst);
                }
                let keep = if is_err { &logs.stderr } else { &logs.stdout };
                let mut keep = keep.lock().unwrap();
                let room = KEEP_LOG_BYTES.saturating_sub(keep.len());
                keep.extend_from_slice(&buf[..n.min(room)]);
                if is_err {
                    let mut t = logs.stderr_tail.lock().unwrap();
                    t.extend_from_slice(&buf[..n]);
                    let len = t.len();
                    if len > 2 * MAX_MESSAGE_BYTES {
                        t.drain(..len - MAX_MESSAGE_BYTES);
                    }
                }
            }
        });
    }

    fn pipe() -> (OwnedFd, OwnedFd) {
        let mut fds = [0i32; 2];
        // SAFETY: fds는 정수 두 칸 배열이다.
        let r = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
        assert_eq!(r, 0, "pipe2");
        // SAFETY: 방금 만든 fd를 하나씩 소유한다.
        unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) }
    }

    #[cfg(target_env = "gnu")]
    type Resource = libc::__rlimit_resource_t;
    #[cfg(not(target_env = "gnu"))]
    type Resource = libc::c_int;

    fn set_rlimit(res: Resource, v: u64) {
        let lim = libc::rlimit {
            rlim_cur: v as libc::rlim_t,
            rlim_max: v as libc::rlim_t,
        };
        // SAFETY: 값으로 넘긴 구조체를 읽기만 한다.
        unsafe { libc::setrlimit(res, &lim) };
    }

    /// 프로세스 묶음 하나를 띄운다(`setpgid`, 자원 상한). `result_fd`가 있으면 자식의 fd 3으로 놓는다.
    fn spawn(
        argv: &[String],
        work: &Path,
        lang: Lang,
        limits: RunLimits,
        result_fd: Option<i32>,
    ) -> std::io::Result<Child> {
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .current_dir(work)
            .env("HOME", "/tmp")
            .env("TMPDIR", "/tmp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mem_bytes = (limits.memory_mb as u64 + RUNTIME_OVERHEAD_MB) << 20;
        let limit_as = result_fd.is_some() && lang.limits_address_space();
        let stack = (limits.memory_mb as u64) << 20;
        // SAFETY: pre_exec 안에서는 async-signal-safe 호출(setpgid·setrlimit·dup2)만 한다.
        unsafe {
            cmd.pre_exec(move || {
                libc::setpgid(0, 0);
                set_rlimit(libc::RLIMIT_CORE, 0);
                set_rlimit(libc::RLIMIT_FSIZE, 16 << 20);
                if limit_as {
                    set_rlimit(libc::RLIMIT_AS, mem_bytes);
                    set_rlimit(libc::RLIMIT_STACK, stack);
                }
                if let Some(fd) = result_fd
                    && libc::dup2(fd, 3) < 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        cmd.spawn()
    }

    fn kill_group(child: &Child) {
        // SAFETY: 자식이 만든 프로세스 묶음 id(= 자식 pid)에 신호를 보낸다.
        unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    }

    /// 다른 자식(고아가 된 손자 포함)을 거둔다 — 컨테이너 PID 1이라 좀비가 pids 상한을 먹지 않게.
    fn reap() {
        loop {
            let mut st = 0;
            // SAFETY: 상태 정수 하나를 넘긴다.
            let r = unsafe { libc::waitpid(-1, &mut st, libc::WNOHANG) };
            if r <= 0 {
                return;
            }
        }
    }

    /// 컴파일 — (성공, 출력, 걸린 시간).
    fn compile(lang: Lang, work: &Path, limits: RunLimits) -> (bool, String, Duration) {
        let start = Instant::now();
        let argv = lang.compile(limits.memory_mb);
        let mut child = match spawn(&argv, work, lang, limits, None) {
            Ok(c) => c,
            Err(e) => {
                return (
                    false,
                    format!("컴파일러를 띄우지 못했다: {e}"),
                    start.elapsed(),
                );
            }
        };
        drop(child.stdin.take());
        let logs = Arc::new(Logs::default());
        let (out, err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
        // 컴파일러 출력은 풀이 출력 상한에 넣지 않는다.
        pump(out, logs.clone(), true, usize::MAX);
        pump(err, logs.clone(), true, usize::MAX);
        let deadline = start + Duration::from_millis(limits.compile_time_ms);
        let status = loop {
            if let Ok(Some(st)) = child.try_wait() {
                break Some(st);
            }
            if Instant::now() >= deadline {
                kill_group(&child);
                let _ = child.wait();
                break None;
            }
            thread::sleep(Duration::from_millis(20));
        };
        kill_group(&child);
        reap();
        thread::sleep(Duration::from_millis(20));
        let text = String::from_utf8_lossy(&logs.stderr_tail.lock().unwrap()).to_string();
        match status {
            Some(st) if st.success() => (true, String::new(), start.elapsed()),
            Some(_) => (false, tail(&text, MAX_MESSAGE_BYTES), start.elapsed()),
            None => (
                false,
                format!(
                    "컴파일 시간 초과({}초)\n{}",
                    limits.compile_time_ms / 1000,
                    tail(&text, MAX_MESSAGE_BYTES)
                ),
                start.elapsed(),
            ),
        }
    }

    enum Line {
        Result(Value),
        Eof,
    }

    /// 하네스를 한 번 띄워 `cases[from..]`을 돌린다. 다음에 돌 케이스 번호를 돌려준다(끝까지 갔으면 `cases.len()`).
    fn run_once(
        lang: Lang,
        work: &Path,
        limits: RunLimits,
        cases: &[Vec<Value>],
        from: usize,
        logs: &Arc<Logs>,
    ) -> usize {
        let (rd, wr) = pipe();
        use std::os::fd::AsRawFd;
        let mut child = match spawn(
            &lang.run(limits.memory_mb),
            work,
            lang,
            limits,
            Some(wr.as_raw_fd()),
        ) {
            Ok(c) => c,
            Err(e) => {
                emit(&RunEvent::Case {
                    index: from,
                    status: CaseStatus::RuntimeError,
                    value: None,
                    message: Some(format!("하네스를 띄우지 못했다: {e}")),
                    ms: 0,
                });
                return from + 1;
            }
        };
        drop(wr);
        logs.stderr_tail.lock().unwrap().clear();
        let input = serde_json::to_vec(&cases[from..]).expect("케이스 직렬화");
        let mut stdin = child.stdin.take().unwrap();
        thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
        pump(
            child.stdout.take().unwrap(),
            logs.clone(),
            false,
            limits.output_bytes,
        );
        pump(
            child.stderr.take().unwrap(),
            logs.clone(),
            true,
            limits.output_bytes,
        );
        let (tx, rx) = mpsc::channel::<Line>();
        thread::spawn(move || {
            let mut r = BufReader::new(fs::File::from(rd));
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match (&mut r)
                    .take(MAX_RESULT_LINE as u64)
                    .read_until(b'\n', &mut buf)
                {
                    Ok(0) | Err(_) => {
                        let _ = tx.send(Line::Eof);
                        return;
                    }
                    Ok(_) => {
                        let v = serde_json::from_slice::<Value>(&buf).unwrap_or(Value::Null);
                        if tx.send(Line::Result(v)).is_err() {
                            return;
                        }
                    }
                }
            }
        });

        let case_time = Duration::from_millis(limits.case_time_ms);
        let mut next = from;
        let mut ready = false;
        let mut since = Instant::now();
        let finish = |child: &mut Child| {
            kill_group(child);
            let st = child.wait().ok();
            reap();
            st
        };
        loop {
            if next >= cases.len() {
                finish(&mut child);
                return next;
            }
            let budget = if ready { case_time } else { STARTUP };
            if logs.over.load(Ordering::SeqCst) {
                finish(&mut child);
                emit(&RunEvent::Case {
                    index: next,
                    status: CaseStatus::OutputLimit,
                    value: None,
                    message: None,
                    ms: ms(since.elapsed()),
                });
                return next + 1;
            }
            if since.elapsed() >= budget {
                finish(&mut child);
                emit(&RunEvent::Case {
                    index: next,
                    status: CaseStatus::TimeLimit,
                    value: None,
                    message: None,
                    ms: ms(since.elapsed()),
                });
                return next + 1;
            }
            match rx.recv_timeout(Duration::from_millis(20)) {
                Err(RecvTimeoutError::Timeout) => continue,
                Ok(Line::Result(v)) if !ready => {
                    ready = v.get("ready").is_some();
                    since = Instant::now();
                }
                Ok(Line::Result(v)) => {
                    let elapsed = ms(since.elapsed());
                    let (status, value, message) = if let Some(ok) = v.get("ok") {
                        (CaseStatus::Ok, Some(ok.clone()), None)
                    } else {
                        match v.get("error").and_then(Value::as_str) {
                            Some("memory") => (CaseStatus::MemoryLimit, None, None),
                            _ => (
                                CaseStatus::RuntimeError,
                                None,
                                Some(truncate(
                                    v.get("message")
                                        .and_then(Value::as_str)
                                        .unwrap_or("하네스 결과 줄을 읽지 못했다"),
                                    MAX_MESSAGE_BYTES,
                                )),
                            ),
                        }
                    };
                    emit(&RunEvent::Case {
                        index: next,
                        status,
                        value,
                        message,
                        ms: elapsed,
                    });
                    next += 1;
                    since = Instant::now();
                }
                Ok(Line::Eof) | Err(RecvTimeoutError::Disconnected) => {
                    let st = finish(&mut child);
                    // stderr 펌프가 마지막 조각을 넣을 틈.
                    thread::sleep(Duration::from_millis(50));
                    let err =
                        String::from_utf8_lossy(&logs.stderr_tail.lock().unwrap()).to_string();
                    let signal = st.and_then(|s| s.signal());
                    let status = classify_crash(&err, signal);
                    let message = match signal {
                        Some(sig) => {
                            format!("신호 {sig}로 끝났다\n{}", tail(&err, MAX_MESSAGE_BYTES))
                        }
                        None => tail(&err, MAX_MESSAGE_BYTES),
                    };
                    emit(&RunEvent::Case {
                        index: next,
                        status,
                        value: None,
                        message: Some(message),
                        ms: ms(since.elapsed()),
                    });
                    return next + 1;
                }
            }
        }
    }

    fn work_dir() -> PathBuf {
        std::env::var_os("PROMPTLAB_WORK_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/work"))
    }

    pub fn main() {
        // fd 3을 막아 둔다 — 하네스 결과 fd(자식의 3)를 놓을 때 std의 내부 파이프와 겹치지 않게.
        // SAFETY: 경로 상수로 파일을 연다.
        unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };

        let Some(lang) = Lang::from_env() else {
            emit(&RunEvent::Fatal {
                message: "PROMPTLAB_RUNNER_LANG이 없다".into(),
            });
            std::process::exit(2);
        };
        let mut raw = Vec::new();
        let input: RunInput = match std::io::stdin()
            .read_to_end(&mut raw)
            .map_err(|e| e.to_string())
            .and_then(|_| serde_json::from_slice(&raw).map_err(|e| e.to_string()))
        {
            Ok(i) => i,
            Err(e) => {
                emit(&RunEvent::Fatal {
                    message: format!("입력을 읽지 못했다: {e}"),
                });
                std::process::exit(2);
            }
        };
        drop(raw);
        let work = work_dir();
        let (sol, har) = lang.files();
        if let Err(e) = fs::write(work.join(sol), &input.code)
            .and_then(|_| fs::write(work.join(har), &input.harness))
        {
            emit(&RunEvent::Fatal {
                message: format!("작업 폴더에 쓰지 못했다: {e}"),
            });
            std::process::exit(2);
        }

        let limits = input.limits;
        let (ok, message, took) = compile(lang, &work, limits);
        emit(&RunEvent::Compile {
            ok,
            message: message.clone(),
            ms: ms(took),
        });
        let logs = Arc::new(Logs::default());
        if ok {
            let mut next = 0;
            while next < input.cases.len() {
                next = run_once(lang, &work, limits, &input.cases, next, &logs);
                logs.over.store(false, Ordering::SeqCst);
                logs.total.store(0, Ordering::SeqCst);
            }
        } else {
            for index in 0..input.cases.len() {
                emit(&RunEvent::Case {
                    index,
                    status: CaseStatus::CompileError,
                    value: None,
                    message: None,
                    ms: 0,
                });
            }
        }
        let text = |m: &Mutex<Vec<u8>>| String::from_utf8_lossy(&m.lock().unwrap()).to_string();
        emit(&RunEvent::Done {
            stdout: text(&logs.stdout),
            stderr: text(&logs.stderr),
        });
    }
}
