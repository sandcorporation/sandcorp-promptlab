-- 「스모크」 조직(#28). 운영 끝에서 끝 스모크(기계 토큰)가 응시·제출하는 시험 전용 조직이다.
-- 대시보드 목록·조직 화면·결과 집계에서 빠진다(구성원 조회가 이 조직을 보지 않는다). 열 추가만 한다(기존 행은 false).
ALTER TABLE orgs ADD COLUMN is_smoke boolean NOT NULL DEFAULT false;
-- 스모크 조직은 하나뿐이다(멱등 생성의 기준).
CREATE UNIQUE INDEX orgs_one_smoke_idx ON orgs ((true)) WHERE is_smoke;
