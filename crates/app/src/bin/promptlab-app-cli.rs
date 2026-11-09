//! 화면 없이 응시 흐름 한 번(#97): 로컬 Docker 점검(그 언어 이미지·빈 하네스) → 문제 받기(시험이면 문항 시작) →
//! 코드 파일로 예제 테스트(로컬 Docker, 판정기와 같은 상한) → 제출 → 채점 결과(통과 수, spec #93).
//! 화면과 같은 상태 기계·HTTP 클라이언트·로컬 실행(`promptlab_app::{exam, localrun, docker_check}`)을 쓴다.
//!
//! ```text
//! promptlab-app-cli --language python --code-file sol.py                          # 연습(설정의 연습 문제, PROMPTLAB_PROBLEM_ID)
//! promptlab-app-cli --problem <연습 문제 id> --language java --code-file Solution.java
//! promptlab-app-cli --list                                                        # 내게 배정된 시험 목록
//! promptlab-app-cli --assignment <배정 id> --item <문항 번호> --language cpp --code-file sol.cpp   # 배정된 시험 문항 응시
//! promptlab-app-cli … --no-docker                                                 # 로컬 Docker 점검·예제 테스트를 건너뛴다
//! promptlab-app-cli --check-docker --language python                              # Docker 점검(이미지 받기·빈 틀)만 하고 끝낸다(로그인·서버 없이, 스모크가 토큰 받기 전에 쓴다)
//! promptlab-app-cli --check-gpu                                                   # 추론 장치(빌드 백엔드·llama.cpp가 본 장치·GPU 여부)만 찍고 끝낸다(모델·서버 없이, #111)
//! promptlab-app-cli --recommend-model                                             # 이 PC의 AI 모델 크기 추천·이유·고른 크기·받아 둔 모델만 찍고 끝낸다(모델·서버 없이, #131)
//! promptlab-app-cli --bench-ai [<모델 파일 | 모델 id(7b·1.5b)>] [--ctx N] [--kv f16|q8_0] [--fa auto|on|off] [--layers N|all] [--matrix]  # AI 도우미 속도 진단(#116): 고정 프롬프트로 두 차례 생성하고 프롬프트·재사용 토큰·tok/s·n_ctx·KV·FA·GPU 레이어·VRAM 여유·백엔드를 찍는다(서버 없이). 설정을 주면 그 설정으로, --matrix면 조합마다 다시 올려 표로
//! ```
//! `--language` 기본은 python, `--code-file`이 없으면 그 언어의 빈 틀을 낸다. 시험 문항은 과정 로그에 문항 시작·언어
//! 고름·코드 스냅숏·로컬 실행·`Submitted`(언어·코드 sha256)를 남긴다(서버가 제출 본문과 맞춰 본다). 결과를 공개하지
//! 않는 시험(채용 등)은 「응시 완료」로 끝난다(성공). 예제 테스트가 틀려도 제출은 한다(서버 채점이 정본).
//! 설정: `PROMPTLAB_SERVER_URL`·`PROMPTLAB_ACCESS_TOKEN`(선택 — 없으면 브라우저로 sandcorp-auth 로그인)·`PROMPTLAB_PROBLEM_ID`,
//! 모델 파일은 `PROMPTLAB_MODEL_PATH`(없으면 화면과 같은 모델 — 설정 파일에서 고른 크기, 없으면 추천 — 의 캐시 경로.
//! 시험 문항 시작 때 그 sha256을 신고한다).
//! 채점이 끝나면 0, Docker 점검 실패(`--no-docker` 없이)·채점 실패·시간 초과면 1.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use promptlab_app::client::{ApiClient, Config};
use promptlab_app::docker_check::{self, DockerCheck};
use promptlab_app::editor::{SnapshotClock, template};
use promptlab_app::exam;
use promptlab_app::localrun::{self, LocalDocker};
use promptlab_app::proclog::{self, LogBook};
use promptlab_app::session::{ExamItemRef, Phase, Session, SessionEvent};
use promptlab_shared::api::ItemState;
use promptlab_shared::coding::Language;
use promptlab_shared::inference::SPILL_TOKENS_PER_SEC;
use promptlab_shared::log::{Attempt, LocalRunKind, LogEvent};

const POLL: Duration = Duration::from_secs(2);
const WAIT_LIMIT: Duration = Duration::from_secs(15 * 60);

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("오류: {e}");
            ExitCode::FAILURE
        }
    }
}

const USAGE: &str = "쓰임: promptlab-app-cli [--problem <연습 문제 id> | --assignment <배정 id> --item <문항 번호>] [--language <python|javascript|java|cpp>] [--code-file <경로>] [--no-docker]\n       promptlab-app-cli --check-docker [--language <언어>]\n       promptlab-app-cli --check-gpu\n       promptlab-app-cli --bench-ai [<모델 파일>] [--ctx <토큰>] [--kv f16|q8_0] [--fa auto|on|off] [--layers <수>|all] [--matrix] [--no-llama-log]\n       promptlab-app-cli --list";

/// 명령줄.
struct Args {
    list: bool,
    /// 제출 언어(기본 Python).
    language: Option<Language>,
    /// 제출 코드(없으면 그 언어의 틀).
    code: Option<String>,
    item: Option<ExamItemRef>,
    /// 연습 문제(없으면 설정의 것).
    problem: Option<String>,
    /// 로컬 Docker 점검·예제 테스트를 건너뛴다.
    no_docker: bool,
    /// Docker 점검만 하고 끝낸다(로그인·서버 없이).
    check_docker: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let (mut list, mut language, mut code, mut assignment, mut item, mut problem) =
        (false, None, None, None, None, None);
    let (mut no_docker, mut check_docker) = (false, false);
    while let Some(flag) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| format!("{flag} 뒤에 값이 없다\n{USAGE}"))
        };
        match flag.as_str() {
            "--list" => list = true,
            "--no-docker" => no_docker = true,
            "--check-docker" => check_docker = true,
            "--language" => {
                let name = value()?;
                language =
                    Some(Language::parse(&name).ok_or_else(|| format!("모르는 언어: {name}"))?);
            }
            "--code-file" => {
                let path = value()?;
                code = Some(std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?);
            }
            "--assignment" => assignment = Some(value()?),
            "--problem" => problem = Some(value()?),
            "--item" => {
                let n = value()?;
                item = Some(
                    n.parse::<u32>()
                        .map_err(|_| format!("--item 은 숫자다: {n}"))?,
                );
            }
            _ => return Err(USAGE.into()),
        }
    }
    let item = match (assignment, item) {
        (Some(assignment_id), Some(position)) => Some(ExamItemRef {
            assignment_id,
            position,
        }),
        (None, None) => None,
        _ => return Err(format!("--assignment 와 --item 은 함께 준다\n{USAGE}")),
    };
    if item.is_some() && problem.is_some() {
        return Err(format!("--problem 은 연습에만 준다\n{USAGE}"));
    }
    Ok(Args {
        list,
        language,
        code,
        item,
        problem,
        no_docker,
        check_docker,
    })
}

/// 배정된 시험 목록을 찍는다.
async fn list(api: &ApiClient) -> Result<bool, String> {
    let assignments = api.assignments().await.map_err(|e| e.to_string())?;
    if assignments.is_empty() {
        println!("배정된 시험이 없다(초대 메일의 링크로 들어와 카카오 로그인하면 배정된다).");
    }
    for a in assignments {
        println!(
            "[{}] {} — {} ({}{})",
            a.id,
            a.org_name,
            a.exam_title,
            a.purpose.label(),
            if a.scores_visible {
                ", 점수 공개"
            } else {
                ""
            }
        );
        for item in a.items {
            let state = match item.state {
                ItemState::NotStarted => "시작 전".to_owned(),
                ItemState::InProgress { deadline } => format!("응시 중(마감 유닉스 {deadline})"),
                ItemState::Submitted { .. } => "제출함".to_owned(),
            };
            println!(
                "   문항 {} · 제한 {}분 · {state}",
                item.position,
                item.limits.time_limit_secs / 60,
            );
        }
    }
    Ok(true)
}

/// 빌드 백엔드와 llama.cpp가 본 장치를 찍는다. 백엔드를 못 올리면 오류, GPU가 안 보이는 것은 오류가 아니다(CPU로 돈다).
/// 마지막 줄 `[GPU] 추론 장치: GPU <이름>` 또는 `[GPU] 추론 장치: CPU …`를 스모크가 읽는다.
fn check_gpu() -> Result<bool, String> {
    let build = if promptlab_inference::GPU_BUILD {
        "vulkan(동적 모듈)"
    } else {
        "cpu"
    };
    println!("[GPU] 빌드 백엔드: {build}");
    let devices = promptlab_inference::devices().map_err(|e| e.to_string())?;
    for d in &devices {
        println!(
            "[GPU] 장치: {} {} — {} ({}, {} MiB)",
            d.backend,
            d.name,
            d.description,
            if d.gpu { "GPU" } else { "GPU 아님" },
            d.memory_total / (1024 * 1024)
        );
    }
    let info = promptlab_inference::compute_info().map_err(|e| e.to_string())?;
    match info.gpu {
        Some(name) => println!("[GPU] 추론 장치: GPU {name}"),
        None => println!("[GPU] 추론 장치: CPU(GPU 장치가 보이지 않는다)"),
    }
    Ok(true)
}

/// `--bench-ai`의 고정 문제(공개 문제 꼴 글).
const BENCH_PROBLEM: &str = "Title: 두 수의 합\nFunction: two_sum(nums: int[], target: int) -> int[]\nTime limit per case: 2000 ms\n\n\
정수 배열 nums와 정수 target이 주어진다. 더해서 target이 되는 두 원소의 인덱스를 작은 것부터 돌려준다. 답은 정확히 하나다.\n\n\
Example 1: arguments [[2,7,11,15],9] -> [0,1]\nExample 2: arguments [[3,2,4],6] -> [1,2]";

/// `--bench-ai`의 고정 코드(버그 있음 — 같은 원소를 두 번 쓴다).
const BENCH_CODE: &str = "def two_sum(nums, target):\n    seen = {}\n    for i, n in enumerate(nums):\n        seen[n] = i\n        if target - n in seen:\n            return [seen[target - n], i]\n    return []\n";

/// `--bench-ai` 인자: 모델 경로·실행 설정 지정·비교표.
#[derive(Debug, Default)]
struct BenchArgs {
    model: Option<String>,
    ctx: Option<u32>,
    kv: Option<promptlab_shared::inference::KvType>,
    fa: Option<promptlab_shared::inference::FlashAttn>,
    /// `Some(None)` = 전부.
    layers: Option<Option<u32>>,
    matrix: bool,
    /// llama.cpp·ggml 로그를 끈다(기본은 stderr로 낸다 — 장치 속성·레이어 배치·graph splits).
    quiet: bool,
}

fn parse_bench_args(args: &[String]) -> Result<BenchArgs, String> {
    use promptlab_shared::inference::{FlashAttn, KvType};
    let mut out = BenchArgs::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{a} 뒤에 값이 없다"))
        };
        match a.as_str() {
            "--ctx" => out.ctx = Some(value()?.parse().map_err(|_| "--ctx 는 숫자")?),
            "--kv" => {
                out.kv = Some(KvType::parse(&value()?).ok_or("--kv 는 f16|q8_0")?);
            }
            "--fa" => {
                out.fa = Some(FlashAttn::parse(&value()?).ok_or("--fa 는 auto|on|off")?);
            }
            "--layers" => {
                let v = value()?;
                out.layers = Some(if v == "all" {
                    None
                } else {
                    Some(v.parse().map_err(|_| "--layers 는 숫자|all")?)
                });
            }
            "--matrix" => out.matrix = true,
            "--no-llama-log" => out.quiet = true,
            other if !other.starts_with("--") && out.model.is_none() => {
                out.model = Some(other.to_owned());
            }
            other => return Err(format!("모르는 --bench-ai 인자: {other}")),
        }
    }
    Ok(out)
}

/// 지정이 하나라도 있으면 GPU 기본(32k·f16·FA 켬·레이어 전부) 위에 덮은 설정, 없으면 `None`(자동).
fn bench_runtime(a: &BenchArgs) -> Option<promptlab_shared::inference::RuntimeConfig> {
    use promptlab_shared::inference::{CONTEXT_TOKENS, FlashAttn, KvType, RuntimeConfig};
    if a.ctx.is_none() && a.kv.is_none() && a.fa.is_none() && a.layers.is_none() {
        return None;
    }
    Some(RuntimeConfig {
        n_ctx: a.ctx.unwrap_or(CONTEXT_TOKENS),
        kv: a.kv.unwrap_or(KvType::F16),
        flash_attn: a.fa.unwrap_or(FlashAttn::On),
        gpu_layers: a.layers.unwrap_or(None),
    })
}

fn print_vram(label: &str) {
    match promptlab_inference::gpu_memory() {
        Ok(Some((free, total))) => println!(
            "[AI] GPU 메모리({label}): 여유 {} MiB / 전체 {} MiB",
            free >> 20,
            total >> 20
        ),
        Ok(None) => println!("[AI] GPU 메모리({label}): GPU 없음"),
        Err(e) => println!("[AI] GPU 메모리({label}): 모름 — {e}"),
    }
}

/// AI 도우미 속도 진단: 모델을 올리고(sha256 확인 포함) 고정 대화로 두 차례(두 번째는 앞 차례 KV 재사용) 생성한다.
/// `--matrix`면 설정 조합마다 모델을 다시 올려 한 차례씩 돌리고 표로 찍는다(sha256 확인은 처음 한 번).
async fn bench_ai(args: &[String]) -> Result<bool, String> {
    use promptlab_app::local::LocalModel;

    let args = parse_bench_args(args)?;
    // 백엔드를 올리기 전에(아래 첫 장치 조회가 올린다).
    promptlab_inference::set_llama_log(!args.quiet);
    let path = args
        .model
        .clone()
        .map(|m| match promptlab_shared::inference::model_by_id(&m) {
            Some(pin) => promptlab_inference::model_path(pin),
            None => std::path::PathBuf::from(m),
        })
        .unwrap_or_else(cli_model_path);
    println!("[AI] 모델: {}", path.display());
    println!(
        "[AI] 빌드 백엔드: {}",
        if promptlab_inference::GPU_BUILD {
            "vulkan(동적 모듈)"
        } else {
            "cpu"
        }
    );
    match promptlab_inference::devices() {
        Ok(devices) => {
            for d in devices {
                println!(
                    "[AI] 장치: backend={} name={} desc={:?} gpu={} memory_mib={}",
                    d.backend,
                    d.name,
                    d.description,
                    d.gpu,
                    d.memory_total >> 20
                );
            }
        }
        Err(e) => println!("[AI] 장치 목록 실패: {e}"),
    }
    // Vulkan·ggml 환경(#116): 레이어(검증·GPU-assisted 검증)가 켜져 있으면 셰이더가 몇 배 느려진다 — vkconfig 재정의 파일도 본다.
    for (k, v) in std::env::vars() {
        if k.starts_with("VK_") || k.starts_with("GGML_") || k.starts_with("LLAMA_") {
            println!("[AI] 환경: {k}={v}");
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let f = std::path::Path::new(&local).join(r"LunarG\vkconfig\override\VkLayerOverride.json");
        println!(
            "[AI] vkconfig 재정의 파일 {}: {}",
            f.display(),
            if f.exists() {
                "있음(검증 레이어가 켜졌을 수 있다)"
            } else {
                "없음"
            }
        );
    }
    print_vram("올리기 전");
    if args.matrix {
        return bench_matrix(&path).await;
    }
    let t = Instant::now();
    let mut model = LocalModel::load_with(path, bench_runtime(&args))
        .await
        .map_err(|e| e.to_string())?;
    model.set_perf_log(None);
    println!(
        "[AI] 올리기(sha256 확인 포함): {:.1}초",
        t.elapsed().as_secs_f64()
    );
    println!("[AI] 설정: {}", model.inner().load_note());
    let (on, all) = model.inner().gpu_layers();
    println!(
        "[AI] GPU 레이어 {on}/{all}{}",
        if model.inner().gpu_short() {
            " — 그래픽 메모리 부족(처음 고른 설정이 느려 레이어를 줄였다, 앱은 AI 칸에 안내를 띄운다)"
        } else {
            ""
        }
    );
    print_vram("올린 뒤");
    match model.inner().probe_speed() {
        Ok(s) => println!(
            "[AI] 속도 재기(앱이 올린 직후 넘침을 가리는 값, {SPILL_TOKENS_PER_SEC} 미만이면 넘침): {s:.1} tok/s"
        ),
        Err(e) => println!("[AI] 속도 재기 실패: {e}"),
    }
    for turn in 1..=2 {
        bench_turn(&model, turn).await?;
    }
    Ok(true)
}

/// 고정 대화(차례 `turn`까지)를 돌리고 (첫 토큰 ms, 통계)를 돌려준다. 차례 2는 차례 1의 답을 대화에 넣고 묻는다.
async fn bench_turn(
    model: &promptlab_app::local::LocalModel<promptlab_inference::LlamaInference>,
    turn: usize,
) -> Result<(u128, Option<promptlab_shared::inference::GenStats>), String> {
    use promptlab_app::local::Cancel;
    use promptlab_shared::ai::{Message, context_block, enabled_tools, render_with_context};
    use promptlab_shared::inference::SamplingParams;
    let tools = enabled_tools();
    let block = context_block(Some(BENCH_PROBLEM), Language::Python, BENCH_CODE);
    let params = SamplingParams {
        max_tokens: 256,
        ..SamplingParams::default()
    };
    let mut messages = vec![Message::User {
        text: "이 코드에 버그가 있나요? 짧게 설명해 주세요.".into(),
    }];
    if turn > 1 {
        messages.push(Message::Assistant {
            text: BENCH_TURN1_REPLY.into(),
            tool_calls: Vec::new(),
        });
        messages.push(Message::User {
            text: "고친 코드를 한 줄로 알려 주세요.".into(),
        });
    }
    let at = messages.len() - 1;
    let prompt = render_with_context(None, &messages, &tools, Some((at, block.as_str())));
    let started = Instant::now();
    let mut first: Option<Duration> = None;
    let g = model
        .stream(prompt, params, &Cancel::new(), |_, n| {
            if n > 0 && first.is_none() {
                first = Some(started.elapsed());
            }
        })
        .await
        .map_err(|e| e.to_string())?;
    let total = started.elapsed().as_secs_f64();
    let first_ms = first.map_or(0, |d| d.as_millis());
    match &g.stats {
        Some(stats) => println!(
            "[AI] 차례 {turn}: 전체 {total:.1}초, 첫 토큰 {first_ms}ms\n[AI]   {}",
            stats.line()
        ),
        None => println!("[AI] 차례 {turn}: 전체 {total:.1}초(통계 없음)"),
    }
    let preview: String = g.text.chars().take(120).collect();
    println!("[AI]   답 앞부분: {preview:?}");
    Ok((first_ms, g.stats))
}

/// 차례 2가 대화에 넣는 차례 1의 답(고정 — 설정마다 같은 프롬프트로 재게).
const BENCH_TURN1_REPLY: &str =
    "네. `seen[n] = i`를 검사보다 먼저 해서 같은 원소를 두 번 쓸 수 있습니다.";

/// 설정 조합마다 모델을 다시 올려 한 차례씩 잰다.
async fn bench_matrix(path: &std::path::Path) -> Result<bool, String> {
    use promptlab_app::local::LocalModel;
    use promptlab_inference::LlamaInference;
    use promptlab_shared::inference::{FlashAttn, KvType, RuntimeConfig};
    let t = Instant::now();
    promptlab_inference::verify_model_file(path).map_err(|e| e.to_string())?;
    println!("[AI] sha256 확인: {:.1}초", t.elapsed().as_secs_f64());
    let cfg = |n_ctx, kv, flash_attn| {
        Some(RuntimeConfig {
            n_ctx,
            kv,
            flash_attn,
            gpu_layers: None,
        })
    };
    let combos: [(&str, Option<RuntimeConfig>); 8] = [
        ("자동", None),
        ("32k f16 fa=auto", cfg(32_768, KvType::F16, FlashAttn::Auto)),
        ("32k f16 fa=off", cfg(32_768, KvType::F16, FlashAttn::Off)),
        ("32k f16 fa=on", cfg(32_768, KvType::F16, FlashAttn::On)),
        ("32k q8_0 fa=on", cfg(32_768, KvType::Q8_0, FlashAttn::On)),
        ("16k q8_0 fa=on", cfg(16_384, KvType::Q8_0, FlashAttn::On)),
        ("8k f16 fa=on", cfg(8_192, KvType::F16, FlashAttn::On)),
        ("8k q8_0 fa=on", cfg(8_192, KvType::Q8_0, FlashAttn::On)),
    ];
    let mut rows = Vec::new();
    for (name, runtime) in combos {
        println!("\n[AI] === {name} ===");
        let p = path.to_owned();
        let loaded =
            tokio::task::spawn_blocking(move || LlamaInference::load_with(&p, true, runtime))
                .await
                .map_err(|e| e.to_string())?;
        let model = match loaded {
            Ok(m) => LocalModel::new(m),
            Err(e) => {
                println!("[AI] 올리기 실패: {e}");
                rows.push(format!("| {name} | 실패 | - | - | - | - | - |"));
                continue;
            }
        };
        println!("[AI] 설정: {}", model.inner().load_note());
        let (first_ms, stats) = bench_turn(&model, 1).await?;
        rows.push(match stats {
            Some(s) => format!(
                "| {name} | n_ctx={} kv={} fa={} layers={}/{} | {} | {:.1} | {:.1} | {first_ms} | {:.1} / {:.2} / {:.2} |",
                s.n_ctx,
                s.kv,
                s.flash_attn,
                s.gpu_layers,
                s.layers,
                s.vram_mib.map_or("-".to_owned(), |v| v.0.to_string()),
                s.prompt_tok_s(),
                s.generate_tok_s(),
                s.per_token(s.decode_ms),
                s.per_token(s.sample_ms),
                s.per_token(s.text_ms),
            ),
            None => format!("| {name} | 통계 없음 | - | - | - | {first_ms} | - |"),
        });
        drop(model);
    }
    println!(
        "\n| 조합 | 실제 설정 | 생성 뒤 VRAM 여유 MiB | 처리 tok/s | 생성 tok/s | 첫 토큰 ms | 토큰당 decode/샘플/글 ms |"
    );
    println!("|---|---|---|---|---|---|---|");
    for r in rows {
        println!("{r}");
    }
    Ok(true)
}

/// 화면과 같은 고르기(#131): 이 PC 사양·받아 둔 모델·설정 파일.
fn model_pick() -> promptlab_app::model_pick::ModelPick {
    use promptlab_app::model_pick::{MODEL_PREFS_FILE, ModelPick, ModelPrefs};
    ModelPick {
        machine: promptlab_inference::machine().unwrap_or(
            promptlab_shared::model_choice::Machine {
                gpu: None,
                free_ram: 0,
            },
        ),
        downloaded: promptlab_inference::downloaded_models(),
        prefs: ModelPrefs::load(&proclog::data_dir().join(MODEL_PREFS_FILE)),
    }
}

/// 쓸 모델 파일: `PROMPTLAB_MODEL_PATH`, 없으면 화면과 같은 고른(또는 추천) 모델의 캐시 경로.
fn cli_model_path() -> std::path::PathBuf {
    std::env::var_os("PROMPTLAB_MODEL_PATH")
        .map(Into::into)
        .unwrap_or_else(|| promptlab_inference::model_path(model_pick().chosen().model))
}

/// 추천만 찍는다(#131). 마지막 줄 `[모델] 쓸 모델: <id> …`.
fn recommend_model() -> Result<bool, String> {
    use promptlab_shared::model_choice::gb;
    let pick = model_pick();
    match pick.machine.gpu {
        Some(g) => println!(
            "[모델] 그래픽 메모리: 전체 {} · 쓸 수 있음 {}",
            gb(g.total),
            gb(g.usable)
        ),
        None => println!("[모델] 그래픽 메모리: GPU 없음(또는 CPU 빌드)"),
    }
    println!("[모델] 여유 RAM: {}", gb(pick.machine.free_ram));
    for m in promptlab_shared::inference::MODELS.iter() {
        println!("[모델] 후보 {}: {}", m.id, pick.option_label(m));
    }
    println!("[모델] {}", pick.headline());
    if let Some(line) = pick.chosen_line() {
        println!("[모델] {line}");
    }
    if let Some(notice) = pick.limited_notice() {
        println!("[모델] 주의: {notice}");
    }
    let chosen = pick.chosen();
    println!(
        "[모델] 쓸 모델: {} ({}) — {}",
        chosen.model.id,
        if pick.by_user() {
            "직접 고름"
        } else {
            "자동"
        },
        promptlab_inference::model_path(chosen.model).display()
    );
    Ok(true)
}

async fn run() -> Result<bool, String> {
    // 설치된 앱의 서버 주소 기본값 확인(스모크 — 로그인·네트워크 없이).
    if std::env::args().nth(1).as_deref() == Some("--print-server-url") {
        println!(
            "{}",
            Config::from_env().map_err(|e| e.to_string())?.server_url
        );
        return Ok(true);
    }
    // 설치된 앱이 어느 장치로 추론하는지(스모크 — 모델·로그인·네트워크 없이, #111).
    if std::env::args().nth(1).as_deref() == Some("--check-gpu") {
        return check_gpu();
    }
    // AI 모델 크기 추천(로그인·서버·모델 없이, #131).
    if std::env::args().nth(1).as_deref() == Some("--recommend-model") {
        return recommend_model();
    }
    // AI 도우미 속도 진단(모델만, 로그인·서버 없이, #116).
    if std::env::args().nth(1).as_deref() == Some("--bench-ai") {
        let rest: Vec<String> = std::env::args().skip(2).collect();
        return bench_ai(&rest).await;
    }
    let args = parse_args()?;
    if args.check_docker && args.no_docker {
        return Err(format!(
            "--check-docker 와 --no-docker 는 함께 못 쓴다\n{USAGE}"
        ));
    }
    let api = ApiClient::new(Config::from_env().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    if args.list {
        return list(&api).await;
    }
    let model_path = cli_model_path();
    let language = args.language.unwrap_or(Language::Python);

    // 1. 로컬 Docker 점검(시험 시간 밖에서) — 그 언어 이미지(고정 digest)를 받고 빈 하네스를 한 번 돌린다.
    let docker = if args.no_docker {
        println!("[Docker] 건너뜀(--no-docker)");
        None
    } else {
        let docker = LocalDocker::detect().ok();
        let mut check = DockerCheck::new(&[language]);
        let mut last = String::new();
        docker_check::drive(docker.as_ref(), &mut check, |c| {
            let line = c.status_text();
            if line != last && !line.contains("받는 중") {
                println!("[Docker] {line}");
                last = line;
            }
        })
        .await;
        if let Some(e) = check.blocker() {
            if check.needs_install_guide() {
                println!(
                    "[Docker] 설치 안내: {} — {}",
                    localrun::INSTALL_URL,
                    localrun::LICENSE_NOTE
                );
            }
            return Err(format!("Docker 점검 실패: {e}"));
        }
        docker
    };

    if args.check_docker {
        println!("[Docker] {language} 준비됨");
        return Ok(true);
    }

    let mut session = Session::new();
    let apply = |s: &mut Session, e: SessionEvent| s.apply(e).map_err(|e| e.to_string());

    apply(&mut session, SessionEvent::Start)?;
    println!("[점검] 서버 {}", api.config().server_url);
    // 시험 문항이면 과정 로그(#9)를 남긴다 — 화면 앱과 같은 설치 키·로컬 쌓기·올리기.
    let plog = match &args.item {
        Some(item) => {
            let book = LogBook::open(&proclog::data_dir()).map_err(|e| e.to_string())?;
            let log = book
                .attempt(Attempt {
                    assignment_id: item.assignment_id.clone(),
                    position: item.position,
                })
                .map_err(|e| e.to_string())?;
            Some((book, log))
        }
        None => None,
    };
    let note = |event: LogEvent| -> Result<(), String> {
        match &plog {
            Some((_, log)) => log
                .lock()
                .expect("과정 로그 잠금")
                .record_now(event)
                .map_err(|e| format!("과정 로그를 쓰지 못했다: {e}")),
            None => Ok(()),
        }
    };
    // 2. 문제 받기(시험이면 문항 시작 — 모델 파일 sha256 신고).
    let event = match args.item {
        Some(item) => exam::check_item(&api, model_path, item).await,
        None => {
            let id = args.problem.as_deref().unwrap_or(&api.config().problem_id);
            exam::check(&api, id).await
        }
    };
    apply(&mut session, event)?;
    let Some(problem) = session.problem().cloned() else {
        return Err(session.notice().unwrap_or("점검 실패").to_owned());
    };
    note(LogEvent::Started)?;
    println!("[문제] {} — {}", problem.id, problem.title);
    println!(
        "       예제 {}개 · 케이스당 {}ms · 언어 {}",
        problem.examples.len(),
        problem.limits.case_time_ms,
        session
            .allowed_languages()
            .iter()
            .map(|l| l.as_str())
            .collect::<Vec<_>>()
            .join("·")
    );
    if !session.allowed_languages().contains(&language) {
        return Err(format!("이 시험에서는 {language}을(를) 쓸 수 없다"));
    }

    apply(
        &mut session,
        SessionEvent::ConsentGiven { at: Instant::now() },
    )?;
    let code = match args.code {
        Some(code) => code,
        None => template(&problem, language),
    };
    note(LogEvent::LanguageChosen { language })?;
    let mut clock = SnapshotClock::new();
    if let Some(e) = clock.force(Instant::now(), &code) {
        note(e)?;
    }

    // 3. 예제 테스트(로컬 Docker).
    if let Some(docker) = &docker {
        match docker.run_examples(&problem, language, &code).await {
            Ok(report) => {
                println!("[예제 테스트] {}", report.summary());
                if let Some(msg) = &report.compile_error {
                    println!("       {}", msg.lines().next().unwrap_or_default());
                }
                for (i, c) in report.cases.iter().enumerate() {
                    println!(
                        "       예제 {}: {} · 반환 {} · 기대 {} · {}ms",
                        i + 1,
                        match c.ok {
                            Some(true) => "통과",
                            _ => localrun::status_text(c.status),
                        },
                        c.got.as_deref().unwrap_or("-"),
                        c.expected.as_deref().unwrap_or("-"),
                        c.ms
                    );
                }
                note(report.event())?;
            }
            Err(e) => {
                println!("[예제 테스트] {e}");
                note(localrun::failed_event(LocalRunKind::Examples, &e))?;
            }
        }
    }

    // 4. 제출.
    println!("[제출] {language} 코드 {}바이트를 낸다", code.len());
    apply(
        &mut session,
        SessionEvent::Submit {
            at: Instant::now(),
            language,
            code,
        },
    )?;
    let event = exam::submit(&api, &session, plog.as_ref().map(|(_, log)| log))
        .await
        .ok_or("제출 상태가 아니다")?;
    apply(&mut session, event)?;
    if let Some((book, _)) = &plog {
        match book.flush_all(&api).await {
            Ok(n) => println!("[로그] 과정 로그 {n}건을 올렸다"),
            Err(e) => println!(
                "[로그] 과정 로그를 올리지 못했다({e}) — 쌓아 두었다가 다음 실행 때 올린다"
            ),
        }
    }
    if let Phase::Grading { submission_id } = session.phase() {
        println!("[제출] {submission_id} — 채점을 기다린다");
    }

    let deadline = Instant::now() + WAIT_LIMIT;
    while matches!(session.phase(), Phase::Grading { .. }) {
        if Instant::now() > deadline {
            return Err("채점을 기다리다 시간이 다 됐다".into());
        }
        tokio::time::sleep(POLL).await;
        if let Some(event) = exam::poll(&api, &session).await {
            apply(&mut session, event)?;
        }
    }

    match session.phase() {
        Phase::Graded { report, .. } => {
            println!(
                "[결과] {} 통과 {}/{} (통과율 {:.0}%)",
                report.language,
                report.passed,
                report.total,
                report.pass_rate() * 100.0,
            );
            Ok(true)
        }
        Phase::Completed => {
            println!("[결과] 응시 완료 — 이 시험의 결과는 고객사만 본다");
            Ok(true)
        }
        Phase::GradingFailed { reason } => {
            println!("[결과] 채점 실패: {reason}");
            Ok(false)
        }
        Phase::InProgress => Err(session.notice().unwrap_or("제출 실패").to_owned()),
        other => Err(format!("예상 밖 상태: {}", other.name())),
    }
}
