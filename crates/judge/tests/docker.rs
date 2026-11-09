//! 판정기 docker 시험 — 로컬 러너 이미지가 있어야 한다(`#[ignore]`).
//!
//! ```text
//! bash scripts/build-runners.sh --local
//! cargo test -p promptlab-judge --test docker -- --ignored
//! ```
//!
//! - 틀·하네스: 4개 언어 × 10개 타입 — 틀이 이미지 안에서 컴파일되고, 값이 하네스를 왕복한다(유니코드·안전 정수 끝·지수 표기 double).
//! - 표본 문제 하나(두 수의 합)의 4개 언어 모범 답안이 예제·히든을 통과한다.
//! - 케이스 격리(한 케이스 예외), 컴파일 오류.
//! - 악성 코드: 무한 루프 → 시간 초과(다음 케이스는 돈다), fork 폭탄 → pids 상한(실패로 끝남), 메모리 폭주 → 메모리 초과,
//!   네트워크 접속 실패, 루트·이미지 파일 쓰기 실패(nobody), 출력 폭주 → 출력 초과.

use std::time::{Duration, Instant};

use promptlab_judge::{CaseResult, Judge, RunOutcome, RunRequest, Status, passed};
use promptlab_shared::coding::{self, Language, MAX_SAFE_INT, Param, Signature, ValueType};
use serde_json::{Value, json};

fn sig(name: &str, params: &[(&str, ValueType)], returns: ValueType) -> Signature {
    Signature {
        name: name.into(),
        params: params
            .iter()
            .map(|(n, t)| Param {
                name: (*n).into(),
                ty: *t,
            })
            .collect(),
        returns,
    }
}

async fn run(lang: Language, sig: &Signature, code: &str, cases: Vec<Vec<Value>>) -> RunOutcome {
    Judge::default()
        .run(&RunRequest {
            language: lang,
            code: code.into(),
            signature: sig.clone(),
            cases,
            case_time_ms: None,
        })
        .await
        .unwrap_or_else(|e| panic!("{lang}: {e}"))
}

fn statuses(o: &RunOutcome) -> Vec<Status> {
    o.cases.iter().map(|c: &CaseResult| c.status).collect()
}

/// 틀의 빈 몸통을 「첫 인자를 그대로 돌려준다」로 바꾼다.
fn echo_solution(lang: Language, sig: &Signature) -> String {
    let t = coding::solution_template(lang, sig);
    let start = t.find("answer = ").unwrap() + "answer = ".len();
    let end = start + t[start..].find('\n').unwrap();
    let semi = if lang == Language::Python { "" } else { ";" };
    format!("{}x{semi}{}", &t[..start], &t[end..])
}

fn samples(t: ValueType) -> Vec<Value> {
    let s = json!("한글 \"따옴표\" \\ 줄\n탭\t 이모지 😀 제어\u{1}");
    match t {
        ValueType::Int => vec![
            json!(0),
            json!(-5),
            json!(MAX_SAFE_INT),
            json!(-MAX_SAFE_INT),
        ],
        ValueType::Double => vec![json!(1.5), json!(-2.25e-7), json!(1e300), json!(3)],
        ValueType::Bool => vec![json!(true), json!(false)],
        ValueType::String => vec![json!(""), s],
        ValueType::IntArray => vec![json!([]), json!([1, -2, MAX_SAFE_INT])],
        ValueType::DoubleArray => vec![json!([]), json!([0.5, -1e-9, 2])],
        ValueType::BoolArray => vec![json!([]), json!([true, false, true])],
        ValueType::StringArray => vec![json!([]), json!(["a", "", s])],
        ValueType::IntMatrix => vec![json!([]), json!([[1], [], [2, 3]])],
        ValueType::StringMatrix => vec![json!([[]]), json!([["a", "b"], [s]])],
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "docker와 로컬 러너 이미지가 필요하다"]
async fn 틀은_4개_언어_모든_타입에서_컴파일되고_값이_왕복한다() {
    let mut set = tokio::task::JoinSet::new();
    let judge = Judge::default();
    for lang in Language::ALL {
        for t in ValueType::ALL {
            let judge = judge.clone();
            set.spawn(async move {
                let s = sig(
                    "echo_value",
                    &[("x", t), ("other", ValueType::StringMatrix)],
                    t,
                );
                let cases: Vec<Vec<Value>> = samples(t)
                    .into_iter()
                    .map(|v| vec![v, json!([["z"]])])
                    .collect();
                let expected: Vec<Value> = cases.iter().map(|c| c[0].clone()).collect();
                // 틀 그대로도 컴파일되고 기본값을 낸다.
                let tpl = coding::solution_template(lang, &s);
                let req = |code: String| RunRequest {
                    language: lang,
                    code,
                    signature: s.clone(),
                    cases: cases.clone(),
                    case_time_ms: None,
                };
                let o = judge.run(&req(tpl)).await.unwrap();
                assert_eq!(
                    o.compile.as_ref().map(|c| c.ok),
                    Some(true),
                    "{lang} {t} 틀: {o:?}"
                );
                assert!(
                    o.cases.iter().all(|c| c.status == Status::Ok),
                    "{lang} {t} 틀: {o:?}"
                );
                let o = judge.run(&req(echo_solution(lang, &s))).await.unwrap();
                assert_eq!(
                    passed(t, &expected, &o),
                    vec![true; expected.len()],
                    "{lang} {t} 왕복: {o:?}"
                );
            });
        }
    }
    while let Some(r) = set.join_next().await {
        r.unwrap();
    }
}

const TWO_SUM: [(Language, &str); 4] = [
    (
        Language::Python,
        "def two_sum(nums: list[int], target: int) -> list[int]:\n    seen = {}\n    for i, x in enumerate(nums):\n        if target - x in seen:\n            return [seen[target - x], i]\n        seen[x] = i\n    return []\n",
    ),
    (
        Language::Javascript,
        "function two_sum(nums, target) {\n  const seen = new Map();\n  for (let i = 0; i < nums.length; i++) {\n    if (seen.has(target - nums[i])) return [seen.get(target - nums[i]), i];\n    seen.set(nums[i], i);\n  }\n  return [];\n}\n",
    ),
    (
        Language::Java,
        "import java.util.*;\n\nclass Solution {\n    public long[] two_sum(long[] nums, long target) {\n        Map<Long, Integer> seen = new HashMap<>();\n        for (int i = 0; i < nums.length; i++) {\n            Integer j = seen.get(target - nums[i]);\n            if (j != null) return new long[]{j, i};\n            seen.put(nums[i], i);\n        }\n        return new long[]{};\n    }\n}\n",
    ),
    (
        Language::Cpp,
        "#include <unordered_map>\n#include <vector>\nusing namespace std;\n\nvector<long long> two_sum(vector<long long> nums, long long target) {\n    unordered_map<long long, long long> seen;\n    for (long long i = 0; i < (long long)nums.size(); i++) {\n        auto it = seen.find(target - nums[i]);\n        if (it != seen.end()) return {it->second, i};\n        seen[nums[i]] = i;\n    }\n    return {};\n}\n",
    ),
];

#[tokio::test(flavor = "multi_thread")]
#[ignore = "docker와 로컬 러너 이미지가 필요하다"]
async fn 표본_문제의_4개_언어_모범_답안이_통과한다() {
    let s = sig(
        "two_sum",
        &[("nums", ValueType::IntArray), ("target", ValueType::Int)],
        ValueType::IntArray,
    );
    let big: Vec<i64> = (0..100_000).collect();
    let cases = [
        (json!([[2, 7, 11, 15], 9]), json!([0, 1])),
        (json!([[3, 2, 4], 6]), json!([1, 2])),
        (json!([[3, 3], 6]), json!([0, 1])),
        (json!([[-1, -2, -3, -4, -5], -8]), json!([2, 4])),
        (json!([[MAX_SAFE_INT - 1, 1], MAX_SAFE_INT]), json!([0, 1])),
        (json!([big, 199_997]), json!([99_998, 99_999])),
    ];
    let args: Vec<Vec<Value>> = cases
        .iter()
        .map(|(a, _)| a.as_array().unwrap().clone())
        .collect();
    let expected: Vec<Value> = cases.iter().map(|(_, e)| e.clone()).collect();
    for (lang, code) in TWO_SUM {
        let o = run(lang, &s, code, args.clone()).await;
        assert_eq!(
            passed(s.returns, &expected, &o),
            vec![true; 6],
            "{lang}: {o:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "docker와 로컬 러너 이미지가 필요하다"]
async fn 한_케이스의_예외는_그_케이스에만_걸린다() {
    let s = sig("half", &[("n", ValueType::Int)], ValueType::Int);
    let codes = [
        (
            Language::Python,
            "def half(n):\n    if n == 3:\n        raise ValueError('셋')\n    return n // 2\n",
        ),
        (
            Language::Javascript,
            "function half(n) {\n  if (n === 3) throw new Error('셋');\n  return Math.floor(n / 2);\n}\n",
        ),
        (
            Language::Java,
            "class Solution {\n    public long half(long n) {\n        if (n == 3) throw new IllegalStateException(\"셋\");\n        return n / 2;\n    }\n}\n",
        ),
        (
            Language::Cpp,
            "#include <stdexcept>\nlong long half(long long n) {\n    if (n == 3) throw std::runtime_error(\"셋\");\n    return n / 2;\n}\n",
        ),
    ];
    for (lang, code) in codes {
        let o = run(
            lang,
            &s,
            code,
            vec![vec![json!(4)], vec![json!(3)], vec![json!(8)]],
        )
        .await;
        assert_eq!(
            statuses(&o),
            vec![Status::Ok, Status::RuntimeError, Status::Ok],
            "{lang}: {o:?}"
        );
        assert!(
            o.cases[1].message.as_deref().unwrap_or("").contains("셋"),
            "{lang}: {o:?}"
        );
        assert_eq!(o.cases[2].value, Some(json!(4)));
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "docker와 로컬 러너 이미지가 필요하다"]
async fn 컴파일_오류는_모든_케이스가_컴파일_오류() {
    let s = sig("half", &[("n", ValueType::Int)], ValueType::Int);
    for (lang, code) in [
        (Language::Python, "def half(n)\n    return n\n"),
        (Language::Javascript, "function half(n) { return n; "),
        (
            Language::Java,
            "class Solution { public long half(long n) { return n } }",
        ),
        (Language::Cpp, "long long half(long long n) { return n }"),
    ] {
        let o = run(lang, &s, code, vec![vec![json!(1)], vec![json!(2)]]).await;
        let c = o.compile.clone().unwrap();
        assert!(!c.ok && !c.message.is_empty(), "{lang}: {o:?}");
        assert_eq!(statuses(&o), vec![Status::CompileError; 2], "{lang}");
    }
}

fn probe() -> Signature {
    sig("probe", &[("n", ValueType::Int)], ValueType::Bool)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "docker와 로컬 러너 이미지가 필요하다"]
async fn 무한_루프는_시간_초과_다음_케이스는_돈다() {
    for (lang, code) in [
        (
            Language::Python,
            "def probe(n):\n    while n == 1:\n        pass\n    return True\n",
        ),
        (
            Language::Javascript,
            "function probe(n) {\n  while (n === 1) {}\n  return true;\n}\n",
        ),
        (
            Language::Java,
            "class Solution {\n    public boolean probe(long n) {\n        while (n == 1) { n = n * 1; }\n        return true;\n    }\n}\n",
        ),
        (
            Language::Cpp,
            "bool probe(long long n) {\n    volatile long long k = n;\n    while (k == 1) {}\n    return true;\n}\n",
        ),
    ] {
        let start = Instant::now();
        let o = run(lang, &probe(), code, vec![vec![json!(1)], vec![json!(2)]]).await;
        assert_eq!(
            statuses(&o),
            vec![Status::TimeLimit, Status::Ok],
            "{lang}: {o:?}"
        );
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "{lang}: {:?}",
            start.elapsed()
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "docker와 로컬 러너 이미지가 필요하다"]
async fn 메모리_폭주는_메모리_초과() {
    for (lang, code) in [
        (
            Language::Python,
            "def probe(n):\n    xs = []\n    while True:\n        xs.append(bytearray(10_000_000))\n",
        ),
        (
            Language::Javascript,
            "function probe(n) {\n  const xs = [];\n  while (true) xs.push(new Array(1000000).fill(n));\n}\n",
        ),
        (
            Language::Java,
            "import java.util.*;\nclass Solution {\n    public boolean probe(long n) {\n        List<long[]> xs = new ArrayList<>();\n        while (true) xs.add(new long[1_000_000]);\n    }\n}\n",
        ),
        (
            Language::Cpp,
            "#include <vector>\nbool probe(long long n) {\n    std::vector<std::vector<char>> xs;\n    while (true) xs.emplace_back(10000000, 'x');\n}\n",
        ),
    ] {
        let o = run(lang, &probe(), code, vec![vec![json!(1)], vec![json!(2)]]).await;
        assert_eq!(o.cases[0].status, Status::MemoryLimit, "{lang}: {o:?}");
        assert_eq!(
            o.cases[1].status,
            Status::MemoryLimit,
            "{lang}: 다음 케이스도 다시 띄워 돈다 {o:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "docker와 로컬 러너 이미지가 필요하다"]
async fn fork_폭탄은_pids_상한에_막혀_실패로_끝난다() {
    let start = Instant::now();
    let o = run(
        Language::Python,
        &probe(),
        "import os\ndef probe(n):\n    while True:\n        os.fork()\n",
        vec![vec![json!(1)]],
    )
    .await;
    assert_ne!(o.cases[0].status, Status::Ok, "{o:?}");
    assert!(
        start.elapsed() < Duration::from_secs(20),
        "{:?}",
        start.elapsed()
    );
    let o = run(
        Language::Cpp,
        &probe(),
        "#include <unistd.h>\nbool probe(long long n) {\n    while (true) fork();\n}\n",
        vec![vec![json!(1)]],
    )
    .await;
    assert_ne!(o.cases[0].status, Status::Ok, "{o:?}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "docker와 로컬 러너 이미지가 필요하다"]
async fn 네트워크_접속은_실패한다() {
    for (lang, code) in [
        (
            Language::Python,
            "import socket\ndef probe(n):\n    try:\n        socket.create_connection(('1.1.1.1', 53), timeout=1).close()\n        return True\n    except OSError:\n        return False\n",
        ),
        (
            Language::Javascript,
            "function probe(n) {\n  const r = require('child_process').spawnSync('node', ['-e', \"require('net').connect(53,'1.1.1.1').on('connect',()=>process.exit(0)).on('error',()=>process.exit(1))\"], { timeout: 1500 });\n  return r.status === 0;\n}\n",
        ),
    ] {
        let o = run(lang, &probe(), code, vec![vec![json!(1)]]).await;
        assert_eq!(o.cases[0].value, Some(json!(false)), "{lang}: {o:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "docker와 로컬 러너 이미지가 필요하다"]
async fn 루트·이미지_파일은_쓸_수_없다() {
    let code = "import os\ndef probe(n):\n    if os.getuid() != 65534:\n        return True\n    for p in ['/etc/pwned', '/pwned', '/usr/local/bin/promptlab-runner', '/root/x']:\n        try:\n            with open(p, 'w') as f:\n                f.write('x')\n            return True\n        except OSError:\n            pass\n    return False\n";
    let o = run(Language::Python, &probe(), code, vec![vec![json!(1)]]).await;
    assert_eq!(o.cases[0].value, Some(json!(false)), "{o:?}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "docker와 로컬 러너 이미지가 필요하다"]
async fn 출력_폭주는_출력_초과() {
    let o = run(
        Language::Python,
        &probe(),
        "def probe(n):\n    while True:\n        print('x' * 10000)\n",
        vec![vec![json!(1)], vec![json!(2)]],
    )
    .await;
    assert_eq!(o.cases[0].status, Status::OutputLimit, "{o:?}");
    assert!(
        o.stdout.len() <= promptlab_runner_keep(),
        "{}",
        o.stdout.len()
    );
}

fn promptlab_runner_keep() -> usize {
    8 * 1024
}
