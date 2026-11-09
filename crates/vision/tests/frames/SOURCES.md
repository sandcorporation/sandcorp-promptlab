# 시험 프레임 출처

분석기 판별 시험(`../frames.rs`)에 넣는 **녹화 영상 프레임**이다. 모두 NASA가 만든 공개 영상에서 뽑아
320px 너비로 줄인 JPEG다. NASA 제작물은 미국 저작권법상 저작권이 없다(퍼블릭 도메인,
<https://www.nasa.gov/nasa-brand-center/images-and-media/>). NASA가 이 저장소를 보증한다는 뜻은 없다.
새로 찍은 사람 사진은 넣지 않는다.

| 파일 | 영상(NASA Image and Video Library `nasa_id`) | 위치 | 기대 판정 |
|---|---|---|---|
| frontal-1.jpg | `iss061m2627771232_Live_Interviews_Jeanette_Epps_191004` | 0:00 | 얼굴 하나·정면 |
| frontal-2.jpg | 같은 영상 | 7:15 | 얼굴 하나·정면 |
| frontal-3.jpg | `jsc2024m000139_Crew-9_Crew_News_Conference` | 10:00 | 얼굴 하나·정면 |
| noface-1.jpg | 같은 영상 | 0:30 | 얼굴 없음 |
| noface-2.jpg | 같은 영상 | 27:00 | 얼굴 없음 |
| multiple-1.jpg | 같은 영상 | 0:00 | 여러 명(4) |
| away-1.jpg | 같은 영상 | 4:30 | 시선 이탈(옆을 봄) |
| away-2.jpg | 같은 영상 | 9:00 | 시선 이탈(옆을 봄) |

뽑은 방법: `ffmpeg -i <영상 ~small.mp4> -vf "fps=1/15,scale=320:-2"`(Epps), `fps=1/30`(Crew-9).
영상 주소: `https://images-assets.nasa.gov/video/<nasa_id>/<nasa_id>~small.mp4`.
프레임을 고를 때는 `cargo run --release -p promptlab-vision --example probe -- <폴더>`로 판정·자세 값을 찍었다.
