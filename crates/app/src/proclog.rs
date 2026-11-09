//! 과정 로그(#9)의 앱 쪽: 설치 키 보관, 사건을 서명해 디스크에 쌓기(오프라인), 재연결 뒤 순서대로 올리기.
//! 형식·검증 규칙은 [`promptlab_shared::log`].
//!
//! - **키**: 앱 설치마다 Ed25519 키 하나. [`data_dir`]`/device-key.ed25519`에 씨앗(개인키)을 base64url로 둔다
//!   (유닉스는 0600). 키 파일이 없거나 깨졌으면 새 키를 만든다 — 깨진 파일은 `.broken`으로 옮겨 둔다. 키가 바뀌면
//!   서버 다시보기에 「키가 바뀌었다」 신호가 뜬다(판단은 고객사).
//! - **쌓기**: 문항마다 `logs/<배정>-<문항>.json`에 아직 서버가 받지 않은 사건과 로컬 체인 머리를 둔다. 사건을
//!   남길 때마다 파일을 통째로 다시 쓴다(임시 파일 → 이름 바꾸기). 앱을 껐다 켜도 이어진다.
//! - **올리기**([`flush`]): 처음이면 공개키를 등록하고, 쌓인 것을 순번대로 [`MAX_EVENTS_PER_UPLOAD`]개씩 올린다.
//!   서버가 받은 순번까지 지운다. 서버는 이미 받은 순번을 건너뛰므로(멱등) 응답이 끊겨도 다시 올리면 된다.
//!   서버 체인과 어긋났다는 `409`(다시 설치해 로컬 로그가 비었던 경우 등)면 서버 머리를 다시 받아 쌓인 사건을
//!   그 위로 다시 서명해 잇는다.
//! - 로그인 창을 띄우지 않는다(뒤에서 돈다). 네트워크·로그인 문제면 쌓아 둔 채 다음에 다시 한다.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use promptlab_shared::log::{
    Attempt, Chain, EventBody, LogEvent, LogSigner, MAX_EVENTS_PER_UPLOAD, SignedEvent, event_hash,
};
use serde::{Deserialize, Serialize};

use crate::client::{ApiClient, ClientError};

/// 앱 데이터 폴더: `PROMPTLAB_DATA_DIR` → Windows `%LOCALAPPDATA%\promptlab` → macOS
/// `~/Library/Application Support/promptlab` → `$XDG_DATA_HOME/promptlab` → `~/.local/share/promptlab`.
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("PROMPTLAB_DATA_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        home.as_ref()
            .map(|h| h.join("Library").join("Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(".local").join("share")))
    };
    base.unwrap_or_else(|| PathBuf::from(".")).join("promptlab")
}

const KEY_FILE: &str = "device-key.ed25519";

/// 이 설치의 서명 키를 읽는다. 없거나 깨졌으면 새로 만든다(깨진 파일은 `.broken`으로 옮긴다).
pub fn load_or_create_signer(dir: &Path) -> io::Result<LogSigner> {
    let path = dir.join(KEY_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let seed = URL_SAFE_NO_PAD
                .decode(text.trim())
                .ok()
                .and_then(|b| <[u8; 32]>::try_from(b).ok());
            if let Some(seed) = seed {
                return Ok(LogSigner::from_seed(seed));
            }
            std::fs::rename(&path, dir.join(format!("{KEY_FILE}.broken")))?;
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    std::fs::create_dir_all(dir)?;
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| io::Error::other(e.to_string()))?;
    let signer = LogSigner::from_seed(seed);
    write_private(&path, URL_SAFE_NO_PAD.encode(signer.seed()).as_bytes())?;
    Ok(signer)
}

/// 다른 사용자가 읽지 못하게 쓴다(유닉스 0600; Windows는 사용자 폴더 `%LOCALAPPDATA%`의 권한을 따른다).
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(bytes)
}

/// 디스크에 두는 문항 로그.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Saved {
    attempt: Attempt,
    /// 로컬 체인 머리(다음에 서명할 순번·마지막 해시).
    chain: Chain,
    /// 서버가 아직 받지 않은 사건(순번순).
    pending: Vec<SignedEvent>,
}

/// 응시 문항 하나의 과정 로그.
#[derive(Debug)]
pub struct ProcessLog {
    signer: Arc<LogSigner>,
    path: PathBuf,
    saved: Saved,
    registered: bool,
}

fn seq_of(event: &SignedEvent) -> Option<u64> {
    serde_json::from_str::<EventBody>(&event.body)
        .ok()
        .map(|b| b.seq)
}

impl ProcessLog {
    /// `dir/logs/`에서 문항 로그를 연다(없으면 새로). 앱을 다시 켜도 쌓인 것이 이어진다.
    pub fn open(dir: &Path, signer: Arc<LogSigner>, attempt: Attempt) -> io::Result<ProcessLog> {
        let path = log_path(dir, &attempt);
        let saved = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Saved>(&bytes)
                .ok()
                .filter(|s| s.attempt == attempt),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        let saved = saved.unwrap_or(Saved {
            attempt,
            chain: Chain::default(),
            pending: Vec::new(),
        });
        Ok(ProcessLog {
            signer,
            path,
            saved,
            registered: false,
        })
    }

    pub fn attempt(&self) -> &Attempt {
        &self.saved.attempt
    }

    /// 이 로그에 서명한 사건이 하나라도 있는가(앱을 다시 켜 이어 쓰는 중인가).
    pub fn has_history(&self) -> bool {
        self.saved.chain.next_seq > 0
    }

    /// 서버가 아직 받지 않은 사건 수.
    pub fn pending(&self) -> usize {
        self.saved.pending.len()
    }

    /// 사건 하나를 서명해 쌓는다(디스크에 바로 쓴다). `at_ms`는 앱 시계의 유닉스 밀리초.
    pub fn record(&mut self, at_ms: i64, event: LogEvent) -> io::Result<()> {
        let chain = &mut self.saved.chain;
        let signed = self.signer.seal(
            &self.saved.attempt,
            chain.next_seq,
            &chain.head,
            at_ms,
            event,
        );
        chain.next_seq += 1;
        chain.head = event_hash(&signed.body);
        self.saved.pending.push(signed);
        self.save()
    }

    /// 이 문항에 서명한 사건 수(다음 순번). 시험 제출의 `log_seq`다.
    pub fn next_seq(&self) -> u64 {
        self.saved.chain.next_seq
    }

    /// 지금 시각으로 쌓는다.
    pub fn record_now(&mut self, event: LogEvent) -> io::Result<()> {
        self.record(now_ms(), event)
    }

    fn save(&self) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec(&self.saved).map_err(io::Error::other)?,
        )?;
        std::fs::rename(&tmp, &self.path)
    }

    /// 서버가 `head.next_seq` 앞까지 받았다 — 그만큼 지운다.
    fn acknowledge(&mut self, head: &Chain) -> io::Result<()> {
        let before = self.saved.pending.len();
        self.saved
            .pending
            .retain(|e| seq_of(e).is_none_or(|seq| seq >= head.next_seq));
        if self.saved.pending.len() != before {
            self.save()?;
        }
        Ok(())
    }

    /// 쌓인 사건을 서버 머리 위로 다시 서명해 잇는다(시각·내용은 그대로).
    fn rebase(&mut self, head: &Chain) -> io::Result<()> {
        let events: Vec<(i64, LogEvent)> = self
            .saved
            .pending
            .iter()
            .filter_map(|e| serde_json::from_str::<EventBody>(&e.body).ok())
            .map(|b| (b.at_ms, b.event))
            .collect();
        self.saved.pending.clear();
        self.saved.chain = head.clone();
        for (at_ms, event) in events {
            let chain = &mut self.saved.chain;
            let signed = self.signer.seal(
                &self.saved.attempt,
                chain.next_seq,
                &chain.head,
                at_ms,
                event,
            );
            chain.next_seq += 1;
            chain.head = event_hash(&signed.body);
            self.saved.pending.push(signed);
        }
        self.save()
    }
}

fn log_path(dir: &Path, attempt: &Attempt) -> PathBuf {
    dir.join("logs").join(format!(
        "{}-{}.json",
        attempt.assignment_id, attempt.position
    ))
}

/// 공유하는 문항 로그.
pub type SharedLog = Arc<Mutex<ProcessLog>>;

/// 이 설치의 과정 로그들. 앱을 켤 때 디스크에 남은(아직 못 올린) 로그를 다시 읽어 올린다.
/// 같은 문항은 언제나 같은 [`SharedLog`]를 준다(한 파일을 둘이 쓰지 않게).
#[derive(Debug)]
pub struct LogBook {
    dir: PathBuf,
    signer: Arc<LogSigner>,
    logs: Mutex<Vec<SharedLog>>,
}

impl LogBook {
    /// `dir`(보통 [`data_dir`])의 설치 키를 읽고(없으면 만들고) 못 올린 로그를 모은다.
    pub fn open(dir: &Path) -> io::Result<LogBook> {
        let signer = Arc::new(load_or_create_signer(dir)?);
        let mut logs = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir.join("logs")) {
            for entry in entries.flatten() {
                let Ok(bytes) = std::fs::read(entry.path()) else {
                    continue;
                };
                let Ok(saved) = serde_json::from_slice::<Saved>(&bytes) else {
                    continue;
                };
                if !saved.pending.is_empty() {
                    let log = ProcessLog::open(dir, signer.clone(), saved.attempt)?;
                    logs.push(Arc::new(Mutex::new(log)));
                }
            }
        }
        Ok(LogBook {
            dir: dir.to_owned(),
            signer,
            logs: Mutex::new(logs),
        })
    }

    /// 문항의 로그(이미 열었으면 그것).
    pub fn attempt(&self, attempt: Attempt) -> io::Result<SharedLog> {
        let mut logs = self.logs.lock().expect("로그 묶음 잠금");
        if let Some(log) = logs
            .iter()
            .find(|l| l.lock().expect("과정 로그 잠금").attempt() == &attempt)
        {
            return Ok(log.clone());
        }
        let log = Arc::new(Mutex::new(ProcessLog::open(
            &self.dir,
            self.signer.clone(),
            attempt,
        )?));
        logs.push(log.clone());
        Ok(log)
    }

    /// 모든 문항의 못 올린 사건 수.
    pub fn pending(&self) -> usize {
        self.snapshot()
            .iter()
            .map(|l| l.lock().expect("과정 로그 잠금").pending())
            .sum()
    }

    fn snapshot(&self) -> Vec<SharedLog> {
        self.logs.lock().expect("로그 묶음 잠금").clone()
    }

    /// 쌓인 것이 있는 문항마다 올린다. 하나가 실패해도 나머지는 해 보고 처음 오류를 준다.
    pub async fn flush_all(&self, api: &ApiClient) -> Result<usize, FlushError> {
        let mut sent = 0;
        let mut first_error = None;
        for log in self.snapshot() {
            if log.lock().expect("과정 로그 잠금").pending() == 0 {
                continue;
            }
            match flush(&log, api).await {
                Ok(n) => sent += n,
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
        }
        match first_error {
            Some(e) => Err(e),
            None => Ok(sent),
        }
    }
}

/// 앱 시계의 유닉스 밀리초.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// 올리기 실패.
#[derive(Debug, thiserror::Error)]
pub enum FlushError {
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error("로컬 로그를 쓰지 못했다: {0}")]
    Disk(#[from] io::Error),
}

/// 쌓인 사건을 순서대로 올린다. 다 올렸으면 `Ok(올린 수)`. 실패하면 쌓인 것은 그대로 두니 나중에 다시 부른다.
/// 잠금은 짧게만 잡는다(올리는 동안에도 화면이 사건을 쌓을 수 있다).
pub async fn flush(log: &Mutex<ProcessLog>, api: &ApiClient) -> Result<usize, FlushError> {
    let (attempt, public_key, registered) = {
        let l = log.lock().expect("과정 로그 잠금");
        (l.saved.attempt.clone(), l.signer.public_key(), l.registered)
    };
    if !registered {
        api.register_log_key(&attempt.assignment_id, attempt.position, &public_key)
            .await?;
        log.lock().expect("과정 로그 잠금").registered = true;
    }
    let mut sent = 0;
    let mut rebased = false;
    loop {
        let batch: Vec<SignedEvent> = {
            let l = log.lock().expect("과정 로그 잠금");
            l.saved
                .pending
                .iter()
                .take(MAX_EVENTS_PER_UPLOAD)
                .cloned()
                .collect()
        };
        if batch.is_empty() {
            return Ok(sent);
        }
        match api
            .upload_log(&attempt.assignment_id, attempt.position, &batch)
            .await
        {
            Ok(head) => {
                log.lock().expect("과정 로그 잠금").acknowledge(&head)?;
                sent += batch.len();
            }
            Err(ClientError::Rejected { status: 409, .. }) if !rebased => {
                // 서버 체인과 어긋났다: 서버 머리를 받아 그 위로 잇는다(한 번만).
                let head = api
                    .register_log_key(&attempt.assignment_id, attempt.position, &public_key)
                    .await?;
                log.lock().expect("과정 로그 잠금").rebase(&head)?;
                rebased = true;
            }
            Err(e) => return Err(e.into()),
        }
    }
}
