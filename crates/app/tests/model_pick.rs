//! 모델 고르기(#131): 설정 파일(형식 판·깨진 파일·모르는 모델), 고른 것과 추천, 넘침 안내, 받기(가짜 서버 —
//! 이어 받기·sha256 불일치·이미 있음).

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use promptlab_app::model_pick::{
    FETCH_ATTEMPTS, FetchError, FetchProgress, ModelPick, ModelPrefs, fetch, file_sha256,
    smaller_model_hint,
};
use promptlab_shared::inference::{MODELS, ModelPin, model_by_id};
use promptlab_shared::model_choice::{GpuMemory, Machine};

const GIB: u64 = 1024 * 1024 * 1024;

#[test]
fn 설정은_형식_판이_맞고_후보에_있는_모델만_받는다() {
    let p = ModelPrefs {
        model: Some("1.5b".into()),
        ..ModelPrefs::default()
    };
    let text = serde_json::to_string(&p).unwrap();
    assert_eq!(ModelPrefs::parse(&text), p);
    assert_eq!(p.chosen().unwrap().id, "1.5b");
    for bad in [
        "깨짐",
        r#"{"version":99,"model":"1.5b"}"#,
        r#"{"version":1,"model":"3b"}"#,
    ] {
        assert_eq!(ModelPrefs::parse(bad), ModelPrefs::default(), "{bad}");
    }
    assert_eq!(ModelPrefs::default().chosen(), None);
}

#[test]
fn 설정_파일을_쓰고_다시_읽는다() {
    let dir = std::env::temp_dir().join(format!("pl-model-prefs-{}", std::process::id()));
    let path = dir.join("ai-model.json");
    assert_eq!(ModelPrefs::load(&path), ModelPrefs::default());
    let p = ModelPrefs {
        model: Some("7b".into()),
        ..ModelPrefs::default()
    };
    p.save(&path).unwrap();
    assert_eq!(ModelPrefs::load(&path), p);
    let _ = std::fs::remove_dir_all(&dir);
}

fn pick(usable: u64, downloaded: Vec<&'static str>, model: Option<&str>) -> ModelPick {
    ModelPick {
        machine: Machine {
            gpu: Some(GpuMemory {
                total: 8 * GIB,
                usable,
            }),
            free_ram: 12 * GIB,
        },
        downloaded,
        prefs: ModelPrefs {
            model: model.map(Into::into),
            ..ModelPrefs::default()
        },
    }
}

#[test]
fn 고른_것이_없으면_추천을_쓰고_고르면_그것을_쓴다() {
    // 8GB 중 여유 5GB: 7B 일부 층(결정 B).
    let auto = pick(5 * GIB, vec![], None);
    assert!(!auto.by_user());
    assert_eq!(auto.chosen().model.id, "7b");
    assert_eq!(auto.chosen_line(), None);
    assert_eq!(auto.limited_notice(), None);
    assert!(
        auto.headline()
            .starts_with("추천 모델: Qwen2.5-Coder 7B (이유: 그래픽 메모리 8.0GB 중 여유 5.0GB · Qwen2.5-Coder 7B의 "),
        "{}",
        auto.headline()
    );
    let mine = pick(5 * GIB, vec!["1.5b"], Some("1.5b"));
    assert!(mine.by_user());
    assert_eq!(mine.chosen().model.id, "1.5b");
    assert_eq!(mine.recommended().model.id, "7b");
    assert_eq!(mine.chosen().download_bytes, 0);
    let line = mine.chosen_line().unwrap();
    assert!(
        line.starts_with("고른 모델: Qwen2.5-Coder 1.5B(제한적 — 답을 못 할 수 있음) — "),
        "{line}"
    );
    let notice = mine.limited_notice().unwrap();
    assert!(notice.contains("제한적입니다"), "{notice}");
    assert!(notice.contains("직접 고른 모델"), "{notice}");
    assert_eq!(
        mine.option_label(model_by_id("7b").unwrap()),
        "Qwen2.5-Coder 7B · 4.4GB · 받아야 함 · 추천"
    );
    assert_eq!(
        mine.option_label(model_by_id("1.5b").unwrap()),
        "Qwen2.5-Coder 1.5B(제한적 — 답을 못 할 수 있음) · 1.0GB · 받아 둠"
    );
}

#[test]
fn ram이_모자라_자동으로_15b가_되면_추천_줄과_안내에_제한적이_붙는다() {
    let mut auto = pick(5 * GIB, vec![], None);
    auto.machine.free_ram = GIB;
    assert_eq!(auto.chosen().model.id, "1.5b");
    assert!(
        auto.headline()
            .starts_with("추천 모델: Qwen2.5-Coder 1.5B(제한적 — 답을 못 할 수 있음) (이유: "),
        "{}",
        auto.headline()
    );
    let notice = auto.limited_notice().unwrap();
    assert!(notice.contains("더 큰 모델을 올릴 수 없어"), "{notice}");
}

#[test]
fn 넘치면_더_작은_모델이_있을_때만_권하고_제한적임을_알린다() {
    let hint = smaller_model_hint(&MODELS[0]).unwrap();
    assert!(hint.contains("Qwen2.5-Coder 1.5B"), "{hint}");
    assert!(hint.contains("제한적"), "{hint}");
    assert_eq!(smaller_model_hint(&MODELS[MODELS.len() - 1]), None);
}

#[test]
fn 진행_문구() {
    let p = FetchProgress {
        done: GIB,
        total: 4 * GIB,
        attempt: 2,
    };
    assert_eq!(
        p.text(),
        format!("1.0GB / 4.0GB (25%) · 다시 시도 2/{FETCH_ATTEMPTS}")
    );
}

// ── 받기(가짜 서버) ──

#[derive(Default)]
struct Fake {
    body: Vec<u8>,
    /// 첫 응답은 앞 절반만 보내고 끊는다.
    cut_first: bool,
    /// 받은 Range 머리(없으면 "").
    ranges: Vec<String>,
}

type Shared = Arc<Mutex<Fake>>;

async fn file(State(s): State<Shared>, headers: HeaderMap) -> impl IntoResponse {
    let mut f = s.lock().unwrap();
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let first = f.ranges.is_empty();
    f.ranges.push(range.clone());
    if let Some(from) = range
        .strip_prefix("bytes=")
        .and_then(|r| r.strip_suffix('-'))
        .and_then(|n| n.parse::<usize>().ok())
    {
        return (StatusCode::PARTIAL_CONTENT, f.body[from..].to_vec()).into_response();
    }
    if first && f.cut_first {
        return (StatusCode::OK, f.body[..f.body.len() / 2].to_vec()).into_response();
    }
    (StatusCode::OK, f.body.clone()).into_response()
}

async fn serve(fake: Shared) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = Router::new().route("/m.gguf", get(file)).with_state(fake);
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{addr}/m.gguf")
}

fn fake_model(body: &[u8], sha256_of: &[u8]) -> ModelPin {
    let dir = std::env::temp_dir().join(format!("pl-sha-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("x");
    std::fs::write(&p, sha256_of).unwrap();
    let sha: &'static str = Box::leak(file_sha256(&p).unwrap().into_boxed_str());
    ModelPin {
        file: "fake.gguf",
        sha256: sha,
        size: body.len() as u64,
        ..MODELS[MODELS.len() - 1]
    }
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("pl-fetch-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[tokio::test]
async fn 끊기면_이어_받고_해시가_맞으면_옮긴다() {
    let body: Vec<u8> = (0..(9 << 20)).map(|i| (i % 251) as u8).collect();
    let model = fake_model(&body, &body);
    let fake = Arc::new(Mutex::new(Fake {
        body: body.clone(),
        cut_first: true,
        ..Fake::default()
    }));
    let url = serve(fake.clone()).await;
    let dir = temp_dir("resume");
    let mut seen = Vec::new();
    let path = fetch(&reqwest::Client::new(), &model, &url, &dir, |p| {
        seen.push(p)
    })
    .await
    .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), body);
    assert!(!dir.join("fake.gguf.part").exists());
    let ranges = fake.lock().unwrap().ranges.clone();
    assert_eq!(
        ranges,
        ["".to_owned(), format!("bytes={}-", body.len() / 2)]
    );
    assert_eq!(seen.last().unwrap().done, body.len() as u64);
    assert_eq!(seen.last().unwrap().attempt, 2);
    // 이미 있으면 받지 않는다.
    fetch(&reqwest::Client::new(), &model, &url, &dir, |_| {})
        .await
        .unwrap();
    assert_eq!(fake.lock().unwrap().ranges.len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn 해시가_틀리면_버리고_다시_받다가_실패한다() {
    let body = vec![7u8; 1000];
    let model = fake_model(&body, b"other");
    let fake = Arc::new(Mutex::new(Fake {
        body,
        ..Fake::default()
    }));
    let url = serve(fake.clone()).await;
    let dir = temp_dir("mismatch");
    let err = fetch(&reqwest::Client::new(), &model, &url, &dir, |_| {})
        .await
        .unwrap_err();
    assert_eq!(err, FetchError::Mismatch);
    assert!(!dir.join("fake.gguf").exists());
    assert!(!dir.join("fake.gguf.part").exists());
    // 매번 처음부터(Range 없이) 받았다.
    let ranges = fake.lock().unwrap().ranges.clone();
    assert_eq!(ranges.len(), FETCH_ATTEMPTS as usize);
    assert!(ranges.iter().all(String::is_empty), "{ranges:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
