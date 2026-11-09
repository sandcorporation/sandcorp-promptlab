-- 대화 평가(#72, spec #69 결정 4·10·11): 채점이 끝난 대화형 제출 하나에 Claude 기준표 평가 한 줄.
-- 서버의 평가 큐가 넣고(스모크 조직 제외, 연습은 하루 한도 넘으면 daily_limit) 하나씩 꺼내 평가한다.
-- 점수(submissions.report)는 건드리지 않는다 — 최종 점수는 응답을 만들 때 shared::evaluation이 계산한다.
CREATE TABLE submission_evaluations (
    submission_id  uuid        PRIMARY KEY REFERENCES submissions (id) ON DELETE CASCADE,
    -- 시험 응시의 조직(조직당 동시 한도). 개인 연습은 NULL.
    org_id         uuid        REFERENCES orgs (id) ON DELETE CASCADE,
    candidate_sub  text        NOT NULL,
    practice       boolean     NOT NULL,
    status         text        NOT NULL DEFAULT 'queued'
                               CHECK (status IN ('queued', 'evaluating', 'done', 'failed', 'daily_limit')),
    -- shared::evaluation::Evaluation(항목·점수·근거 턴). done일 때.
    body           jsonb,
    model          text,
    -- 짧은 실패 까닭(키·요청 본문 없음).
    failure_reason text,
    attempts       integer     NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    requested_at   timestamptz NOT NULL,
    started_at     timestamptz,
    finished_at    timestamptz
);
CREATE INDEX submission_evaluations_queued_idx ON submission_evaluations (requested_at) WHERE status = 'queued';
CREATE INDEX submission_evaluations_practice_idx ON submission_evaluations (candidate_sub, requested_at) WHERE practice;

-- 비용 기록에 종류 「대화 평가」를 더한다.
ALTER TABLE usage_records DROP CONSTRAINT usage_records_kind_check;
ALTER TABLE usage_records ADD CONSTRAINT usage_records_kind_check
    CHECK (kind IN ('grading', 'report', 'preview', 'recheck', 'evaluation'));
