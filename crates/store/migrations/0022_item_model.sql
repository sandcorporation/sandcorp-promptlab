-- 쓴 모델(#131): 문항 시작 때 앱이 신고한 모델 파일 sha256(허용 목록을 통과한 값). 결과·응시자 상세·AI 활용 평가에
-- 모델 이름으로 풀어 싣는다(판단은 고객사). 신고가 없으면(스모크 등) NULL. 다시 시작하면 마지막 신고로 바꾼다.
ALTER TABLE exam_item_starts ADD COLUMN model_sha256 text;
