//! 실제 모델 시험(#98, #131 후보 모델마다 — Qwen2.5-Coder 7B·1.5B): 같은 입력 → 같은 출력, 접두 KV 캐시를 써도 같다,
//! 도구 호출 템플릿이 먹힌다. 후보 표([`MODELS`])의 모델을 모두 돌린다(`PROMPTLAB_TEST_MODEL=1.5b`면 그것만).
//!
//! 모델 파일(약 4.7GB)이 있어야 하므로 `#[ignore]`로 묶어 두고 명시적으로 켠다(PR 검사는 돌리지 않는다 — PR #98 본문의
//! 「CI 판단」: 받기 4.7GB·RSS 약 6GB·CPU 시간이 빌드 노드에 무겁다):
//!
//! ```sh
//! bash scripts/fetch-model.sh --all
//! cargo test --release -p promptlab-inference -- --ignored --test-threads=1
//! ```
//!
//! 생성 토큰을 짧게(최대 16·48) 잡아 CPU에서도 1분 안팎이다. 기대 출력은 CPU(기능 `vulkan` 없음)에서 나온 값이다.

use std::time::{Duration, Instant};

use std::path::PathBuf;

use promptlab_inference::{LlamaInference, compute_info, model_path, verify_model_file};
use promptlab_shared::ai::{Message, Tool, enabled_tools, parse_reply, render};
use promptlab_shared::inference::{Inference, MODELS, ModelPin, SamplingParams};

/// 시험 하나의 시간 한도(release, 모델 캐시 뒤). 7B를 CPU 8스레드로 — 로드·짧은 생성.
const LIMIT: Duration = Duration::from_secs(180);

/// 모델마다 고정 system·고정 템플릿에서 나온 답 — 모델·llama.cpp 판·`ai::SYSTEM_PROMPT`·템플릿을 바꾸면 함께 바꾼다.
/// 후보를 더하면 여기에도 한 줄 더한다(빠지면 시험이 알린다).
/// 2026-11-09 CPU(#131): 두 모델 모두 「ready」. 도구 호출 시험은 7B 통과, **1.5B 실패** — ```json 울타리에 인자를 지어내
/// 48토큰 안에 JSON이 닫히지 않는다(기대는 낮추지 않는다).
const EXPECTED: &[(&str, &str)] = &[("7b", "ready"), ("1.5b", "ready")];

/// 돌릴 후보 모델(`PROMPTLAB_TEST_MODEL`이 있으면 그 id 하나)과 경로.
fn models() -> Vec<(&'static ModelPin, PathBuf)> {
    let only = std::env::var("PROMPTLAB_TEST_MODEL").ok();
    let picked: Vec<_> = MODELS
        .iter()
        .filter(|m| only.as_deref().is_none_or(|id| id == m.id))
        .map(|m| (m, model_path(m)))
        .collect();
    assert!(
        !picked.is_empty(),
        "PROMPTLAB_TEST_MODEL={only:?}은 후보가 아니다"
    );
    picked
}

fn short(max_tokens: u32) -> SamplingParams {
    SamplingParams {
        max_tokens,
        ..SamplingParams::default()
    }
}

fn user(text: &str) -> Message {
    Message::User { text: text.into() }
}

#[test]
#[ignore = "모델 파일이 필요하다 — scripts/fetch-model.sh 뒤 --ignored 로 켠다"]
fn 같은_입력이면_같은_출력이고_접두_캐시를_써도_같다() {
    for (m, path) in models() {
        같은_출력(m, &path);
    }
}

fn 같은_출력(m: &ModelPin, path: &std::path::Path) {
    let started = Instant::now();
    verify_model_file(path).unwrap();
    let expected = EXPECTED
        .iter()
        .find(|(id, _)| *id == m.id)
        .unwrap_or_else(|| panic!("{}의 EXPECTED가 없다", m.id))
        .1;
    let info = compute_info().unwrap();
    let plain = LlamaInference::load(path).unwrap();
    let cached = LlamaInference::load_with_prefix_cache(path).unwrap();
    let loaded = started.elapsed();
    let messages = [user("Reply with exactly one lowercase word: ready")];
    let p = render(None, &messages, &[]);
    let params = short(16);

    let a = plain.complete(&p.prefix, &[&p.suffix], &params).unwrap();
    let t0 = Instant::now();
    let b = cached.complete(&p.prefix, &[&p.suffix], &params).unwrap();
    let first = t0.elapsed();
    // 같은 접두 다시 — 캐시에서 접두를 하나도 다시 처리하지 않는다.
    let (in_before, _) = cached.token_totals().unwrap();
    let c = cached.complete(&p.prefix, &[&p.suffix], &params).unwrap();
    let (in_after, out_after) = cached.token_totals().unwrap();
    let suffix_tokens = cached.count_tokens(&p.suffix).unwrap() as u64;
    eprintln!(
        "[{}] 장치 {info:?}, 로드 {loaded:?}, 첫 호출 {first:?}, 접두 {}토큰, 답 {a:?}, 출력 누계 {out_after}, 전체 {:?}",
        m.id,
        cached.count_tokens(&p.prefix).unwrap(),
        started.elapsed()
    );
    assert_eq!(in_after - in_before, suffix_tokens, "접두를 다시 처리했다");
    assert_eq!(a, b, "캐시를 쓴 답이 쓰지 않은 답과 다르다");
    assert_eq!(b, c);
    assert_eq!(
        a[0].trim(),
        expected,
        "{}: 고정 모델의 기대 출력과 다르다",
        m.id
    );
    assert!(
        started.elapsed() < LIMIT,
        "시험 시간 한도 {LIMIT:?}를 넘었다: {:?}",
        started.elapsed()
    );
}

#[test]
#[ignore = "모델 파일이 필요하다 — scripts/fetch-model.sh 뒤 --ignored 로 켠다"]
fn 도구를_주면_qwen_꼴로_도구를_부른다() {
    for (m, path) in models() {
        도구_호출(m, &path);
    }
}

fn 도구_호출(m: &ModelPin, path: &std::path::Path) {
    let started = Instant::now();
    let model = LlamaInference::load(path).unwrap();
    let tools = enabled_tools();
    let p = render(
        None,
        &[user(
            "Call the read_problem tool now. Do not write anything else.",
        )],
        &tools,
    );
    let raw = model.complete(&p.prefix, &[&p.suffix], &short(48)).unwrap();
    let (text, calls) = parse_reply(&raw[0]);
    eprintln!(
        "[{}] 답 {:?} → 글 {text:?}, 호출 {calls:?}, {:?}",
        m.id,
        raw[0],
        started.elapsed()
    );
    assert_eq!(calls.len(), 1, "{}: {raw:?}", m.id);
    assert_eq!(calls[0].name, Tool::ReadProblem.name());
    assert!(started.elapsed() < LIMIT, "{:?}", started.elapsed());
}

#[test]
#[ignore = "모델 파일이 필요하다 — scripts/fetch-model.sh 뒤 --ignored 로 켠다"]
fn 템플릿_특수_토큰은_하나로_센다() {
    for (m, path) in models() {
        let model = LlamaInference::load(&path).unwrap();
        for t in ["<|im_start|>", "<|im_end|>", "<tool_call>", "</tool_call>"] {
            assert_eq!(model.count_tokens(t).unwrap(), 1, "{}: {t}", m.id);
        }
        // 무력화한 표기는 특수 토큰이 아니다.
        assert!(model.count_tokens("< |im_start|>").unwrap() > 1);
    }
}

#[test]
fn 후보마다_기대_출력이_있다() {
    for m in &MODELS {
        assert!(EXPECTED.iter().any(|(id, _)| *id == m.id), "{}", m.id);
    }
}

#[test]
fn 모델_파일이_없으면_받는_법을_알려_준다() {
    let missing = std::path::Path::new("/없는/모델.gguf");
    let err = LlamaInference::load(missing).err().unwrap();
    assert!(err.to_string().contains("fetch-model.sh"), "{err}");
    assert!(verify_model_file(missing).is_err());
}

#[test]
fn 장치_정보는_모델_없이_본다() {
    let info = compute_info().unwrap();
    // CPU 빌드(기능 vulkan 없음)는 GPU가 없다. 메모리는 CPU 장치가 알려 준다.
    if !cfg!(feature = "vulkan") {
        assert_eq!(info.gpu, None);
    }
    assert!(info.ram_bytes.unwrap_or(1) > 0);
}
