//! C++17(g++) 틀·하네스. `int`는 `long long`. 하네스가 `solution.cpp`를 `#include`해 한 번역 단위로 한 번 컴파일한다.

use super::{Scalar, Signature, ValueType};

fn ty(t: ValueType) -> String {
    let base = match t.scalar() {
        Scalar::Int => "long long",
        Scalar::Double => "double",
        Scalar::Bool => "bool",
        Scalar::String => "string",
    };
    match t.dims() {
        0 => base.to_string(),
        1 => format!("vector<{base}>"),
        _ => format!("vector<vector<{base}>>"),
    }
}

pub(super) fn template(sig: &Signature) -> String {
    let params = sig
        .params
        .iter()
        .map(|p| format!("{} {}", ty(p.ty), p.name))
        .collect::<Vec<_>>()
        .join(", ");
    let ret = ty(sig.returns);
    let body = match sig.returns.dims() {
        0 => match sig.returns.scalar() {
            Scalar::Int => "0".to_string(),
            Scalar::Double => "0.0".to_string(),
            Scalar::Bool => "false".to_string(),
            Scalar::String => "\"\"".to_string(),
        },
        _ => "{}".to_string(),
    };
    format!(
        "#include <string>\n#include <vector>\n\nusing namespace std;\n\n{ret} {name}({params}) {{\n    {ret} answer = {body};\n    return answer;\n}}\n",
        name = sig.name
    )
}

/// 값 변환(`pl_h::J` → C++ 값).
fn conv_fn(t: ValueType) -> &'static str {
    match t {
        ValueType::Int => "pl_h::as_int",
        ValueType::Double => "pl_h::as_double",
        ValueType::Bool => "pl_h::as_bool",
        ValueType::String => "pl_h::as_str",
        ValueType::IntArray => "pl_h::arr<long long, pl_h::as_int>",
        ValueType::DoubleArray => "pl_h::arr<double, pl_h::as_double>",
        ValueType::BoolArray => "pl_h::arr<bool, pl_h::as_bool>",
        ValueType::StringArray => "pl_h::arr<std::string, pl_h::as_str>",
        ValueType::IntMatrix => {
            "pl_h::arr<std::vector<long long>, pl_h::arr<long long, pl_h::as_int>>"
        }
        ValueType::StringMatrix => {
            "pl_h::arr<std::vector<std::string>, pl_h::arr<std::string, pl_h::as_str>>"
        }
    }
}

/// 하네스 안에서 쓰는 C++ 타입(`std::` 붙임 — 풀이가 `using namespace std`를 안 써도 된다).
fn std_ty(t: ValueType) -> String {
    ty(t)
        .replace("vector", "std::vector")
        .replace("string", "std::string")
}

const HARNESS: &str = r#"#include "solution.cpp"

#include <unistd.h>

#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <exception>
#include <iostream>
#include <iterator>
#include <new>
#include <string>
#include <vector>

namespace pl_h {

struct J {
    enum Kind { Null, Int, Dbl, Bool, Str, Arr } k = Null;
    long long i = 0;
    double d = 0;
    bool b = false;
    std::string s;
    std::vector<J> a;
};

struct Parser {
    const std::string& s;
    size_t p = 0;
    void ws() { while (p < s.size() && (s[p] == ' ' || s[p] == '\n' || s[p] == '\r' || s[p] == '\t')) p++; }
    static void utf8(std::string& o, unsigned cp) {
        if (cp < 0x80) o += char(cp);
        else if (cp < 0x800) { o += char(0xC0 | (cp >> 6)); o += char(0x80 | (cp & 0x3F)); }
        else if (cp < 0x10000) { o += char(0xE0 | (cp >> 12)); o += char(0x80 | ((cp >> 6) & 0x3F)); o += char(0x80 | (cp & 0x3F)); }
        else { o += char(0xF0 | (cp >> 18)); o += char(0x80 | ((cp >> 12) & 0x3F)); o += char(0x80 | ((cp >> 6) & 0x3F)); o += char(0x80 | (cp & 0x3F)); }
    }
    unsigned hex4() { unsigned v = std::stoul(s.substr(p, 4), nullptr, 16); p += 4; return v; }
    J value() {
        ws();
        J v;
        char c = s[p];
        if (c == '[') {
            p++;
            v.k = J::Arr;
            ws();
            if (s[p] == ']') { p++; return v; }
            while (true) {
                v.a.push_back(value());
                ws();
                char d = s[p++];
                if (d == ']') return v;
            }
        }
        if (c == '"') {
            p++;
            v.k = J::Str;
            while (true) {
                char ch = s[p++];
                if (ch == '"') return v;
                if (ch != '\\') { v.s += ch; continue; }
                char e = s[p++];
                switch (e) {
                    case 'n': v.s += '\n'; break;
                    case 't': v.s += '\t'; break;
                    case 'r': v.s += '\r'; break;
                    case 'b': v.s += '\b'; break;
                    case 'f': v.s += '\f'; break;
                    case 'u': {
                        unsigned cp = hex4();
                        if (cp >= 0xD800 && cp < 0xDC00 && s.compare(p, 2, "\\u") == 0) {
                            p += 2;
                            unsigned lo = hex4();
                            cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                        }
                        utf8(v.s, cp);
                        break;
                    }
                    default: v.s += e;
                }
            }
        }
        if (s.compare(p, 4, "true") == 0) { p += 4; v.k = J::Bool; v.b = true; return v; }
        if (s.compare(p, 5, "false") == 0) { p += 5; v.k = J::Bool; return v; }
        if (s.compare(p, 4, "null") == 0) { p += 4; return v; }
        size_t st = p;
        bool dbl = false;
        while (p < s.size() && std::string("+-0123456789.eE").find(s[p]) != std::string::npos) {
            if (s[p] == '.' || s[p] == 'e' || s[p] == 'E') dbl = true;
            p++;
        }
        std::string n = s.substr(st, p - st);
        if (dbl) { v.k = J::Dbl; v.d = std::strtod(n.c_str(), nullptr); }
        else { v.k = J::Int; v.i = std::strtoll(n.c_str(), nullptr, 10); v.d = double(v.i); }
        return v;
    }
};

long long as_int(const J& v) { return v.i; }
double as_double(const J& v) { return v.k == J::Int ? double(v.i) : v.d; }
bool as_bool(const J& v) { return v.b; }
std::string as_str(const J& v) { return v.s; }
template <typename T, T (*F)(const J&)>
std::vector<T> arr(const J& v) {
    std::vector<T> r;
    r.reserve(v.a.size());
    for (const J& x : v.a) r.push_back(F(x));
    return r;
}

void w(std::string& o, long long v) { o += std::to_string(v); }
void w(std::string& o, double v) {
    if (!std::isfinite(v)) { o += "null"; return; }
    char buf[40];
    std::snprintf(buf, sizeof buf, "%.17g", v);
    o += buf;
}
void w(std::string& o, bool v) { o += v ? "true" : "false"; }
void w(std::string& o, const std::string& v) {
    o += '"';
    for (unsigned char c : v) {
        if (c == '"') o += "\\\"";
        else if (c == '\\') o += "\\\\";
        else if (c < 0x20) { char buf[8]; std::snprintf(buf, sizeof buf, "\\u%04x", c); o += buf; }
        else o += char(c);
    }
    o += '"';
}
template <typename T>
void w(std::string& o, const std::vector<T>& v) {
    o += '[';
    bool first = true;
    for (const auto& x : v) {
        if (!first) o += ',';
        first = false;
        w(o, static_cast<T>(x));
    }
    o += ']';
}

void emit(std::string line) {
    line += '\n';
    size_t off = 0;
    while (off < line.size()) {
        ssize_t n = ::write(3, line.data() + off, line.size() - off);
        if (n <= 0) std::_Exit(3);
        off += size_t(n);
    }
}

void runtime(const std::string& msg) {
    std::string o = "{\"error\":\"runtime\",\"message\":";
    w(o, msg.substr(0, 1000));
    emit(o + "}");
}

}  // namespace pl_h

int main() {
    std::string input((std::istreambuf_iterator<char>(std::cin)), std::istreambuf_iterator<char>());
    pl_h::Parser parser{input};
    pl_h::J cases = parser.value();
    pl_h::emit("{\"ready\":true}");
    for (const pl_h::J& c : cases.a) {
        try {
__CONVERT__
            __RET__ r = __NAME__(__ARGS__);
            std::string o = "{\"ok\":";
            pl_h::w(o, r);
            pl_h::emit(o + "}");
        } catch (const std::bad_alloc&) {
            pl_h::emit("{\"error\":\"memory\"}");
        } catch (const std::exception& e) {
            pl_h::runtime(std::string("exception: ") + e.what());
        } catch (...) {
            pl_h::runtime("알 수 없는 예외");
        }
    }
    std::cout.flush();
    std::fflush(stdout);
    std::_Exit(0);
}
"#;

pub(super) fn harness(sig: &Signature) -> String {
    let convert = sig
        .params
        .iter()
        .enumerate()
        .map(|(k, p)| {
            format!(
                "            {} pl_a{k} = {}(c.a[{k}]);",
                std_ty(p.ty),
                conv_fn(p.ty)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let args = (0..sig.params.len())
        .map(|k| format!("pl_a{k}"))
        .collect::<Vec<_>>()
        .join(", ");
    HARNESS
        .replace("__CONVERT__", &convert)
        .replace("__RET__", &std_ty(sig.returns))
        .replace("__NAME__", &sig.name)
        .replace("__ARGS__", &args)
}
