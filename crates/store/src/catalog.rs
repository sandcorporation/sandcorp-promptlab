//! 문제은행의 저장소 밖 문항(#138). 히든 테스트·모범 답안은 바이너리·이미지에 굽지 않고 **실행할 때 디렉터리에서
//! 읽는다** — 공개 저장소·GHCR 이미지에 숨은 입력이 실리지 않게.
//!
//! - `PROMPTLAB_PROBLEMS_DIR`([`PROBLEMS_DIR_ENV`])가 있으면 그 디렉터리(운영: 비공개 데이터 저장소를 compose가
//!   ro로 마운트)를, 없으면 저장소에 구운 **공개 픽스처**(`crates/store/fixtures/problems/` — 개발 스택·CI·시험용,
//!   운영 문제은행과 다른 문항)를 싣는다. 둘은 섞지 않는다.
//! - 디렉터리 꼴: `index.json` = `{"practice": [id…], "exam": [id…]}`(목록 순서 = 화면 순서) + 문항마다 `<id>.json`
//!   (`CodingProblem`). 목록에 없는 `*.json`이 있거나, 파일 이름 ≠ id, id가 겹치거나, `validate()`가 깨지면
//!   싣지 않는다 — 서버·워커는 시작할 때 [`init`]으로 전부 확인하고 하나라도 깨지면 이유를 찍고 뜨지 않는다.
//! - **스모크 문항**([`SMOKE_PROBLEM_ID`], `fixtures/smoke/`)은 공개 문항이라 늘 구워 싣는다. 스모크 조직 시험만 쓰고
//!   연습·시험 문제은행·운영자 목록에는 나오지 않는다(발행 아님).
//! - 연습/시험 나눔(#18): 연습 문항은 연습에만, 시험 문항은 시험에만 — 시험 문항을 연습에 열면 같은 히든 테스트에
//!   마음껏 제출해 통과 수를 볼 수 있어 유출 통로가 된다.
//!
//! 밖으로는 [`CodingProblem::public_view`]만 내보낸다.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use promptlab_shared::coding::CodingProblem;
use serde_json::Value;

/// 문제 디렉터리를 주는 환경변수.
pub const PROBLEMS_DIR_ENV: &str = "PROMPTLAB_PROBLEMS_DIR";
/// 디렉터리의 목록 파일 이름.
pub const INDEX_FILE: &str = "index.json";
/// 스모크 전용 공개 문항 id(`fixtures/smoke/<id>.json`, Windows 스모크가 이 파일의 모범 답안으로 응시한다).
pub const SMOKE_PROBLEM_ID: &str = "smoke-running-sum";

const SMOKE_SOURCE: &str = include_str!("../fixtures/smoke/smoke-running-sum.json");

/// 공개 픽스처(`fixtures/problems/`) — 공개돼도 되는 문항이라 구워 둔다. 파일을 더하면 여기와 `index.json`을 함께.
const FIXTURES: &[(&str, &str)] = &[
    (INDEX_FILE, include_str!("../fixtures/problems/index.json")),
    (
        "array-sum.json",
        include_str!("../fixtures/problems/array-sum.json"),
    ),
    (
        "reverse-words.json",
        include_str!("../fixtures/problems/reverse-words.json"),
    ),
    (
        "count-vowels.json",
        include_str!("../fixtures/problems/count-vowels.json"),
    ),
    (
        "second-largest.json",
        include_str!("../fixtures/problems/second-largest.json"),
    ),
];

/// 문항의 쓰임.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// 개인 연습에만.
    Practice,
    /// 시험에만.
    Exam,
    /// 스모크 조직 시험에만(문제은행에 안 나온다).
    Smoke,
}

/// 문항을 어디서 실었나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// `PROMPTLAB_PROBLEMS_DIR`의 디렉터리.
    Dir(PathBuf),
    /// 구운 공개 픽스처.
    Fixtures,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Dir(dir) => write!(f, "{}", dir.display()),
            Source::Fixtures => f.write_str("공개 픽스처"),
        }
    }
}

/// 싣기 실패 — 무엇이 어디서 깨졌는지 사람 말로.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogError(pub String);

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CatalogError {}

/// 실은 문항 전부(스모크 문항 포함).
#[derive(Debug)]
pub struct Catalog {
    problems: Vec<(CodingProblem, Kind)>,
    source: Source,
}

impl Catalog {
    /// 디렉터리에서 싣는다(목록에 없는 `*.json`도 오류 — 빠뜨린 문항을 조용히 버리지 않는다).
    pub fn load_dir(dir: &Path) -> Result<Self, CatalogError> {
        let read = |name: &str| {
            std::fs::read_to_string(dir.join(name))
                .map_err(|e| CatalogError(format!("{}: {e}", dir.join(name).display())))
        };
        let mut files = BTreeSet::new();
        let entries = std::fs::read_dir(dir)
            .map_err(|e| CatalogError(format!("문제 디렉터리 {}: {e}", dir.display())))?;
        for entry in entries {
            let entry = entry.map_err(|e| CatalogError(format!("{}: {e}", dir.display())))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".json") && name != INDEX_FILE {
                files.insert(name);
            }
        }
        let catalog = Self::build(Source::Dir(dir.to_owned()), &read(INDEX_FILE)?, |id| {
            read(&format!("{id}.json"))
        })?;
        let listed: BTreeSet<String> = catalog
            .problems
            .iter()
            .filter(|(_, kind)| *kind != Kind::Smoke)
            .map(|(p, _)| format!("{}.json", p.id))
            .collect();
        if let Some(extra) = files.difference(&listed).next() {
            return Err(CatalogError(format!(
                "{}: {extra} 이 {INDEX_FILE} 목록에 없다",
                dir.display()
            )));
        }
        Ok(catalog)
    }

    /// 구운 공개 픽스처를 싣는다.
    pub fn fixtures() -> Result<Self, CatalogError> {
        let read = |name: &str| {
            FIXTURES
                .iter()
                .find(|(file, _)| *file == name)
                .map(|(_, text)| (*text).to_owned())
                .ok_or_else(|| CatalogError(format!("픽스처 {name} 이 구워지지 않았다")))
        };
        Self::build(Source::Fixtures, &read(INDEX_FILE)?, |id| {
            read(&format!("{id}.json"))
        })
    }

    /// 환경변수대로 싣는다: `PROMPTLAB_PROBLEMS_DIR`가 있으면 디렉터리, 없으면 공개 픽스처.
    pub fn from_env() -> Result<Self, CatalogError> {
        match std::env::var_os(PROBLEMS_DIR_ENV).filter(|v| !v.is_empty()) {
            Some(dir) => Self::load_dir(Path::new(&dir)),
            None => Self::fixtures(),
        }
    }

    fn build(
        source: Source,
        index: &str,
        read: impl Fn(&str) -> Result<String, CatalogError>,
    ) -> Result<Self, CatalogError> {
        let index: Value = serde_json::from_str(index)
            .map_err(|e| CatalogError(format!("{source}: {INDEX_FILE} 이 깨졌다: {e}")))?;
        let ids = |key: &str| -> Result<Vec<String>, CatalogError> {
            index
                .get(key)
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    CatalogError(format!("{source}: {INDEX_FILE} 에 `{key}` 배열이 없다"))
                })?
                .iter()
                .map(|v| {
                    v.as_str().map(str::to_owned).ok_or_else(|| {
                        CatalogError(format!("{source}: {INDEX_FILE} `{key}` 에 글이 아닌 값"))
                    })
                })
                .collect()
        };
        let mut wanted: Vec<(String, Kind)> = Vec::new();
        wanted.extend(ids("practice")?.into_iter().map(|id| (id, Kind::Practice)));
        wanted.extend(ids("exam")?.into_iter().map(|id| (id, Kind::Exam)));

        let parse = |id: &str, text: &str| -> Result<CodingProblem, CatalogError> {
            let problem: CodingProblem = serde_json::from_str(text)
                .map_err(|e| CatalogError(format!("{source}: {id}.json 이 깨졌다: {e}")))?;
            if problem.id != id {
                return Err(CatalogError(format!(
                    "{source}: {id}.json 의 id가 {} 다(파일 이름 = id)",
                    problem.id
                )));
            }
            problem
                .validate()
                .map_err(|e| CatalogError(format!("{source}: {id} 규칙 위반: {e}")))?;
            Ok(problem)
        };
        let mut problems = Vec::with_capacity(wanted.len() + 1);
        let mut seen = BTreeSet::new();
        for (id, kind) in wanted {
            if !seen.insert(id.clone()) {
                return Err(CatalogError(format!("{source}: {id} 가 목록에 두 번 있다")));
            }
            problems.push((parse(&id, &read(&id)?)?, kind));
        }
        if !seen.insert(SMOKE_PROBLEM_ID.to_owned()) {
            return Err(CatalogError(format!(
                "{source}: {SMOKE_PROBLEM_ID} 는 스모크 전용 id다"
            )));
        }
        problems.push((
            parse(SMOKE_PROBLEM_ID, SMOKE_SOURCE)
                .map_err(|e| CatalogError(format!("스모크 문항: {e}")))?,
            Kind::Smoke,
        ));
        Ok(Self { problems, source })
    }

    /// 어디서 실었나.
    pub fn source(&self) -> &Source {
        &self.source
    }

    /// 쓰임별 문항 수.
    pub fn count(&self, kind: Kind) -> usize {
        self.problems.iter().filter(|(_, k)| *k == kind).count()
    }
}

static CATALOG: OnceLock<Catalog> = OnceLock::new();

/// 서버·워커 시작 때 부른다: 환경대로 싣고 전부 확인한다. 실패면 띄우지 말 것.
pub fn init() -> Result<&'static Catalog, CatalogError> {
    if let Some(catalog) = CATALOG.get() {
        return Ok(catalog);
    }
    let loaded = Catalog::from_env()?;
    Ok(CATALOG.get_or_init(|| loaded))
}

fn get() -> &'static Catalog {
    // 시험·도구는 init 없이 쓴다 — 환경이 없으면 공개 픽스처. 운영은 시작 때 init이 먼저 확인했다.
    CATALOG
        .get_or_init(|| Catalog::from_env().unwrap_or_else(|e| panic!("문제를 싣지 못했다: {e}")))
}

/// 실은 문항 전부(연습·시험·스모크).
pub fn all() -> impl Iterator<Item = &'static CodingProblem> {
    get().problems.iter().map(|(p, _)| p)
}

/// 문제은행 문항(연습·시험 — 스모크 문항 뺌). 운영자 목록이 쓴다.
pub fn bank() -> impl Iterator<Item = &'static CodingProblem> {
    get()
        .problems
        .iter()
        .filter(|(_, k)| *k != Kind::Smoke)
        .map(|(p, _)| p)
}

/// id로 문제 하나(스모크 문항 포함 — 채점·문항 시작이 쓴다).
pub fn find(id: &str) -> Option<&'static CodingProblem> {
    all().find(|p| p.id == id)
}

/// 그 문항의 쓰임.
pub fn kind(id: &str) -> Option<Kind> {
    get()
        .problems
        .iter()
        .find(|(p, _)| p.id == id)
        .map(|(_, k)| *k)
}

/// 연습 문제인가.
pub fn is_practice(id: &str) -> bool {
    kind(id) == Some(Kind::Practice)
}

/// 연습 문제들(`index.json` 순서).
pub fn practice() -> impl Iterator<Item = &'static CodingProblem> {
    get()
        .problems
        .iter()
        .filter(|(_, k)| *k == Kind::Practice)
        .map(|(p, _)| p)
}

/// 시험에 쓰는 문제들(`index.json` 순서).
pub fn exam_bank() -> impl Iterator<Item = &'static CodingProblem> {
    get()
        .problems
        .iter()
        .filter(|(_, k)| *k == Kind::Exam)
        .map(|(p, _)| p)
}

/// 앱이 처음 보여 줄 연습 문제 = 첫 연습 문항(없으면 `None`).
pub fn default_practice() -> Option<&'static str> {
    practice().next().map(|p| p.id.as_str())
}

/// 발행한 문항인가 — 연습·시험 문항은 있으면 발행이다(데이터 쪽 docker 시험이 점검한다). 스모크 문항은 문제은행
/// 문항이 아니라 발행이 아니다(새 시험·연습에 못 쓴다).
pub fn is_published(id: &str) -> bool {
    matches!(kind(id), Some(Kind::Practice | Kind::Exam))
}
