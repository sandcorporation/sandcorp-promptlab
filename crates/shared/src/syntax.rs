//! 가벼운 구문 점검(#125): AI 편집 제안을 적용한 코드가 **적용 전보다** 명백히 깨지는지 본다. 완전한 파서가 아니다 —
//! 오탐(멀쩡한 코드를 깨졌다고 하기)을 피하는 쪽으로 좁게 센다.
//!
//! 세는 것:
//! - **짝**(네 언어): `()`·`[]`·`{}`의 남는 여는 괄호·짝 없는 닫는 괄호·엇갈린 짝, 닫히지 않은 문자열·블록 주석.
//!   문자열·주석 안은 건너뛴다 — Python `#`·`'`·`"`·`'''`·`"""`, JavaScript `//`·`/* */`·`'`·`"`·템플릿 `` ` ``(안의
//!   `${…}`는 코드로 센다), Java·C++ `//`·`/* */`·`"`·`'`(Java 텍스트 블록 `"""`, C++ 숫자 구분 `1'000`은 문자 아님, C++ 원시 문자열
//!   `R"d(…)d"`). JavaScript 정규식 리터럴은 따로 다루지 않는다(`/[(]/` 같은 것은 괄호로 센다 — 그래서 아래 비교).
//! - **들여쓰기**(Python만): ① 빈 블록 — 괄호 밖에서 `:`로 끝나는 줄 뒤에 더 깊이 들여 쓴 코드 줄이 없다(`def f():`
//!   다음 줄이 바로 밖으로 나감), ② 예상 밖 들여쓰기 — `:`로 끝나지 않은 논리 줄 다음 논리 줄이 더 깊다(함수 머리
//!   `def …:`만 지우고 본문을 남긴 제안), 첫 코드 줄이 들여 쓰여 있다.
//!
//! - **풀이 함수 머리**: 공개 서명의 함수 이름으로 찾은 머리 줄([`header_lines`])이 줄었다 — 괄호 짝은 맞는 채로
//!   머리·닫는 줄을 지우고 본문만 넣은 제안(#125 Windows 재현)을 잡는다.
//!
//! 판정은 [`regression`] — 적용 뒤 코드의 어느 항목 수가 적용 전보다 **늘었을 때만** 깨졌다고 본다. 응시자 코드가 이미
//! 깨져 있거나(쓰는 중) 위 규칙이 모르는 문법이 원래 있던 것이면 같은 수가 양쪽에 있어 걸리지 않는다.

use crate::coding::Language;

/// 코드 하나를 센 결과.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyntaxReport {
    /// 짝이 맞지 않는 괄호·닫히지 않은 문자열·주석 수.
    pub unbalanced: u32,
    /// 본문 없는 블록 머리 + 예상 밖 들여쓰기 수(Python).
    pub indentation: u32,
}

impl SyntaxReport {
    pub fn is_clean(&self) -> bool {
        *self == Self::default()
    }
}

/// 적용 뒤 코드가 적용 전보다 나빠진 점.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Breakage {
    /// 풀이 함수 머리 줄(공개 서명의 함수 이름 — [`header_lines`])이 사라졌다. 괄호 짝은 맞을 수 있다 — 머리·닫는 줄을
    /// 함께 지우고 본문만 넣은 제안(#125 Windows 재현).
    MissingHeader,
    /// 괄호·따옴표 짝이 더 어긋났다(함수 머리나 닫는 괄호를 빠뜨린 제안이 흔하다).
    Unbalanced,
    /// 들여쓰기가 더 어긋났다 — 본문 없는 블록 머리·예상 밖 들여쓰기(Python).
    Indentation,
}

impl Breakage {
    /// 응시자에게 보이는 경고(편집 제안 카드).
    pub fn warning(self) -> &'static str {
        match self {
            Breakage::MissingHeader => {
                "이 제안을 적용하면 풀이 함수의 머리 줄(함수 선언)이 사라집니다. 수락하면 실행되지 않습니다."
            }
            Breakage::Unbalanced => {
                "이 제안을 적용하면 괄호나 따옴표의 짝이 맞지 않게 됩니다. 함수 머리나 닫는 괄호가 빠졌는지 확인하세요."
            }
            Breakage::Indentation => {
                "이 제안을 적용하면 들여쓰기가 맞지 않게 됩니다(함수 머리가 빠졌거나 본문이 없는 블록). 확인하세요."
            }
        }
    }
}

/// `before`에 제안을 적용한 `after`가 더 깨졌나(모듈 문서). `function` = 공개 서명의 풀이 함수 이름 — 주면 그 머리 줄
/// 수가 줄었는지도 본다.
pub fn regression(
    language: Language,
    function: Option<&str>,
    before: &str,
    after: &str,
) -> Option<Breakage> {
    if let Some(name) = function
        && header_lines(language, name, after).len() < header_lines(language, name, before).len()
    {
        return Some(Breakage::MissingHeader);
    }
    let (b, a) = (check(language, before), check(language, after));
    if a.unbalanced > b.unbalanced {
        Some(Breakage::Unbalanced)
    } else if a.indentation > b.indentation {
        Some(Breakage::Indentation)
    } else {
        None
    }
}

/// 풀이 함수 `name`의 머리 줄들(1부터 번호, 줄 글). 언어별 꼴(주석 줄은 빼고):
/// - Python: `def name(`·`async def name(`
/// - JavaScript: `function name(`(`async`·`export` 앞붙이 포함), `const|let|var name =`
/// - Java·C++: 앞에 형(낱말·`>`·`]`·`*`·`&`로 끝남)이 있는 `name(`이고 `;`로 끝나지 않는다 — `return name(`·`x = name(`·
///   `if (name(` 같은 부름은 아니다.
pub fn header_lines<'a>(language: Language, name: &str, code: &'a str) -> Vec<(u32, &'a str)> {
    code.split('\n')
        .enumerate()
        .filter(|(_, l)| is_header(language, name, l))
        .map(|(i, l)| (i as u32 + 1, l.trim_end_matches('\r')))
        .collect()
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

fn is_header(language: Language, name: &str, line: &str) -> bool {
    let t = line.trim();
    let comment = match language {
        Language::Python => t.starts_with('#'),
        _ => t.starts_with("//") || t.starts_with("/*") || t.starts_with('*'),
    };
    if comment || name.is_empty() {
        return false;
    }
    if language == Language::Javascript {
        for kw in ["const ", "let ", "var "] {
            if let Some(rest) = t.strip_prefix(kw)
                && rest
                    .trim_start()
                    .strip_prefix(name)
                    .is_some_and(|r| r.trim_start().starts_with('='))
            {
                return true;
            }
        }
    }
    let call = format!("{name}(");
    let mut from = 0;
    while let Some(at) = t[from..].find(&call).map(|i| i + from) {
        from = at + call.len();
        if t[..at].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        let prefix = t[..at].trim_end();
        let ok = match language {
            Language::Python => prefix == "def" || prefix == "async def",
            Language::Javascript => {
                let p = prefix
                    .trim_start_matches("export ")
                    .trim_start_matches("async ");
                p == "function" || p == "function*"
            }
            Language::Java | Language::Cpp => {
                let last_word = prefix.rsplit(|c: char| !is_ident(c)).next().unwrap_or("");
                !t.ends_with(';')
                    && prefix
                        .chars()
                        .next_back()
                        .is_some_and(|c| is_ident(c) || matches!(c, '>' | ']' | '*' | '&'))
                    && !matches!(last_word, "return" | "new" | "else" | "case" | "throw")
            }
        };
        if ok {
            return true;
        }
    }
    false
}

/// 코드 하나를 센다(모듈 문서).
pub fn check(language: Language, code: &str) -> SyntaxReport {
    let mut s = Scanner::new(language, code);
    s.run();
    SyntaxReport {
        unbalanced: s.unbalanced,
        indentation: if language == Language::Python {
            empty_blocks(&s.lines) + unexpected_indents(&s.lines)
        } else {
            0
        },
    }
}

/// 줄 하나에서 Python 빈 블록 판정에 쓰는 것.
#[derive(Debug, Clone, Default)]
struct LineInfo {
    /// 괄호 밖·문자열 밖에서 시작한 줄(논리 줄의 첫 줄)이다.
    starts_logical: bool,
    /// 들여쓰기 폭(공백 1, 탭 8).
    indent: usize,
    /// 주석·공백이 아닌 글자가 있다(문자열 안 글자 포함).
    has_code: bool,
    /// 괄호 밖·문자열 밖에서 끝났고 마지막 의미 있는 글자가 `:`이다.
    ends_with_colon: bool,
}

fn empty_blocks(lines: &[LineInfo]) -> u32 {
    let mut count = 0;
    // 지금 논리 줄의 들여쓰기(여러 줄에 걸친 머리는 첫 줄의 들여쓰기).
    let mut logical_indent = 0;
    for (i, line) in lines.iter().enumerate() {
        if line.starts_logical && line.has_code {
            logical_indent = line.indent;
        }
        if !line.ends_with_colon {
            continue;
        }
        let body = lines[i + 1..]
            .iter()
            .find(|l| l.has_code && l.starts_logical);
        if body.is_none_or(|b| b.indent <= logical_indent) {
            count += 1;
        }
    }
    count
}

/// 앞 논리 줄이 `:`로 끝나지 않았는데 더 깊이 들여 쓴 논리 줄 수(첫 논리 줄이 들여 쓰였으면 그것도 하나).
fn unexpected_indents(lines: &[LineInfo]) -> u32 {
    let mut count = 0;
    // (들여쓰기, `:`로 끝남) — 지금 논리 줄과 그 앞 논리 줄.
    let mut prev: Option<(usize, bool)> = None;
    let mut cur: Option<(usize, bool)> = None;
    for line in lines {
        if line.starts_logical && line.has_code {
            if cur.is_some() {
                prev = cur;
            }
            let ok = match prev {
                None => line.indent == 0,
                Some((indent, colon)) => colon || line.indent <= indent,
            };
            if !ok {
                count += 1;
            }
            cur = Some((line.indent, false));
        }
        if line.ends_with_colon
            && let Some(c) = cur.as_mut()
        {
            c.1 = true;
        }
    }
    count
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Open {
    Paren,
    Bracket,
    Brace,
    /// JavaScript 템플릿의 `${` — 짝 `}`를 만나면 템플릿 글로 돌아간다.
    TemplateExpr,
}

struct Scanner {
    lang: Language,
    chars: Vec<char>,
    i: usize,
    stack: Vec<Open>,
    unbalanced: u32,
    lines: Vec<LineInfo>,
    line: LineInfo,
    /// 줄 끝 판정용: 이 줄의 마지막 의미 있는 글자(문자열 밖).
    last: Option<char>,
}

impl Scanner {
    fn new(lang: Language, src: &str) -> Self {
        let mut s = Self {
            lang,
            chars: src.chars().collect(),
            i: 0,
            stack: Vec::new(),
            unbalanced: 0,
            lines: Vec::new(),
            line: LineInfo::default(),
            last: None,
        };
        s.begin_line(true);
        s
    }

    fn peek(&self, k: usize) -> Option<char> {
        self.chars.get(self.i + k).copied()
    }

    fn starts_with(&self, pat: &str) -> bool {
        pat.chars()
            .enumerate()
            .all(|(k, c)| self.peek(k) == Some(c))
    }

    /// 줄을 시작한다: 들여쓰기를 센다. `logical` = 괄호·문자열 밖에서 시작했다.
    fn begin_line(&mut self, logical: bool) {
        let mut indent = 0;
        let mut k = self.i;
        while let Some(&c) = self.chars.get(k) {
            match c {
                ' ' => indent += 1,
                '\t' => indent += 8,
                _ => break,
            }
            k += 1;
        }
        self.line = LineInfo {
            starts_logical: logical,
            indent,
            ..LineInfo::default()
        };
        self.last = None;
    }

    /// 줄바꿈 하나를 지난다(`self.i`는 `\n` 위).
    fn newline(&mut self, in_string: bool) {
        let at_top = !in_string && self.stack.is_empty();
        self.line.ends_with_colon = at_top && self.last == Some(':');
        // 백슬래시 줄 잇기(Python)는 다음 줄이 논리 줄의 이어짐이다.
        let continued = self.last == Some('\\');
        self.lines.push(std::mem::take(&mut self.line));
        self.i += 1;
        self.begin_line(at_top && !continued);
    }

    fn mark(&mut self, c: char) {
        self.line.has_code = true;
        self.last = Some(c);
    }

    fn run(&mut self) {
        while let Some(c) = self.peek(0) {
            if c == '\n' {
                self.newline(false);
                continue;
            }
            if c.is_whitespace() {
                self.i += 1;
                continue;
            }
            if self.comment() || self.string() {
                continue;
            }
            self.mark(c);
            match c {
                '(' => self.stack.push(Open::Paren),
                '[' => self.stack.push(Open::Bracket),
                '{' => self.stack.push(Open::Brace),
                ')' => self.close(Open::Paren),
                ']' => self.close(Open::Bracket),
                '}' => {
                    if self.stack.last() == Some(&Open::TemplateExpr) {
                        self.stack.pop();
                        self.i += 1;
                        self.template_body();
                        continue;
                    }
                    self.close(Open::Brace)
                }
                _ => {}
            }
            self.i += 1;
        }
        self.line.ends_with_colon = self.stack.is_empty() && self.last == Some(':');
        self.lines.push(std::mem::take(&mut self.line));
        self.unbalanced += self.stack.len() as u32;
    }

    fn close(&mut self, want: Open) {
        match self.stack.last() {
            Some(&top) if top == want => {
                self.stack.pop();
            }
            // 엇갈림: 안쪽 것을 닫힌 것으로 치고 한 번만 센다(`(]`).
            Some(_) if self.stack.contains(&want) => {
                while let Some(top) = self.stack.pop() {
                    if top == want {
                        break;
                    }
                    self.unbalanced += 1;
                }
            }
            _ => self.unbalanced += 1,
        }
    }

    /// 주석이면 건너뛰고 `true`.
    fn comment(&mut self) -> bool {
        let python = self.lang == Language::Python;
        if (python && self.peek(0) == Some('#')) || (!python && self.starts_with("//")) {
            while self.peek(0).is_some_and(|c| c != '\n') {
                self.i += 1;
            }
            return true;
        }
        if !python && self.starts_with("/*") {
            self.i += 2;
            loop {
                match self.peek(0) {
                    None => {
                        self.unbalanced += 1;
                        return true;
                    }
                    Some('*') if self.peek(1) == Some('/') => {
                        self.i += 2;
                        return true;
                    }
                    Some('\n') => self.newline(true),
                    Some(_) => self.i += 1,
                }
            }
        }
        false
    }

    /// 문자열이면 건너뛰고 `true`.
    fn string(&mut self) -> bool {
        let c = self.peek(0).unwrap_or(' ');
        match self.lang {
            Language::Python if c == '\'' || c == '"' => {
                self.mark(c);
                let triple = self.peek(1) == Some(c) && self.peek(2) == Some(c);
                if triple {
                    self.i += 3;
                    self.quoted(c, true, 3);
                } else {
                    self.i += 1;
                    self.quoted(c, false, 1);
                }
                true
            }
            Language::Javascript if c == '`' => {
                self.mark(c);
                self.i += 1;
                self.template_body();
                true
            }
            Language::Javascript if c == '\'' || c == '"' => {
                self.mark(c);
                self.i += 1;
                self.quoted(c, false, 1);
                true
            }
            Language::Cpp if c == 'R' && self.peek(1) == Some('"') && !self.ident_before() => {
                self.mark(c);
                self.raw_cpp();
                true
            }
            Language::Cpp if c == '\'' && self.digit_before() => false,
            Language::Java if self.starts_with("\"\"\"") => {
                self.mark(c);
                self.i += 3;
                self.quoted('"', true, 3);
                true
            }
            Language::Java | Language::Cpp if c == '\'' || c == '"' => {
                self.mark(c);
                self.i += 1;
                self.quoted(c, false, 1);
                true
            }
            _ => false,
        }
    }

    fn ident_before(&self) -> bool {
        self.i > 0 && {
            let p = self.chars[self.i - 1];
            p.is_alphanumeric() || p == '_'
        }
    }

    fn digit_before(&self) -> bool {
        self.i > 0 && self.chars[self.i - 1].is_ascii_hexdigit()
    }

    /// 여는 따옴표 뒤부터 닫는 따옴표까지. 한 줄 문자열이 줄 끝에서 끝나면(닫히지 않음) 하나 세고 그 줄에서 멈춘다.
    fn quoted(&mut self, q: char, multiline: bool, width: usize) {
        loop {
            match self.peek(0) {
                None => {
                    self.unbalanced += 1;
                    return;
                }
                Some('\\') => {
                    if self.peek(1) == Some('\n') {
                        self.i += 1;
                        self.newline(true);
                    } else {
                        self.i += 2;
                    }
                }
                Some('\n') if multiline => self.newline(true),
                Some('\n') => {
                    self.unbalanced += 1;
                    return;
                }
                Some(c) if c == q && (0..width).all(|k| self.peek(k) == Some(q)) => {
                    self.i += width;
                    return;
                }
                Some(_) => self.i += 1,
            }
        }
    }

    /// JavaScript 템플릿 글(여는 `` ` `` 뒤 또는 `${…}`의 `}` 뒤)부터 닫는 `` ` ``나 `${`까지.
    fn template_body(&mut self) {
        loop {
            match self.peek(0) {
                None => {
                    self.unbalanced += 1;
                    return;
                }
                Some('\\') => self.i += 2,
                Some('`') => {
                    self.i += 1;
                    return;
                }
                Some('$') if self.peek(1) == Some('{') => {
                    self.i += 2;
                    self.stack.push(Open::TemplateExpr);
                    return;
                }
                Some('\n') => self.newline(true),
                Some(_) => self.i += 1,
            }
        }
    }

    /// C++ 원시 문자열 `R"구분(…)구분"`(`self.i`는 `R` 위).
    fn raw_cpp(&mut self) {
        self.i += 2;
        let mut delim = String::new();
        while let Some(c) = self.peek(0) {
            if c == '(' || c == '\n' || delim.len() > 16 {
                break;
            }
            delim.push(c);
            self.i += 1;
        }
        if self.peek(0) != Some('(') {
            self.unbalanced += 1;
            return;
        }
        self.i += 1;
        let end = format!("){delim}\"");
        loop {
            if self.starts_with(&end) {
                self.i += end.chars().count();
                return;
            }
            match self.peek(0) {
                None => {
                    self.unbalanced += 1;
                    return;
                }
                Some('\n') => self.newline(true),
                Some(_) => self.i += 1,
            }
        }
    }
}
