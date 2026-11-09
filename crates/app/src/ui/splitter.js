// 응시 화면 칸 경계 손잡이(#117): 포인터 캡처로 끌고, 위치만 Rust로 보낸다(폭 계산은 workspace.rs).
// 보내는 것: {h: "problem"|"ai", x: 칸 묶음 왼쪽에서 포인터까지 px, total: 칸 묶음 폭, end: bool} 또는 키 {h, step: ±1}.
// 시작 신호 = 이 스크립트가 처음 보내는 {ready:true}.
(async () => {
  if (window.__plSplitterOff) { window.__plSplitterOff(); }
  let drag = null;
  const panes = () => document.querySelector('.panes');
  const send = (m) => dioxus.send(m);
  const down = (e) => {
    const el = e.target.closest && e.target.closest('.splitter');
    if (!el || e.button !== 0) return;
    e.preventDefault();
    el.focus(); // preventDefault가 기본 포커스를 막으므로 직접 준다 — 그래야 ←→가 손잡이로 간다.
    el.setPointerCapture(e.pointerId);
    drag = { el, id: e.pointerId, h: el.dataset.h };
    document.body.classList.add('resizing');
  };
  const move = (e) => {
    if (!drag || e.pointerId !== drag.id) return;
    const p = panes();
    if (!p) return;
    const r = p.getBoundingClientRect();
    send({ h: drag.h, x: Math.round(e.clientX - r.left), total: Math.round(r.width), end: false });
  };
  const up = (e) => {
    if (!drag || e.pointerId !== drag.id) return;
    const p = panes();
    const r = p ? p.getBoundingClientRect() : { left: 0, width: 0 };
    send({ h: drag.h, x: Math.round(e.clientX - r.left), total: Math.round(r.width), end: true });
    try { drag.el.releasePointerCapture(drag.id); } catch (_) {}
    drag = null;
    document.body.classList.remove('resizing');
  };
  const key = (e) => {
    const el = e.target.closest && e.target.closest('.splitter');
    if (!el || (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight')) return;
    e.preventDefault();
    send({ h: el.dataset.h, step: e.key === 'ArrowLeft' ? -1 : 1 });
  };
  document.addEventListener('pointerdown', down, true);
  document.addEventListener('pointermove', move, true);
  document.addEventListener('pointerup', up, true);
  document.addEventListener('pointercancel', up, true);
  document.addEventListener('keydown', key, true);
  window.__plSplitterOff = () => {
    document.removeEventListener('pointerdown', down, true);
    document.removeEventListener('pointermove', move, true);
    document.removeEventListener('pointerup', up, true);
    document.removeEventListener('pointercancel', up, true);
    document.removeEventListener('keydown', key, true);
    document.body.classList.remove('resizing');
    window.__plSplitterOff = null;
  };
  // 응시 화면이 사라지면 Rust가 보내는 신호로 끝낸다.
  await dioxus.recv();
  if (window.__plSplitterOff) window.__plSplitterOff();
})();
