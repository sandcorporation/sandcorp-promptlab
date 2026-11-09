//! 발행 전 점검의 4개 언어 틀 컴파일(#96)을 **실제 docker**로 확인한다(`#[ignore]` — CI 이미지에는 docker가 없다).
//!
//! ```text
//! bash scripts/build-runners.sh --local && cargo test -p promptlab-worker --test templates_docker -- --ignored
//! ```
//!
//! 내장 12문항 서명과 타입 10개를 모두 쓰는 서명에서, 서명이 만든 빈 틀이 Python·JavaScript·Java·C++ 모두에서
//! 컴파일(문법 오류 없음)되는지 `check_templates`가 `true`로 돌려주는지 본다.

use promptlab_judge::{Judge, RunOutcome, RunRequest};
use promptlab_shared::coding::{Language, Param, Signature, ValueType};
use promptlab_worker::{JudgeCallError, JudgeClient, check_templates};

struct Docker(Judge);

impl JudgeClient for Docker {
    async fn run(&self, req: &RunRequest) -> Result<RunOutcome, JudgeCallError> {
        self.0
            .run(req)
            .await
            .map_err(|e| JudgeCallError::Unavailable(e.to_string()))
    }
}

fn problem(signature: Signature) -> promptlab_shared::coding::CodingProblem {
    let mut p = promptlab_store::catalog::all().next().unwrap().clone();
    p.signature = signature;
    p
}

#[tokio::test]
#[ignore = "docker와 러너 이미지가 필요하다"]
async fn 내장_문항과_모든_타입의_틀이_4개_언어에서_컴파일된다() {
    let judge = Docker(Judge::default());
    for p in promptlab_store::catalog::all() {
        let t = check_templates(&judge, p).await.unwrap();
        assert_eq!(t.len(), 4, "{}", p.id);
        assert!(t.iter().all(|c| c.compiled), "{}: {t:?}", p.id);
    }
    let all_types = Signature {
        name: "everything".into(),
        params: ValueType::ALL
            .iter()
            .enumerate()
            .map(|(i, t)| Param {
                name: format!("p{i}"),
                ty: *t,
            })
            .take(8)
            .collect(),
        returns: ValueType::StringMatrix,
    };
    let t = check_templates(&judge, &problem(all_types)).await.unwrap();
    assert_eq!(
        t.iter().map(|c| c.language).collect::<Vec<_>>(),
        Language::ALL.to_vec()
    );
    assert!(t.iter().all(|c| c.compiled), "{t:?}");
}
