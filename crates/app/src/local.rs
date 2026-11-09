//! 로컬 모델(AI 도우미, #98). 추론은 CPU·GPU를 오래 쓰는 동기 호출이라 따로 스레드에서 돌리고 결과를 기다린다
//! (화면이 멈추지 않게). 대화·도구 루프는 [`crate::assistant`].
//!
//! - 스트리밍(#116): [`LocalModel::stream`]이 토큰마다 지금까지의 글을 알리고(밀린 조각은 마지막 것만) [`Cancel`]로 멈춘다.
//! - 속도 진단(#116): 실제 모델의 생성마다 [`GenStats`] 한 줄을 `<앱 데이터>/ai-perf.log`([`PERF_LOG`])에 덧붙인다
//!   (프롬프트·재사용 토큰, 처리·생성 tok/s, n_ctx, GPU 레이어, 백엔드). 1MB를 넘으면 `.old`로 돌린다.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use promptlab_inference::{LlamaInference, verify_model_file};
use promptlab_shared::ai::Prompt;
use promptlab_shared::inference::{
    GenStats, Generation, Inference, InferenceError, SamplingParams,
};

/// 속도 진단 로그 파일 이름(앱 데이터 폴더 안).
pub const PERF_LOG: &str = "ai-perf.log";

/// 이 크기를 넘으면 진단 로그를 `.old`로 돌린다.
const PERF_LOG_MAX_BYTES: u64 = 1 << 20;

/// 생성 취소 깃발(차례마다 새로 만든다). 화면의 「멈추기」가 켜고 추론 스레드가 토큰마다 본다.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// 진단 한 줄을 덧붙인다(실패해도 응시를 막지 않는다).
pub fn append_perf_log(path: &Path, stats: &GenStats, stopped: bool) {
    append_line(path, &format!("{} stopped={stopped}", stats.line()));
}

/// 진단 로그에 `at_ms=… <글>` 한 줄(크기를 넘으면 `.log.old`로 돌린다).
fn append_line(path: &Path, line: &str) {
    if std::fs::metadata(path).is_ok_and(|m| m.len() > PERF_LOG_MAX_BYTES) {
        let _ = std::fs::rename(path, path.with_extension("log.old"));
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "at_ms={at} {line}");
    }
}

/// 앱이 쥐는 로컬 모델.
pub struct LocalModel<M> {
    model: Arc<M>,
    /// 올릴 때 잰 모델 파일 sha256(문항 시작 때 서버 허용 목록과 대조한다, #12).
    sha256: String,
    /// 속도 진단 로그(실제 모델만 — [`PERF_LOG`]).
    perf_log: Option<PathBuf>,
}

impl<M> Clone for LocalModel<M> {
    fn clone(&self) -> Self {
        Self {
            model: Arc::clone(&self.model),
            sha256: self.sha256.clone(),
            perf_log: self.perf_log.clone(),
        }
    }
}

impl<M> LocalModel<M> {
    /// 올릴 때 잰 모델 파일 sha256.
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// 안의 추론기(시험이 가짜 모델을 들여다본다).
    pub fn inner(&self) -> &M {
        &self.model
    }
}

impl LocalModel<LlamaInference> {
    /// 고정 모델 파일을 확인(sha256)하고 올린다. 실행 설정은 GPU 여유 메모리로 고른다.
    pub async fn load(path: PathBuf) -> Result<Self, InferenceError> {
        Self::load_with(path, None).await
    }

    /// [`load`](Self::load) + 실행 설정 지정(`None` = 자동, `promptlab-app-cli --bench-ai`가 비교에 쓴다).
    /// 올린 뒤 GPU 메모리·고른 설정을 앱 로그(`ai-perf.log`)에 한 줄 남긴다.
    pub async fn load_with(
        path: PathBuf,
        runtime: Option<promptlab_shared::inference::RuntimeConfig>,
    ) -> Result<Self, InferenceError> {
        let (model, sha256) = off_thread(move || {
            let sha256 = verify_model_file(&path)?;
            // 응시 앱만 호출 사이 접두 KV를 남긴다(턴마다 같은 대화 접두를 다시 처리하지 않는다 — inference 크레이트 문서).
            Ok((LlamaInference::load_with(&path, true, runtime)?, sha256))
        })
        .await?;
        let mut model = Self::with_sha256(model, sha256);
        let log = crate::proclog::data_dir().join(PERF_LOG);
        append_line(&log, &format!("load {}", model.inner().load_note()));
        model.perf_log = Some(log);
        Ok(model)
    }
}

impl<M: Inference + Send + Sync + 'static> LocalModel<M> {
    /// 파일 없이 만든 모델(시험의 가짜 LLM). sha256은 비어 있다.
    pub fn new(model: M) -> Self {
        Self::with_sha256(model, String::new())
    }

    pub fn with_sha256(model: M, sha256: String) -> Self {
        Self {
            model: Arc::new(model),
            sha256,
            perf_log: None,
        }
    }

    /// 속도 진단 로그를 이 파일에 남긴다(`None`이면 끈다).
    pub fn set_perf_log(&mut self, path: Option<PathBuf>) {
        self.perf_log = path;
    }

    /// 프롬프트 하나로 답을 만들며 토큰마다 `on_text(지금까지의 글, 생성 토큰 수)`를 부른다(화면이 밀리면 마지막 조각만).
    /// `cancel`이 켜지면 다음 토큰에서 멈추고 그때까지의 글을 `stopped`로 돌려준다. 진단 줄을 [`PERF_LOG`]에 남긴다.
    pub async fn stream(
        &self,
        prompt: Prompt,
        params: SamplingParams,
        cancel: &Cancel,
        mut on_text: impl FnMut(&str, usize),
    ) -> Result<Generation, InferenceError> {
        let model = Arc::clone(&self.model);
        let cancel = cancel.clone();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(String, usize)>();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            // 깃발은 조각을 보내기 **전에** 읽는다 — 보낸 뒤에 읽으면 받는 쪽이 이 조각을 보고 누른 멈추기가 같은 토큰에서
            // 걸리기도 하고 안 걸리기도 해(스레드 순서), 멈춘 자리가 한 토큰씩 흔들린다(#116 CI). 이제는 늘 「누른 뒤 다음 토큰」.
            let mut on = move |text: &str, n: usize| {
                let go = !cancel.is_cancelled();
                let _ = tx.send((text.to_owned(), n));
                go
            };
            let result = if on("", 0) {
                model.stream(&prompt.prefix, &prompt.suffix, &params, &mut on)
            } else {
                // 시작 전에 이미 멈췄다.
                Ok(Generation {
                    text: String::new(),
                    stopped: true,
                    stats: None,
                })
            };
            drop(on);
            // 모델을 놓은 **뒤에** 알린다 — 받는 쪽이 마지막 참조를 버리면 그 자리에서 모델·KV가 풀린다(#116: 반대 순서면
            // `--bench-ai --matrix`가 다음 조합을 올릴 때 앞 모델이 아직 VRAM에 있어 문맥이 안 열렸다).
            drop(model);
            let _ = done_tx.send(result);
        });
        while let Some(mut latest) = rx.recv().await {
            while let Ok(next) = rx.try_recv() {
                latest = next;
            }
            if latest.1 > 0 || !latest.0.is_empty() {
                on_text(&latest.0, latest.1);
            }
        }
        let result = done_rx
            .await
            .map_err(|_| InferenceError::Failed("추론 스레드가 멈췄다".into()))?;
        if let (Ok(g), Some(path)) = (&result, &self.perf_log)
            && let Some(stats) = &g.stats
        {
            append_perf_log(path, stats, g.stopped);
        }
        result
    }

    /// 프롬프트 하나로 답을 만든다(접두 KV는 구현이 남겨 두면 다시 쓴다).
    pub async fn generate(
        &self,
        prompt: Prompt,
        params: SamplingParams,
    ) -> Result<String, InferenceError> {
        let model = Arc::clone(&self.model);
        off_thread(move || {
            Ok(model
                .complete(&prompt.prefix, &[&prompt.suffix], &params)?
                .pop()
                .unwrap_or_default())
        })
        .await
    }

    /// 글마다 토큰 수(문맥 예산 계산).
    pub async fn count(&self, texts: Vec<String>) -> Result<Vec<usize>, InferenceError> {
        let model = Arc::clone(&self.model);
        off_thread(move || texts.iter().map(|t| model.count_tokens(t)).collect()).await
    }

    /// 추론기가 연 문맥 창(토큰) — 요약 예산이 따른다(#116, GPU 메모리에 맞춰 32k보다 작을 수 있다).
    pub fn context_tokens(&self) -> u32 {
        self.model.context_tokens()
    }

    /// 처리한 (입력, 출력) 토큰 누계(모르면 `None`).
    pub fn token_totals(&self) -> Option<(u64, u64)> {
        self.model.token_totals()
    }
}

/// 동기 작업을 새 스레드에서 돌리고 끝나기를 기다린다(어느 async 런타임에서도 쓴다).
async fn off_thread<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, InferenceError> + Send + 'static,
) -> Result<T, InferenceError> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    rx.await
        .map_err(|_| InferenceError::Failed("추론 스레드가 멈췄다".into()))?
}
