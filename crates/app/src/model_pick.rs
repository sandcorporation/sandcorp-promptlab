//! AI 도우미 모델 고르기(#131): 추천([`promptlab_shared::model_choice`])과 응시자가 고른 크기(설정 파일
//! [`MODEL_PREFS_FILE`] — 과정 로그가 아니다), 고른 모델 받기(진행·재시도·이어 받기·sha256 검증).
//!
//! - 고른 크기가 없으면(「자동」) 추천을 쓴다. 고르면 기억하고 다음 실행에도 그것을 쓴다(사양이 바뀌어도).
//! - 다른 크기로 바꾸면 화면이 올린 모델을 내리고 새 모델을 올린다. 옛 파일은 지우지 않는다(다시 바꾸면 바로 쓴다).
//! - 바꾸는 곳은 응시 전 점검 페이지·설정뿐이다 — 문항을 푸는 동안(응시 화면)에는 바꿀 수 없다.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use promptlab_shared::inference::{MODELS, ModelPin, model_by_id};
use promptlab_shared::model_choice::{Machine, Plan, gb, plan, recommend};
use sha2::{Digest, Sha256};

/// 설정 파일 이름(앱 데이터 폴더 안).
pub const MODEL_PREFS_FILE: &str = "ai-model.json";
/// 설정 파일 형식 판. 모르는 판·깨진 파일은 「자동」으로 시작한다(옛 판 호환은 두지 않는다).
pub const MODEL_PREFS_FORMAT: u32 = 1;

/// 응시자가 고른 모델 크기.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ModelPrefs {
    pub version: u32,
    /// 고른 모델의 [`ModelPin::id`]. `None` = 자동(추천을 따른다).
    pub model: Option<String>,
}

impl Default for ModelPrefs {
    fn default() -> Self {
        Self {
            version: MODEL_PREFS_FORMAT,
            model: None,
        }
    }
}

impl ModelPrefs {
    /// 글에서 읽는다. 깨졌거나 모르는 판·후보에 없는 모델이면 자동.
    pub fn parse(text: &str) -> Self {
        match serde_json::from_str::<Self>(text) {
            Ok(p)
                if p.version == MODEL_PREFS_FORMAT
                    && p.model
                        .as_deref()
                        .is_none_or(|id| model_by_id(id).is_some()) =>
            {
                p
            }
            _ => Self::default(),
        }
    }

    /// 파일에서 읽는다. 없거나 못 읽으면 자동.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .map(|t| Self::parse(&t))
            .unwrap_or_default()
    }

    /// 파일에 쓴다.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string(self).map_err(std::io::Error::other)?;
        std::fs::write(path, text)
    }

    /// 고른 모델(자동이면 `None`).
    pub fn chosen(&self) -> Option<&'static ModelPin> {
        self.model.as_deref().and_then(model_by_id)
    }
}

/// 이 PC에서 모델 고르기의 상태: 사양·받아 둔 모델·설정.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelPick {
    pub machine: Machine,
    /// 캐시에 받아 둔 후보 id.
    pub downloaded: Vec<&'static str>,
    pub prefs: ModelPrefs,
}

impl ModelPick {
    /// 추천 계획.
    pub fn recommended(&self) -> Plan {
        recommend(&self.machine, &self.downloaded)
    }

    /// 쓸 모델의 계획 — 고른 것이 있으면 그것, 없으면 추천.
    pub fn chosen(&self) -> Plan {
        match self.prefs.chosen() {
            Some(m) => plan(m, &self.machine, &self.downloaded, false),
            None => self.recommended(),
        }
    }

    /// 응시자가 직접 골랐나.
    pub fn by_user(&self) -> bool {
        self.prefs.chosen().is_some()
    }

    /// 받아 뒀나.
    pub fn is_downloaded(&self, model: &ModelPin) -> bool {
        self.downloaded.contains(&model.id)
    }

    /// 「추천 모델: Qwen2.5-Coder 7B (이유: …)」 — 제한적인 모델이면 이름에 표시가 붙는다.
    pub fn headline(&self) -> String {
        let r = self.recommended();
        format!("추천 모델: {} (이유: {})", r.model.display_name(), r.reason)
    }

    /// 직접 고른 모델 줄 「고른 모델: … — 이유」(자동이면 `None`).
    pub fn chosen_line(&self) -> Option<String> {
        self.by_user().then(|| {
            let c = self.chosen();
            format!("고른 모델: {} — {}", c.model.display_name(), c.reason)
        })
    }

    /// 쓸 모델이 제한적이면 보일 안내(#131 결정 B — 고를 때·자동으로 골랐을 때 모두).
    pub fn limited_notice(&self) -> Option<String> {
        let c = self.chosen();
        c.model.limited.then(|| {
            let why = if self.by_user() {
                "직접 고른 모델입니다"
            } else {
                "이 PC에서는 더 큰 모델을 올릴 수 없어 골랐습니다"
            };
            format!(
                "{}는 제한적입니다 — 작은 모델이라 질문에 답을 못 하거나 편집 제안이 깨질 수 있습니다({why}).",
                c.model.label
            )
        })
    }

    /// 고르기 목록의 한 줄 이름(「Qwen2.5-Coder 7B · 4.4GB · 받아 둠」).
    pub fn option_label(&self, model: &ModelPin) -> String {
        let state = if self.is_downloaded(model) {
            "받아 둠"
        } else {
            "받아야 함"
        };
        let rec = if self.recommended().model.id == model.id {
            " · 추천"
        } else {
            ""
        };
        format!(
            "{} · {} · {state}{rec}",
            model.display_name(),
            gb(model.size)
        )
    }
}

/// 다른 앱이 그래픽 메모리를 써 넘쳤을 때(#116 `gpu_short`) 더 작은 후보가 있으면 덧붙이는 안내.
pub fn smaller_model_hint(current: &ModelPin) -> Option<String> {
    MODELS.iter().find(|m| m.size < current.size).map(|m| {
        let limited = if m.limited {
            " 다만 작은 모델은 제한적이라 답을 못 할 수 있습니다."
        } else {
            ""
        };
        format!(
            "응시 전 점검에서 더 작은 모델({})로 바꾸면 빨라질 수 있습니다.{limited}",
            m.label
        )
    })
}

/// 받기 시도 횟수.
pub const FETCH_ATTEMPTS: u32 = 3;

/// 받기 진행.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FetchProgress {
    pub done: u64,
    pub total: u64,
    /// 몇 번째 시도(1부터).
    pub attempt: u32,
}

impl FetchProgress {
    /// 「1.2GB / 4.4GB (27%)」(두 번째 시도부터 「· 다시 시도 2/3」).
    pub fn text(&self) -> String {
        let pct = (self.done * 100).checked_div(self.total).unwrap_or(0);
        let retry = if self.attempt > 1 {
            format!(" · 다시 시도 {}/{FETCH_ATTEMPTS}", self.attempt)
        } else {
            String::new()
        };
        format!("{} / {} ({pct}%){retry}", gb(self.done), gb(self.total))
    }
}

/// 받기 실패.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    #[error("모델을 받지 못했습니다(인터넷 연결을 확인하세요): {0}")]
    Network(String),
    #[error("받은 모델 파일이 손상됐습니다(sha256 불일치) — 다시 받아 주세요")]
    Mismatch,
    #[error("모델 파일을 쓰지 못했습니다: {0}")]
    Disk(String),
}

/// `model`을 `url`에서 `dir`로 받는다(`dir/<file>.part`에 이어 받고, 끝나면 sha256을 재어 맞으면 `<file>`로 옮긴다).
/// 이미 맞는 파일이 있으면 받지 않는다. 끊기면 [`FETCH_ATTEMPTS`]번까지 이어서 다시 받고, 해시가 틀리면 버리고 처음부터.
pub async fn fetch(
    http: &reqwest::Client,
    model: &ModelPin,
    url: &str,
    dir: &Path,
    mut on_progress: impl FnMut(FetchProgress),
) -> Result<PathBuf, FetchError> {
    let dest = dir.join(model.file);
    if std::fs::metadata(&dest).is_ok_and(|m| m.len() == model.size) {
        return Ok(dest);
    }
    std::fs::create_dir_all(dir).map_err(|e| FetchError::Disk(e.to_string()))?;
    let part = dir.join(format!("{}.part", model.file));
    let mut last = FetchError::Network("시도하지 않음".into());
    for attempt in 1..=FETCH_ATTEMPTS {
        if attempt > 1 {
            tokio::time::sleep(Duration::from_millis(500 * u64::from(attempt - 1))).await;
        }
        match fetch_once(http, model, url, &part, attempt, &mut on_progress).await {
            Ok(()) => {}
            Err(e @ FetchError::Disk(_)) => return Err(e),
            Err(e) => {
                last = e;
                continue;
            }
        }
        let path = part.clone();
        let sha = tokio::task::spawn_blocking(move || file_sha256(&path))
            .await
            .map_err(|e| FetchError::Disk(e.to_string()))?
            .map_err(|e| FetchError::Disk(e.to_string()))?;
        if sha != model.sha256 {
            let _ = std::fs::remove_file(&part);
            last = FetchError::Mismatch;
            continue;
        }
        std::fs::rename(&part, &dest).map_err(|e| FetchError::Disk(e.to_string()))?;
        return Ok(dest);
    }
    Err(last)
}

async fn fetch_once(
    http: &reqwest::Client,
    model: &ModelPin,
    url: &str,
    part: &Path,
    attempt: u32,
    on_progress: &mut impl FnMut(FetchProgress),
) -> Result<(), FetchError> {
    let mut have = std::fs::metadata(part).map_or(0, |m| m.len());
    if have > model.size {
        let _ = std::fs::remove_file(part);
        have = 0;
    }
    if have == model.size {
        return Ok(());
    }
    let net = |e: reqwest::Error| FetchError::Network(e.to_string());
    let mut req = http.get(url);
    if have > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let mut resp = req.send().await.map_err(net)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(FetchError::Network(format!("HTTP {}", status.as_u16())));
    }
    // 이어 받기를 서버가 안 받으면(200) 처음부터.
    let append = have > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT;
    if !append {
        have = 0;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(part)
        .map_err(|e| FetchError::Disk(e.to_string()))?;
    let mut done = have;
    on_progress(FetchProgress {
        done,
        total: model.size,
        attempt,
    });
    let mut reported = done;
    while let Some(chunk) = resp.chunk().await.map_err(net)? {
        file.write_all(&chunk)
            .map_err(|e| FetchError::Disk(e.to_string()))?;
        done += chunk.len() as u64;
        // 진행은 4MB마다(화면을 너무 자주 다시 그리지 않게).
        if done - reported >= 4 << 20 || done >= model.size {
            reported = done;
            on_progress(FetchProgress {
                done,
                total: model.size,
                attempt,
            });
        }
    }
    file.flush().map_err(|e| FetchError::Disk(e.to_string()))?;
    if done != model.size {
        return Err(FetchError::Network(format!(
            "받다가 끊겼습니다({} / {})",
            gb(done),
            gb(model.size)
        )));
    }
    Ok(())
}

/// 파일 sha256(소문자 16진).
pub fn file_sha256(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
