//! 문제은행 싣기 불변식(#138): 문항은 실행할 때 디렉터리(`PROMPTLAB_PROBLEMS_DIR`)에서, 없으면 구운 공개 픽스처에서
//! 싣는다. 하나라도 깨지면 싣지 않는다(서버·워커가 뜨지 않는다). 스모크 문항은 늘 실리고 문제은행에 없다.
//! 모범 답안이 예제·히든을 모두 통과하는지는 docker가 있어야 해서 `crates/judge/tests/bank.rs`가 건다.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use promptlab_shared::coding::Language;
use promptlab_store::catalog::{self, Catalog, Kind, SMOKE_PROBLEM_ID, Source};

fn fixtures_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/problems"))
}

/// 시험마다 새 임시 디렉터리에 픽스처를 복사한다.
fn copy_fixtures(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "promptlab-catalog-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    for entry in std::fs::read_dir(fixtures_dir()).unwrap() {
        let path = entry.unwrap().path();
        std::fs::copy(&path, dir.join(path.file_name().unwrap())).unwrap();
    }
    dir
}

#[test]
fn 환경이_없으면_공개_픽스처를_싣고_모두_규칙을_지킨다() {
    // 시험 프로세스에는 PROMPTLAB_PROBLEMS_DIR가 없다(CI) — 전역 목록 = 픽스처 + 스모크.
    if std::env::var_os(catalog::PROBLEMS_DIR_ENV).is_some() {
        return;
    }
    let all: Vec<_> = catalog::all().collect();
    let ids: BTreeSet<&str> = all.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids.len(), all.len(), "id가 겹친다");
    for p in &all {
        p.validate()
            .unwrap_or_else(|e| panic!("{}: 규칙 위반 {e}", p.id));
        assert_eq!(p.reference_language, Language::Python, "{}", p.id);
        // 공개 보기에는 히든·모범 답안이 없다.
        let public = serde_json::to_string(&p.public_view()).unwrap();
        assert!(!public.contains("hidden"), "{}", p.id);
        assert!(!public.contains(p.reference_solution.trim()), "{}", p.id);
    }
    let fixtures = Catalog::fixtures().unwrap();
    assert_eq!(fixtures.source(), &Source::Fixtures);
    assert_eq!(fixtures.count(Kind::Smoke), 1);
    assert_eq!(
        catalog::practice().count() + catalog::exam_bank().count() + 1,
        all.len()
    );
}

#[test]
fn 구운_픽스처는_저장소의_픽스처_디렉터리와_같다() {
    // FIXTURES 표에 파일을 빠뜨리면 디렉터리 싣기(목록에 없는 파일·없는 파일 오류)와 갈린다.
    let from_dir = Catalog::load_dir(fixtures_dir()).unwrap();
    let baked = Catalog::fixtures().unwrap();
    for kind in [Kind::Practice, Kind::Exam, Kind::Smoke] {
        assert_eq!(from_dir.count(kind), baked.count(kind), "{kind:?}");
    }
}

#[test]
fn 연습과_시험은_겹치지_않고_스모크는_문제은행에_없다() {
    let practice: BTreeSet<&str> = catalog::practice().map(|p| p.id.as_str()).collect();
    assert!(!practice.is_empty());
    assert!(catalog::exam_bank().next().is_some());
    assert!(catalog::exam_bank().all(|p| !practice.contains(p.id.as_str())));
    assert!(catalog::practice().all(|p| catalog::is_practice(&p.id)));
    assert!(catalog::bank().all(|p| catalog::is_published(&p.id)));
    assert!(catalog::is_practice(catalog::default_practice().unwrap()));

    assert_eq!(promptlab_store::smoke::PROBLEM_ID, SMOKE_PROBLEM_ID);
    assert!(catalog::find(SMOKE_PROBLEM_ID).is_some());
    assert_eq!(catalog::kind(SMOKE_PROBLEM_ID), Some(Kind::Smoke));
    assert!(!catalog::is_practice(SMOKE_PROBLEM_ID));
    assert!(!catalog::is_published(SMOKE_PROBLEM_ID));
    assert!(catalog::bank().all(|p| p.id != SMOKE_PROBLEM_ID));
    assert!(catalog::exam_bank().all(|p| p.id != SMOKE_PROBLEM_ID));
}

#[test]
fn 디렉터리를_싣고_스모크_문항을_더한다() {
    let dir = copy_fixtures("ok");
    let c = Catalog::load_dir(&dir).unwrap();
    assert_eq!(c.source(), &Source::Dir(dir));
    assert!(c.count(Kind::Practice) > 0 && c.count(Kind::Exam) > 0);
    assert_eq!(c.count(Kind::Smoke), 1);
}

fn load_err(dir: &Path) -> String {
    Catalog::load_dir(dir).unwrap_err().to_string()
}

#[test]
fn 목록에_없는_파일이나_없는_파일이_있으면_싣지_않는다() {
    let dir = copy_fixtures("extra");
    std::fs::copy(dir.join("array-sum.json"), dir.join("stray.json")).unwrap();
    assert!(load_err(&dir).contains("stray.json"), "{}", load_err(&dir));

    let dir = copy_fixtures("missing");
    std::fs::remove_file(dir.join("array-sum.json")).unwrap();
    assert!(load_err(&dir).contains("array-sum.json"));

    let dir = copy_fixtures("noindex");
    std::fs::remove_file(dir.join("index.json")).unwrap();
    assert!(load_err(&dir).contains("index.json"));

    assert!(Catalog::load_dir(Path::new("/없는/디렉터리")).is_err());
}

#[test]
fn 깨진_문항이_하나라도_있으면_싣지_않는다() {
    let rewrite = |dir: &Path, file: &str, f: &dyn Fn(&mut serde_json::Value)| {
        let path = dir.join(file);
        let mut v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        f(&mut v);
        std::fs::write(&path, v.to_string()).unwrap();
    };
    // 규칙 위반(히든 5개 미만).
    let dir = copy_fixtures("invalid");
    rewrite(&dir, "array-sum.json", &|v| {
        v["hidden"].as_array_mut().unwrap().truncate(1);
    });
    assert!(load_err(&dir).contains("array-sum"), "{}", load_err(&dir));
    // 파일 이름 ≠ id.
    let dir = copy_fixtures("id");
    rewrite(&dir, "array-sum.json", &|v| v["id"] = "other".into());
    assert!(load_err(&dir).contains("other"));
    // JSON이 깨짐.
    let dir = copy_fixtures("json");
    std::fs::write(dir.join("array-sum.json"), "{").unwrap();
    assert!(load_err(&dir).contains("array-sum"));
    // 목록에 두 번.
    let dir = copy_fixtures("dup");
    rewrite(&dir, "index.json", &|v| {
        v["exam"].as_array_mut().unwrap().push("array-sum".into());
    });
    assert!(load_err(&dir).contains("두 번"));
    // 스모크 id를 문제은행에 쓰면 안 된다.
    let dir = copy_fixtures("smoke");
    std::fs::copy(
        dir.join("array-sum.json"),
        dir.join(format!("{SMOKE_PROBLEM_ID}.json")),
    )
    .unwrap();
    rewrite(&dir, &format!("{SMOKE_PROBLEM_ID}.json"), &|v| {
        v["id"] = SMOKE_PROBLEM_ID.into();
    });
    rewrite(&dir, "index.json", &|v| {
        v["exam"]
            .as_array_mut()
            .unwrap()
            .push(SMOKE_PROBLEM_ID.into());
    });
    assert!(load_err(&dir).contains("스모크"), "{}", load_err(&dir));
}

#[test]
fn 운영_문항_디렉터리가_주어지면_모두_실린다() {
    // 데이터 저장소를 받아 둔 곳에서: PROMPTLAB_PROBLEMS_DIR=<디렉터리> cargo test -p promptlab-store --test bank
    let Some(dir) = std::env::var_os(catalog::PROBLEMS_DIR_ENV) else {
        return;
    };
    let c = Catalog::load_dir(Path::new(&dir)).unwrap_or_else(|e| panic!("{e}"));
    eprintln!(
        "연습 {} · 시험 {}",
        c.count(Kind::Practice),
        c.count(Kind::Exam)
    );
}
