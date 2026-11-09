-- 코딩테스트 전환(#95, spec #93): 프롬프팅 응시·채점을 지우고 코딩 문제(언어 + 코드 → 판정기 통과 수)로 바꾼다.
-- 출시 전이라 옛 데이터는 옮기지 않고 지운다(호환 없음).
--
-- 비우는 것(DELETE — FK 동작을 지킨다):
--   submissions(→ submission_evaluations CASCADE), usage_records 중 kind grading·preview·recheck,
--   exams(→ exam_items·exam_invites·exam_item_starts·exam_log_keys·exam_log_events·exam_log_rejections·
--   exam_integrity_rejections·exam_webcam_consents·exam_webcam_photos·exam_reports·시험별 retention_changes CASCADE,
--   erasure_audit.exam_id SET NULL — 스모크 시험 포함, smoke::prepare가 다시 만든다),
--   org_problems, bank_problems, problem_retirements, problem_previews.
-- 남기는 것: users·orgs·memberships·org_invites·sessions·terms_acceptances·조직 보관 기간·erasure_audit·
--   integrity_allowlist_changes, usage_records 중 report·evaluation.
--
-- 지우는 표: exam_example_rechecks(공개 예시 대조).
-- 지우는 열: submissions.prompt·turns·conversation_id·conversations·calls·baseline_rate,
--   org_problems.baseline_rate, bank_problems.baseline_rate, exams.max_tokens·max_calls, exam_items.max_turns,
--   problem_previews.baseline·reference.
-- 더하는 열: submissions.language·code, problem_previews.result(publication::CheckResult).
-- 원본 jsonb(org_problems·bank_problems·problem_previews.source)는 이제 shared::authoring::ProblemSource(코딩 문제).

DELETE FROM submissions;
DELETE FROM usage_records WHERE kind IN ('grading', 'preview', 'recheck');
DELETE FROM exams;
DELETE FROM problem_previews;
DELETE FROM org_problems;
DELETE FROM bank_problems;
DELETE FROM problem_retirements;

DROP TABLE exam_example_rechecks;

ALTER TABLE submissions
    DROP COLUMN prompt,
    DROP COLUMN turns,
    DROP COLUMN conversation_id,
    DROP COLUMN conversations,
    DROP COLUMN calls,
    DROP COLUMN baseline_rate,
    ADD COLUMN language text NOT NULL CHECK (language IN ('python', 'javascript', 'java', 'cpp')),
    ADD COLUMN code     text NOT NULL;

ALTER TABLE org_problems DROP COLUMN baseline_rate;
ALTER TABLE bank_problems DROP COLUMN baseline_rate;

ALTER TABLE exams
    DROP COLUMN max_tokens,
    DROP COLUMN max_calls;
ALTER TABLE exam_items DROP COLUMN max_turns;

-- 점검 한 걸음 = 모범 답안을 예제 + 히든으로 판정기에 한 번. total = 예제 + 히든 수.
ALTER TABLE problem_previews
    DROP COLUMN baseline,
    DROP COLUMN reference,
    ADD COLUMN result jsonb;

ALTER TABLE usage_records DROP CONSTRAINT usage_records_kind_check;
ALTER TABLE usage_records ADD CONSTRAINT usage_records_kind_check CHECK (kind IN ('report', 'evaluation'));
