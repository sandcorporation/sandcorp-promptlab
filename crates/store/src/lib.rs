//! 서버 쪽 저장소. 서버(API)와 채점 워커가 함께 쓴다. **응시 앱은 이 크레이트에 기대지 않는다**
//! (히든 테스트이 든 문제 목록이 여기 있다 — `crates/app/tests/no_hidden_inputs.rs`가 의존을 건다).
//!
//! - [`catalog`]: 문제은행 문항(히든 테스트 포함) — 실행할 때 `PROMPTLAB_PROBLEMS_DIR`(없으면 공개 픽스처)에서 싣는다.
//! - [`problems`]: 문제 찾기(내장 + 조직 문제), 조직 문제 저장, 풀림 미리 돌리기 큐(#16).
//! - [`accounts`]: 계정·조직·역할·초대와 대시보드 로그인 상태·세션(#7).
//! - [`exams`]: 시험(문항 묶음)·응시 배정 초대·문항 시작 시각(#8).
//! - [`smoke`]: 「스모크」 조직·시험·기계 주체의 배정을 멱등으로 준비한다(#28).
//! - [`integrity`]: 허용 목록 거부 기록(#12).
//! - [`logs`]: 과정 로그 — 문항별 설치 키, 검증해 받은 사건(서명 원문 그대로), 거부 기록(#9).
//! - [`reports`]: AI 리포트 큐 — 응시마다 한 줄, 자동 생성·요청·조직당 동시 상한(#15).
//! - [`usage`]: 토큰·비용 기록 — 호출마다 한 줄, 조직·시험·응시 집계(스모크 제외, #15).
//! - [`retention`]: 데이터 보유·자동 삭제(#54) — 보관 기간 설정·이력, 기간이 지난 응시·연습 찾기, 삭제 서비스 [`retention::erase`].
//! - [`erasure`]: 고객사 관리자의 응시자 데이터 삭제(#55) — 배정을 잠그고 [`retention::erase_in`]과 감사 기록을 한 트랜잭션으로.
//! - [`submissions`]: 제출 저장과 채점 큐(Postgres 테이블 큐, `FOR UPDATE SKIP LOCKED`).
//! - [`MIGRATOR`]: `migrations/`를 바이너리에 구운 마이그레이션. 서버·워커가 시작할 때 돌린다
//!   (sqlx가 advisory lock으로 한 번만 적용한다). 시험은 `#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]`.

pub mod accounts;
pub mod bank;
pub mod catalog;
pub mod checks;
pub mod erasure;
pub mod evaluations;
pub mod exams;
pub mod integrity;
pub mod live;
pub mod logs;
pub mod operator;
pub mod problems;
pub mod reports;
pub mod results;
pub mod retention;
pub mod smoke;
pub mod submissions;
pub mod usage;
pub mod webcam;

pub use sqlx::PgPool;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// `DATABASE_URL`로 연결하고 마이그레이션을 돌린다.
pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .connect(database_url)
        .await?;
    MIGRATOR.run(&pool).await?;
    Ok(pool)
}
