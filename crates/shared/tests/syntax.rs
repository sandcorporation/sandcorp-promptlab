//! 가벼운 구문 점검(#125): 제안 적용 뒤 코드가 적용 전보다 명백히 깨졌을 때만 잡는다(오탐 없음 쪽).

use promptlab_shared::coding::{Language, Param, Signature, ValueType, harness, solution_template};
use promptlab_shared::syntax::{Breakage, check, regression};

const JS: &str = "/**\n * @param {number[]} nums\n * @return {number[]}\n */\nfunction two_sum(nums, target) {\n    for (let i = 0; i < nums.length; i++) {\n        for (let j = i; j < nums.length; j++) {\n            if (nums[i] + nums[j] === target) return [i, j];\n        }\n    }\n    return [];\n}\n";

/// 이슈 #125 재현: 6~9줄(함수 머리 포함)을 본문만으로 바꾼 제안.
const JS_BROKEN: &str = "/**\n * @param {number[]} nums\n * @return {number[]}\n */\n    for (let i = 0; i < nums.length; i++) {\n        for (let j = i + 1; j < nums.length; j++) {\n            if (nums[i] + nums[j] === target) return [i, j];\n        }\n    }\n    return [];\n}\n";

const JS_FIXED: &str = "/**\n * @param {number[]} nums\n * @return {number[]}\n */\nfunction two_sum(nums, target) {\n    for (let i = 0; i < nums.length; i++) {\n        for (let j = i + 1; j < nums.length; j++) {\n            if (nums[i] + nums[j] === target) return [i, j];\n        }\n    }\n    return [];\n}\n";

#[test]
fn 함수_머리를_지운_js_제안은_짝이_깨진다() {
    assert!(check(Language::Javascript, JS).is_clean());
    assert!(check(Language::Javascript, JS_FIXED).is_clean());
    assert_eq!(
        regression(Language::Javascript, None, JS, JS_BROKEN),
        Some(Breakage::Unbalanced)
    );
    assert_eq!(regression(Language::Javascript, None, JS, JS_FIXED), None);
}

#[test]
fn 이미_깨진_코드는_더_나빠질_때만_잡는다() {
    // 쓰는 중이라 닫는 괄호가 없는 코드 — 제안이 그대로 두면 깨짐이 아니다.
    let before = "function f(a) {\n    return a\n";
    let same = "function f(a) {\n    return a + 1;\n";
    assert_eq!(regression(Language::Javascript, None, before, same), None);
    // 고치면 당연히 아니다.
    assert_eq!(
        regression(
            Language::Javascript,
            None,
            before,
            "function f(a) {\n    return a + 1;\n}\n"
        ),
        None
    );
    // 하나 더 깨면 잡는다.
    assert_eq!(
        regression(
            Language::Javascript,
            None,
            before,
            "function f(a) {\n    return (a + 1;\n"
        ),
        Some(Breakage::Unbalanced)
    );
}

#[test]
fn 문자열과_주석_안의_괄호는_세지_않는다() {
    for (lang, code) in [
        (
            Language::Javascript,
            "const s = \"(\" + '[' + `{${a + (b)}` + /* } */ x; // )\n",
        ),
        (
            Language::Javascript,
            "const t = `line1\n${ {a: 1}.a }\nline3 )`;\n",
        ),
        (
            Language::Python,
            "s = '(' + \"[\"  # )\nt = '''\n}}}\n'''\n",
        ),
        (
            Language::Java,
            "String s = \"(\"; char c = '{'; char q = '\\''; /* ( */ // ]\nString b = \"\"\"\n  )\n  \"\"\";\n",
        ),
        (
            Language::Cpp,
            "auto s = R\"x()\")x\"; char c = '('; long n = 1'000'000; // {\n#include <vector>\n",
        ),
    ] {
        assert!(
            check(lang, code).is_clean(),
            "{lang:?}: {:?}\n{code}",
            check(lang, code)
        );
    }
}

#[test]
fn 엇갈린_괄호와_닫히지_않은_문자열을_센다() {
    assert!(check(Language::Python, "x = (1, 2]\n").unbalanced > 0);
    assert!(check(Language::Python, "s = 'abc\nprint(s)\n").unbalanced > 0);
    assert!(check(Language::Javascript, "/* 끝없는 주석\nfoo();\n").unbalanced > 0);
    assert!(check(Language::Java, "class A { void f() { }\n").unbalanced > 0);
    assert!(check(Language::Cpp, "int f() { return 0; } }\n").unbalanced > 0);
}

#[test]
fn python은_본문_없는_블록_머리를_잡는다() {
    let before = "def two_sum(nums, target):\n    seen = {}\n    for i, n in enumerate(nums):\n        if target - n in seen:\n            return [seen[target - n], i]\n        seen[n] = i\n    return []\n";
    assert!(check(Language::Python, before).is_clean());
    // 본문을 바깥 들여쓰기로 붙인 제안(머리만 남고 본문이 밖으로 나감).
    let after = "def two_sum(nums, target):\nseen = {}\nfor i, n in enumerate(nums):\n    if target - n in seen:\n        return [seen[target - n], i]\n    seen[n] = i\nreturn []\n";
    assert_eq!(
        regression(Language::Python, None, before, after),
        Some(Breakage::Indentation)
    );
    // 함수 머리만 지운 제안(본문이 예상 밖으로 들여 쓰임).
    assert_eq!(
        regression(
            Language::Python,
            None,
            before,
            &before.replacen("def two_sum(nums, target):\n", "", 1)
        ),
        Some(Breakage::Indentation)
    );
    // 맨 끝의 머리.
    assert_eq!(check(Language::Python, "def f():\n").indentation, 1);
    // 오탐이 아닌 것: 여러 줄 머리·사전·슬라이스·람다·형 힌트·독스트링 본문·주석 줄 뒤 본문.
    for ok in [
        "def f(a,\n      b):\n    return a\n",
        "d = {\n    'a': 1,\n}\nx = s[1:]\ng = lambda v: v\ny: int = 3\n",
        "class A:\n    \"\"\"설명:\n    끝\"\"\"\n",
        "if x:\n    # 주석\n    pass\n",
        "if a and \\\n   b:\n    pass\n",
    ] {
        assert!(
            check(Language::Python, ok).is_clean(),
            "{ok}\n{:?}",
            check(Language::Python, ok)
        );
    }
}

/// 앱이 만드는 틀·하네스(네 언어 × 모든 값 타입)는 깨끗하다 — 오탐이 없다는 넓은 확인.
#[test]
fn 생성한_틀과_하네스는_깨끗하다() {
    let sig = Signature {
        name: "solve".into(),
        params: ValueType::ALL
            .iter()
            .enumerate()
            .map(|(i, &ty)| Param {
                name: format!("a{i}"),
                ty,
            })
            .collect(),
        returns: ValueType::Int,
    };
    for lang in Language::ALL {
        for code in [solution_template(lang, &sig), harness(lang, &sig)] {
            let r = check(lang, &code);
            assert!(r.is_clean(), "{lang:?} {r:?}\n{code}");
        }
    }
}

/// 내장 문제의 모범 답안(실제로 통과하는 코드)도 깨끗하다.
#[test]
fn 내장_문제_모범_답안은_깨끗하다() {
    // 문제은행은 PROMPTLAB_PROBLEMS_DIR(운영 문항), 없으면 공개 픽스처 — 스모크 문항은 늘 함께(#138).
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../store/fixtures");
    let bank = std::env::var_os("PROMPTLAB_PROBLEMS_DIR")
        .filter(|v| !v.is_empty())
        .map_or_else(|| root.join("problems"), std::path::PathBuf::from);
    let mut n = 0;
    for dir in [bank, root.join("smoke")] {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "json")
                || path.file_name().is_some_and(|f| f == "index.json")
            {
                continue;
            }
            let v: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let lang: Language =
                serde_json::from_value(v["reference_language"].clone()).unwrap_or(Language::Python);
            let code = v["reference_solution"].as_str().unwrap();
            let r = check(lang, code);
            assert!(r.is_clean(), "{path:?} {r:?}");
            n += 1;
        }
    }
    assert!(n >= 2, "문항 {n}개");
}

/// #125 루프 2: 풀이 함수 머리 줄은 공개 서명의 이름으로 찾는다. 네 언어 틀에 하나씩, 부름·주석은 머리가 아니다.
#[test]
fn 풀이_함수_머리_줄을_찾는다() {
    use promptlab_shared::syntax::header_lines;
    let sig = Signature {
        name: "two_sum".into(),
        params: vec![Param {
            name: "nums".into(),
            ty: ValueType::IntArray,
        }],
        returns: ValueType::IntArray,
    };
    for lang in Language::ALL {
        let code = solution_template(lang, &sig);
        assert_eq!(
            header_lines(lang, "two_sum", &code).len(),
            1,
            "{lang:?}\n{code}"
        );
        assert!(header_lines(lang, "sum", &code).is_empty(), "{lang:?}");
    }
    for (lang, not_header) in [
        (
            Language::Python,
            "    return two_sum(nums[1:])\n# def two_sum(x):\n",
        ),
        (
            Language::Javascript,
            "  return two_sum(nums);\n// function two_sum(a) {\nx = my_two_sum(a);\n",
        ),
        (
            Language::Java,
            "        return two_sum(nums);\n        long[] r = two_sum(nums);\n        if (two_sum(a) == null) {\n        int x = new two_sum(a);\n",
        ),
        (
            Language::Cpp,
            "    return two_sum(v);\n    auto r = two_sum(v);\nvector<long long> two_sum(vector<long long> nums);\n",
        ),
    ] {
        assert!(
            header_lines(lang, "two_sum", not_header).is_empty(),
            "{lang:?}: {:?}",
            header_lines(lang, "two_sum", not_header)
        );
    }
    assert_eq!(
        header_lines(
            Language::Javascript,
            "two_sum",
            "const two_sum = (nums) => {\n"
        )
        .len(),
        1
    );
    assert_eq!(
        header_lines(
            Language::Cpp,
            "two_sum",
            "static std::vector<long long>& two_sum(int a) {\n"
        )
        .len(),
        1
    );
    // 머리가 사라지면 깨짐, 남으면 아니다(괄호 짝이 맞아도).
    let before = solution_template(Language::Java, &sig);
    let after = before.replace("    public long[] two_sum(long[] nums) {\n", "    {\n");
    assert_ne!(before, after);
    assert_eq!(
        regression(Language::Java, Some("two_sum"), &before, &after),
        Some(Breakage::MissingHeader)
    );
    assert_eq!(regression(Language::Java, None, &before, &after), None);
}
