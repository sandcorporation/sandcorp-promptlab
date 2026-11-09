-- 발행 전 점검·발행·기준선 대비(#73, spec #69 결정 7·8).

-- 1. 옛 「기준 프롬프트」를 모범 대화(사용자 메시지 목록)의 첫 메시지로 옮긴다(ProblemSource.reference_conversation).
UPDATE org_problems
SET source = (source - 'reference_prompt') || jsonb_build_object(
    'reference_conversation',
    CASE WHEN btrim(coalesce(source ->> 'reference_prompt', '')) = '' THEN '[]'::jsonb
         ELSE jsonb_build_array(source ->> 'reference_prompt') END);
UPDATE bank_problems
SET source = (source - 'reference_prompt') || jsonb_build_object(
    'reference_conversation',
    CASE WHEN btrim(coalesce(source ->> 'reference_prompt', '')) = '' THEN '[]'::jsonb
         ELSE jsonb_build_array(source ->> 'reference_prompt') END);

-- 2. 발행 상태. 발행 = 지금 원본의 마지막 점검이 모두 통과일 때 출제자가 누른 것. 원본을 고치면 셋 다 NULL로
--    돌아간다(store 의 update). 기존 문제는 모두 발행 전 — 다시 점검해야 새 시험·연습 목록에 나온다(이미 만든 시험은 그대로).
--    baseline_rate = 발행 때 기준선 통과율(0~1), 결과의 「기준선 대비」에 쓴다.
ALTER TABLE org_problems
    ADD COLUMN published_at  timestamptz,
    ADD COLUMN published_by  text REFERENCES users (sub),
    ADD COLUMN baseline_rate double precision CHECK (baseline_rate BETWEEN 0 AND 1);
ALTER TABLE bank_problems
    ADD COLUMN published_at  timestamptz,
    ADD COLUMN published_by  text REFERENCES users (sub),
    ADD COLUMN baseline_rate double precision CHECK (baseline_rate BETWEEN 0 AND 1);

-- 3. 미리 돌리기 큐(#16)를 발행 전 점검 큐로 다시 쓴다. 한 줄 = 저장된 문제 하나의 그때 원본(source 스냅숏)에 대한
--    점검. 워커가 채점 큐가 빌 때 기준선 한 걸음, 모범 대화 한 걸음씩 돌린다(걸음마다 채점 큐를 다시 본다).
--    baseline·reference = publication::RunResult(케이스별 통과·토큰 — 숨은 입력·모델 출력 없음).
--    org_id 가 NULL 이면 운영자 문항(#19). 옛 기록은 기준 프롬프트 꼴이라 지운다.
DELETE FROM problem_previews;
ALTER TABLE problem_previews
    DROP COLUMN reference_prompt,
    DROP COLUMN cases,
    ALTER COLUMN org_id DROP NOT NULL,
    ADD COLUMN problem_id text NOT NULL,
    ADD COLUMN baseline   jsonb,
    ADD COLUMN reference  jsonb;
CREATE INDEX problem_previews_problem_idx ON problem_previews (problem_id, created_at DESC);

-- 4. 제출 당시 문제의 기준선 통과율(0~1). 기준선이 없는 옛 문제·옛 제출은 NULL — 「기준선 대비」를 비운다.
ALTER TABLE submissions ADD COLUMN baseline_rate double precision;
