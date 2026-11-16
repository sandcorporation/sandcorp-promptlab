//! AI 속도·GPU 신고(#5): 로컬 모델이 생성마다 토큰·시간·GPU 층을 누계하고, 복제본도 같은 누계를 본다.

use promptlab_app::local::{Cancel, LocalModel};
use promptlab_shared::ai::Prompt;
use promptlab_shared::inference::{
    GenStats, Generation, Inference, InferenceError, SamplingParams,
};
use promptlab_shared::log::AiSpeed;

/// 답마다 정해 둔 진단 값을 내는 가짜 모델(GPU 21/29층, 답 하나에 100토큰·10초).
struct Measured;

fn stats() -> GenStats {
    GenStats {
        prompt_tokens: 10,
        reused_tokens: 0,
        prompt_ms: 5,
        generated_tokens: 100,
        generate_ms: 10_000,
        n_ctx: 32_768,
        n_batch: 512,
        n_ubatch: 512,
        flash_attn: "auto",
        kv: "f16",
        decode_ms: 0.0,
        sample_ms: 0.0,
        text_ms: 0.0,
        llama_prompt_ms: 0.0,
        llama_prompt_tokens: 0,
        llama_eval_ms: 0.0,
        llama_eval_tokens: 0,
        vram_mib: None,
        gpu_layers: 21,
        layers: 29,
        backend: "가짜".into(),
        threads: 1,
    }
}

impl Inference for Measured {
    fn complete(
        &self,
        _: &str,
        _: &[&str],
        _: &SamplingParams,
    ) -> Result<Vec<String>, InferenceError> {
        Ok(vec!["답".into()])
    }

    fn count_tokens(&self, text: &str) -> Result<usize, InferenceError> {
        Ok(text.chars().count())
    }

    fn stream(
        &self,
        _: &str,
        _: &str,
        _: &SamplingParams,
        on_text: &mut dyn FnMut(&str, usize) -> bool,
    ) -> Result<Generation, InferenceError> {
        on_text("답", 1);
        Ok(Generation {
            text: "답".into(),
            stopped: false,
            stats: Some(stats()),
        })
    }
}

#[tokio::test]
async fn 생성마다_속도와_gpu_층을_누계한다() {
    let model = LocalModel::new(Measured);
    // 아직 생성이 없으면 모른다(가짜 모델은 올릴 때 GPU 층을 알리지 않는다).
    assert_eq!(model.ai_speed(), None);
    let copy = model.clone();
    for _ in 0..2 {
        let prompt = Prompt {
            prefix: "p".into(),
            suffix: "s".into(),
        };
        model
            .stream(prompt, SamplingParams::default(), &Cancel::new(), |_, _| {})
            .await
            .unwrap();
    }
    let expected = AiSpeed {
        generated_tokens: 200,
        generate_ms: 20_000,
        gpu_layers: 21,
        total_layers: 29,
    };
    assert_eq!(copy.ai_speed(), Some(expected));
    assert_eq!(expected.label(), "10.0 토큰/초 · GPU 21/29층");
}
