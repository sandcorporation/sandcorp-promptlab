//! 「스모크」 조직(#28). 운영 끝에서 끝 스모크(Windows VM, 기계 토큰)가 응시·제출하는 시험 전용 조직이다.
//!
//! - 조직은 하나(`orgs.is_smoke`, 유일 색인)이고 구성원이 없다. 구성원 조회([`crate::accounts::org_role`]·
//!   [`crate::accounts::orgs_of`])가 이 조직을 보지 않아 대시보드 목록·조직 화면·결과 집계에서 빠진다.
//!   집계를 새로 만들 때도 `NOT orgs.is_smoke`로 거른다.
//! - 준비는 관리 명령 없이 멱등으로: 기계 주체가 배정 목록을 부를 때 [`prepare`]가 조직·시험(없으면)과
//!   「아직 시작하지 않은 배정」(없으면)을 만든다. 문항마다 제출은 한 번이라 스모크 한 번에 새 배정 하나를 쓴다.
//!   조직이 대시보드에 보이지 않으니 사람이 만들 자리가 없고, 운영 준비는 env 키 하나로 끝난다.

use sqlx::PgPool;
use uuid::Uuid;

/// 스모크 조직 이름.
pub const ORG_NAME: &str = "스모크";
/// 스모크 시험 제목.
pub const EXAM_TITLE: &str = "스모크";
/// 스모크 시험의 문항 — 스모크 전용 공개 문항(`fixtures/smoke/`, #138). 운영 문제 디렉터리와 상관없이 늘 실리고
/// 연습·시험 문제은행에는 나오지 않는다. Windows 스모크(`scripts/windows/common.ps1`)가 이 파일의 모범 답안으로 응시한다.
pub const PROBLEM_ID: &str = crate::catalog::SMOKE_PROBLEM_ID;
/// 문항 제한: 30분.
pub const TIME_LIMIT_SECS: i32 = 30 * 60;

/// 그 기계 주체(`sub`=`client:…`)에게 스모크 조직의 시험마다 「아직 시작하지 않은 배정」이 하나씩 있게 한다.
/// 조직·시험이 없으면 먼저 만든다. 여러 요청이 겹쳐도 한 번만 만든다(트랜잭션 advisory lock).
pub async fn prepare(pool: &PgPool, machine_sub: &str, now: i64) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('promptlab-smoke'))")
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO users (sub) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(machine_sub)
        .execute(&mut *tx)
        .await?;
    let org_id: Uuid =
        match sqlx::query_scalar("SELECT id FROM orgs WHERE is_smoke")
            .fetch_optional(&mut *tx)
            .await?
        {
            Some(id) => id,
            None => sqlx::query_scalar(
                "INSERT INTO orgs (name, created_by, is_smoke) VALUES ($1, $2, true) RETURNING id",
            )
            .bind(ORG_NAME)
            .bind(machine_sub)
            .fetch_one(&mut *tx)
            .await?,
        };
    // 지금 스모크 문항([`PROBLEM_ID`])을 쓰는 시험만 쓴다. 문항이 바뀌었으면(#138: 옛 운영 문항 → 스모크 전용 공개 문항)
    // 옛 시험에는 새 배정을 만들지 않고, 이 기계 주체의 쓰지 않은 배정(시작·제출 없음)은 지운다 — 배정 목록이 수락 순이라
    // 남겨 두면 스모크가 옛 문항을 고른다. 옛 시험과 끝난 배정(결과)은 그대로 남는다.
    sqlx::query(
        "DELETE FROM exam_invites i
          USING exams e
          WHERE i.exam_id = e.id AND e.org_id = $1 AND i.candidate_sub = $2
            AND NOT EXISTS (SELECT 1 FROM exam_items it WHERE it.exam_id = e.id AND it.problem_id = $3)
            AND NOT EXISTS (SELECT 1 FROM exam_item_starts s WHERE s.invite_id = i.id)
            AND NOT EXISTS (SELECT 1 FROM submissions s WHERE s.invite_id = i.id)",
    )
    .bind(org_id)
    .bind(machine_sub)
    .bind(PROBLEM_ID)
    .execute(&mut *tx)
    .await?;
    let mut exam_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT e.id FROM exams e
          WHERE e.org_id = $1
            AND EXISTS (SELECT 1 FROM exam_items it WHERE it.exam_id = e.id AND it.problem_id = $2)",
    )
    .bind(org_id)
    .bind(PROBLEM_ID)
    .fetch_all(&mut *tx)
    .await?;
    if exam_ids.is_empty() {
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO exams (org_id, title, purpose, reveal_scores, created_by, created_at)
             VALUES ($1, $2, 'internal', true, $3, to_timestamp($4)) RETURNING id",
        )
        .bind(org_id)
        .bind(EXAM_TITLE)
        .bind(machine_sub)
        .bind(now as f64)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO exam_items (exam_id, position, problem_id, time_limit_secs)
             VALUES ($1, 0, $2, $3)",
        )
        .bind(id)
        .bind(PROBLEM_ID)
        .bind(TIME_LIMIT_SECS)
        .execute(&mut *tx)
        .await?;
        exam_ids.push(id);
    }
    for exam_id in exam_ids {
        let fresh = sqlx::query(
            "SELECT 1 FROM exam_invites i
              WHERE i.exam_id = $1 AND i.candidate_sub = $2
                AND NOT EXISTS (SELECT 1 FROM exam_item_starts s WHERE s.invite_id = i.id)
                AND NOT EXISTS (SELECT 1 FROM submissions s WHERE s.invite_id = i.id)
              LIMIT 1",
        )
        .bind(exam_id)
        .bind(machine_sub)
        .fetch_optional(&mut *tx)
        .await?;
        if fresh.is_some() {
            continue;
        }
        // 링크로 받는 초대가 아니다: 토큰 해시는 아무도 모르는 값, 메일은 보내지 않는다.
        sqlx::query(
            "INSERT INTO exam_invites
                (exam_id, email, token_hash, created_by, created_at, expires_at,
                 candidate_sub, accepted_at, delivery)
             VALUES ($1, 'smoke-' || gen_random_uuid() || '@smoke.invalid', uuid_send(gen_random_uuid()),
                     $2, to_timestamp($3), to_timestamp($3), $2, to_timestamp($3), 'not_configured')",
        )
        .bind(exam_id)
        .bind(machine_sub)
        .bind(now as f64)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}
