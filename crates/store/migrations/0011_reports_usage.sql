-- AI 리포트·의심 요약(#15)과 토큰·비용 기록.

-- 응시(배정) 하나의 AI 리포트. 모든 문항의 채점이 끝나면 서버가 자동으로 queued를 넣고(스모크 조직 제외),
-- 결과 보기 역할이 다시 만들기를 요청할 수 있다(generations < 상한). 점수(submissions.report)는 건드리지 않는다.
CREATE TABLE exam_reports (
    invite_id      uuid        PRIMARY KEY REFERENCES exam_invites (id) ON DELETE CASCADE,
    org_id         uuid        NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    status         text        NOT NULL DEFAULT 'queued'
                               CHECK (status IN ('queued', 'generating', 'done', 'failed')),
    -- 모델이 쓴 리포트(shared::report::ReportBody). 마지막으로 성공한 것.
    body           jsonb,
    model          text,
    -- 담당자에게 보이는 짧은 실패 까닭(키·요청 본문 없음).
    failure_reason text,
    generations    integer     NOT NULL DEFAULT 0 CHECK (generations >= 0),
    requested_at   timestamptz NOT NULL,
    started_at     timestamptz,
    finished_at    timestamptz
);
CREATE INDEX exam_reports_queued_idx ON exam_reports (requested_at) WHERE status = 'queued';
CREATE INDEX exam_reports_generating_idx ON exam_reports (org_id) WHERE status = 'generating';

-- 모델 호출 하나에 한 줄. 조직·시험·응시로 묶어 본다(연습 제출처럼 조직이 없는 것은 NULL).
-- 단가는 기록 시점 값을 함께 남긴다(USD / 백만 토큰) — 단가 표가 바뀌어도 과거 비용은 그대로다.
CREATE TABLE usage_records (
    id                 bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    kind               text        NOT NULL CHECK (kind IN ('grading', 'report', 'preview', 'recheck')),
    org_id             uuid        REFERENCES orgs (id) ON DELETE CASCADE,
    exam_id            uuid        REFERENCES exams (id) ON DELETE SET NULL,
    invite_id          uuid        REFERENCES exam_invites (id) ON DELETE SET NULL,
    submission_id      uuid        REFERENCES submissions (id) ON DELETE SET NULL,
    preview_id         uuid        REFERENCES problem_previews (id) ON DELETE SET NULL,
    model              text        NOT NULL,
    input_tokens       bigint      NOT NULL CHECK (input_tokens >= 0),
    output_tokens      bigint      NOT NULL CHECK (output_tokens >= 0),
    cache_read_tokens  bigint      NOT NULL DEFAULT 0 CHECK (cache_read_tokens >= 0),
    cache_write_tokens bigint      NOT NULL DEFAULT 0 CHECK (cache_write_tokens >= 0),
    duration_ms        bigint      NOT NULL CHECK (duration_ms >= 0),
    price_input        double precision NOT NULL,
    price_output       double precision NOT NULL,
    price_cache_read   double precision NOT NULL,
    price_cache_write  double precision NOT NULL,
    cost_usd           double precision NOT NULL CHECK (cost_usd >= 0),
    created_at         timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX usage_records_org_idx ON usage_records (org_id, created_at);
CREATE INDEX usage_records_exam_idx ON usage_records (exam_id) WHERE exam_id IS NOT NULL;
CREATE INDEX usage_records_invite_idx ON usage_records (invite_id) WHERE invite_id IS NOT NULL;
