//! 원시 색 금지(#41·#65): 대시보드·응시 앱의 소스와 CSS는 토큰(`var(--…)`)만 쓴다.
//! 원시 색은 대시보드의 디자인시스템 복사본(`design/`, 손으로 고치지 않는다)과 응시 앱의 Fluent 토큰 파일
//! (`TOKEN_FILES`, #65)에만 있다 — 그 밖의 파일에 하나라도 있으면 실패한다.
//! 다크 모드는 토큰이 바꾸므로, 원시 색이 하나라도 섞이면 그 자리는 다크 모드에서 읽히지 않는다.
//!
//! 잡는 것: `#rgb`·`#rgba`·`#rrggbb`·`#rrggbbaa`, `rgb(`·`rgba(`·`hsl(`·`hsla(`·`hwb(`·`lab(`·`lch(`·`oklab(`·`oklch(`·`color(`.
//! 주석(`//…`, `/* … */`)은 뺀다 — 이슈 번호(`#41`)가 색처럼 보인다.

use std::path::{Path, PathBuf};

const ROOTS: [&str; 2] = ["crates/dashboard/src", "crates/app/src"];
/// 응시 앱 Fluent 토큰 파일 — 원시 색을 둘 수 있는 유일한 소스(저장소 루트 기준).
const TOKEN_FILES: [&str; 1] = ["crates/app/src/fluent.css"];
const FUNCS: [&str; 10] = [
    "rgb(", "rgba(", "hsl(", "hsla(", "hwb(", "lab(", "lch(", "oklab(", "oklch(", "color(",
];

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("소스 폴더") {
        let path = entry.expect("폴더 항목").path();
        if path.is_dir() {
            files(&path, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("rs" | "css" | "html" | "js")
        ) {
            out.push(path);
        }
    }
}

/// 주석을 공백으로 지운다(줄 번호는 그대로).
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let end = tail.find("*/").map(|e| e + 2).unwrap_or(tail.len());
        out.extend(
            tail[..end]
                .chars()
                .map(|c| if c == '\n' { '\n' } else { ' ' }),
        );
        rest = &tail[end..];
    }
    out.push_str(rest);
    out.lines()
        .map(|line| match line.find("//") {
            // `https://…` 같은 주소 안의 `//`는 주석이 아니다.
            Some(i) if !line[..i].ends_with(':') => &line[..i],
            _ => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 한 줄에서 원시 색을 찾는다.
fn raw_colors(line: &str) -> Vec<String> {
    let mut found = Vec::new();
    let bytes = line.as_bytes();
    for (i, _) in line.match_indices('#') {
        let prev_word = i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
        let hex = bytes[i + 1..]
            .iter()
            .take_while(|b| b.is_ascii_hexdigit())
            .count();
        let next_word = bytes
            .get(i + 1 + hex)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_');
        if !prev_word && !next_word && matches!(hex, 3 | 4 | 6 | 8) {
            found.push(line[i..i + 1 + hex].to_owned());
        }
    }
    let lower = line.to_ascii_lowercase();
    for func in FUNCS {
        for (i, _) in lower.match_indices(func) {
            let prev = lower[..i].chars().next_back();
            if !prev.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                found.push(func.to_owned());
            }
        }
    }
    found
}

#[test]
fn ui_sources_use_design_tokens_only() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut list = Vec::new();
    for root in ROOTS {
        files(&repo.join(root), &mut list);
    }
    let tokens: Vec<PathBuf> = TOKEN_FILES.iter().map(|f| repo.join(f)).collect();
    for t in &tokens {
        assert!(t.is_file(), "토큰 파일이 없다: {}", t.display());
    }
    list.retain(|p| !tokens.contains(p));
    assert!(list.len() > 10, "소스를 찾지 못했다: {list:?}");
    let mut bad = Vec::new();
    for path in &list {
        let text = std::fs::read_to_string(path).expect("소스 읽기");
        for (n, line) in strip_comments(&text).lines().enumerate() {
            for color in raw_colors(line) {
                bad.push(format!("{}:{}: {color}", path.display(), n + 1));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "원시 색 대신 sandcorp-design 토큰(var(--color-*))을 쓴다:\n{}",
        bad.join("\n")
    );
}

#[test]
fn detector_catches_raw_colors_and_skips_issue_refs() {
    assert_eq!(raw_colors("color: #1d2127;"), ["#1d2127"]);
    assert_eq!(raw_colors("background:#fff}"), ["#fff"]);
    assert_eq!(raw_colors("fill: rgba(0, 0, 0, .5)"), ["rgba("]);
    assert_eq!(raw_colors("c: HSL(1 2% 3%)"), ["hsl("]);
    assert!(raw_colors("color: var(--color-ink);").is_empty());
    assert!(raw_colors("#[derive(Debug)] r#\"x\"#; #41 #12345").is_empty());
    assert!(raw_colors("fn background(x)").is_empty());
    assert_eq!(
        strip_comments("a // #fff\nb /* #000\n */ c")
            .lines()
            .count(),
        3
    );
    assert!(raw_colors(&strip_comments("a // #fff /* rgb( */")).is_empty());
}

#[test]
fn design_copy_is_pinned() {
    let design = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../design");
    let version = std::fs::read_to_string(design.join("VERSION")).expect("design/VERSION");
    assert!(
        version.trim().starts_with('v'),
        "design/VERSION은 sandcorp-design 태그다(scripts/sync-design.sh <태그>)"
    );
    let css = std::fs::read_to_string(design.join("sandcorp.css")).expect("design/sandcorp.css");
    assert!(
        css.contains("prefers-color-scheme:dark") || css.contains("prefers-color-scheme: dark")
    );
}
