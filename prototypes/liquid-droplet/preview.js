const root = document.documentElement;
// 아래부터 뒤 화면, 물방울 너머로 휘어 보이는 화면, 물방울 레이어 순서로 겹칩니다.
const backdrop = document.getElementById('backdrop');
const lensCanvas = document.getElementById('lens');
const canvas = document.getElementById('water');
const timeline = document.getElementById('time');
const slow = document.getElementById('slow');
const sample = document.getElementById('sample');
const lensOn = document.getElementById('lens-on');
const clock = document.getElementById('clock');
const popButton = document.getElementById('pop');
const doneButton = document.getElementById('done');
const popTimeline = document.getElementById('pop-time');
const popClock = document.getElementById('pop-clock');
// 등장 동작 길이(실제 초). water.frag의 장면 시계가 INTRO(1.3)에 닿는 순간입니다(RISE_SPEED 3일 때 약 0.44초).
const DURATION = 0.45;
// 터지는 동작 길이(초). water.frag의 POP_LENGTH와 같습니다.
const POP_LENGTH = 0.75;
// 후처리가 끝난 뒤 고리가 모여 뿅 하고 사라지기까지의 시간(초). water.frag의 DONE_LENGTH와 같습니다.
const DONE_LENGTH = 0.62;
// 터짐·기다림 시간 막대의 끝(초). 고리가 모인 뒤 한 바퀴 남짓 도는 데까지 볼 수 있습니다.
const POP_SHOWN = 2.6;
const BACKGROUNDS = ['#000', '#292929', '#b8b8b8', '#fff'];
// 예시 화면에 놓는 색 카드
const CARDS = ['#d9683a', '#2f9e74', '#3f7fd4', '#a65bc4'];
const CONTEXT = {
  alpha: true, premultipliedAlpha: false, antialias: false, depth: false, stencil: false,
  powerPreference: 'high-performance', preserveDrawingBuffer: true,
};
const reducedMotion = matchMedia('(prefers-reduced-motion: reduce)').matches;
const labels = Object.fromEntries(
  (await (await fetch('locales/ko.yml')).text()).trim().split('\n').map(line => {
    const at = line.indexOf(':');
    return [line.slice(0, at).trim(), line.slice(at + 1).trim()];
  }),
);
const t = key => labels[key];
timeline.max = DURATION;
popTimeline.max = POP_SHOWN;
document.title = `${t('title')} · ${t('subtitle')}`;
document.querySelectorAll('[data-i18n]').forEach(node => { node.textContent = t(node.dataset.i18n); });
canvas.setAttribute('aria-label', t('canvas'));
timeline.setAttribute('aria-label', t('timeline'));
popTimeline.setAttribute('aria-label', t('pop_timeline'));
document.querySelector('.backgrounds').setAttribute('aria-label', t('background'));

let running = false;
let time = 0;
// 터지기 시작한 시각(time 기준). 아직 터지지 않았으면(녹음 중) null입니다.
let popAt = null;
// 후처리가 끝난 시각(time 기준). 아직 기다리는 중이면 null입니다.
let doneAt = null;
let last = 0;
let frame = null;
let background = 0;

function fail(message) {
  const error = document.getElementById('error');
  error.textContent = message;
  error.hidden = false;
  root.dataset.ready = 'error';
}

// 캔버스 하나에 셰이더를 올립니다. 화면을 덮는 삼각형 하나만 그립니다.
function createPass(target, fragmentSource) {
  const gl = target.getContext('webgl2', CONTEXT);
  if (!gl) return null;
  const vertexSource = `#version 300 es
  void main() {
    vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
  }`;
  function compile(type, source) {
    const shader = gl.createShader(type);
    gl.shaderSource(shader, source);
    gl.compileShader(shader);
    if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
      const message = gl.getShaderInfoLog(shader);
      gl.deleteShader(shader);
      throw new Error(message);
    }
    return shader;
  }
  const program = gl.createProgram();
  const vertex = compile(gl.VERTEX_SHADER, vertexSource);
  const fragment = compile(gl.FRAGMENT_SHADER, fragmentSource);
  gl.attachShader(program, vertex);
  gl.attachShader(program, fragment);
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(program));
  gl.deleteShader(vertex);
  gl.deleteShader(fragment);
  gl.useProgram(program);
  gl.bindVertexArray(gl.createVertexArray());
  const resolution = gl.getUniformLocation(program, 'u_resolution');
  const clockTime = gl.getUniformLocation(program, 'u_time');
  const popClockTime = gl.getUniformLocation(program, 'u_pop');
  const doneClockTime = gl.getUniformLocation(program, 'u_done');
  return {
    gl, program,
    draw(seconds, pop, done) {
      gl.viewport(0, 0, target.width, target.height);
      gl.uniform2f(resolution, target.width, target.height);
      gl.uniform1f(clockTime, seconds);
      gl.uniform1f(popClockTime, pop);
      gl.uniform1f(doneClockTime, done);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    },
  };
}

// 예시 화면: 글과 색 카드. 물방울이 떠 있는 화면 아래 가운데에 글줄과 카드 가장자리가 지나가게 놓습니다.
function paintSample(ctx, ratio) {
  const width = backdrop.width / ratio;
  const height = backdrop.height / ratio;
  const ink = background >= 2 ? '24 28 26' : '238 242 239';
  const column = Math.min(720, width - 48);
  const left = (width - column) / 2;
  ctx.save();
  ctx.scale(ratio, ratio);
  ctx.textBaseline = 'alphabetic';

  const gap = 14;
  const cardTop = Math.round(height * 0.883);
  const cardWidth = (column - gap * 3) / 4;
  ctx.font = '13px Pretendard, sans-serif';
  CARDS.forEach((color, index) => {
    const x = left + index * (cardWidth + gap);
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.roundRect(x, cardTop, cardWidth, height - cardTop - 16, 10);
    ctx.fill();
    ctx.fillStyle = 'rgb(255 255 255 / 92%)';
    ctx.fillText(t('sample_title'), x + 14, cardTop + 28, cardWidth - 28);
  });

  ctx.fillStyle = `rgb(${ink} / 94%)`;
  ctx.font = '26px Pretendard, sans-serif';
  ctx.fillText(t('sample_title'), left, 178);
  ctx.fillStyle = `rgb(${ink} / 80%)`;
  ctx.font = '15px Pretendard, sans-serif';
  const words = t('sample_text').split(' ');
  let y = 218;
  // 같은 글을 문단으로 되풀이해 카드 바로 위까지 채웁니다.
  while (y < cardTop - 12) {
    let line = '';
    for (const word of words) {
      const next = line ? `${line} ${word}` : word;
      if (line && ctx.measureText(next).width > column) {
        if (y < cardTop - 12) ctx.fillText(line, left, y);
        y += 27;
        line = word;
      } else {
        line = next;
      }
    }
    if (y < cardTop - 12) ctx.fillText(line, left, y);
    y += 41;
  }
  ctx.restore();
}

try {
  const fragmentSource = await (await fetch('water.frag')).text();
  const layer = createPass(canvas, fragmentSource);
  if (!layer) {
    fail(t('unsupported'));
  } else {
    // 같은 셰이더를 LENS로 한 번 더 컴파일해 물방울 너머로 휘어 보이는 뒤 화면을 그립니다.
    const lens = createPass(lensCanvas, fragmentSource.replace('#version 300 es', '#version 300 es\n#define LENS 1'));
    if (lens) {
      const { gl } = lens;
      gl.bindTexture(gl.TEXTURE_2D, gl.createTexture());
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, true);
      gl.uniform1i(gl.getUniformLocation(lens.program, 'u_backdrop'), 0);
    }
    await document.fonts.load('15px Pretendard');

    // 터지기 시작한 뒤 지난 시간. 아직 터지지 않았으면 -1입니다.
    function popTime() {
      return popAt === null ? -1 : time - popAt;
    }

    // 후처리가 끝난 뒤 지난 시간. 아직 기다리는 중이면 -1입니다.
    function doneTime() {
      return doneAt === null ? -1 : time - doneAt;
    }

    // 다 터지고 고리도 사라져 더 그릴 것이 없는 시각. 아직 기다리는 중이면 null입니다.
    function endTime() {
      return popAt === null || doneAt === null ? null : Math.max(popAt + POP_LENGTH, doneAt + DONE_LENGTH);
    }

    function draw() {
      const pop = popTime();
      const done = doneTime();
      if (lens && lensOn.checked) lens.draw(time, pop, done);
      layer.draw(time, pop, done);
      const introTime = Math.min(time, DURATION);
      if (root.dataset.time !== introTime.toFixed(3)) {
        timeline.value = introTime;
        clock.value = introTime.toFixed(2);
        root.dataset.time = introTime.toFixed(3);
      }
      const popShown = Math.min(Math.max(pop, 0), POP_SHOWN);
      if (root.dataset.pop !== popShown.toFixed(3)) {
        popTimeline.value = popShown;
        popClock.value = popShown.toFixed(2);
        root.dataset.pop = popShown.toFixed(3);
      }
      popButton.disabled = popAt !== null;
      doneButton.disabled = popAt === null || doneAt !== null;
    }

    // 뒤 화면을 다시 그립니다. 고른 배경색을 깔고, 예시 화면을 켰으면 글과 색 카드를 얹습니다.
    // 물방울 레이어는 건드리지 않고, 렌즈가 옮겨 올 뒤 화면 그림만 바꿉니다.
    function paintBackdrop() {
      const ctx = backdrop.getContext('2d');
      ctx.fillStyle = BACKGROUNDS[background];
      ctx.fillRect(0, 0, backdrop.width, backdrop.height);
      if (sample.checked) paintSample(ctx, backdrop.width / backdrop.clientWidth);
      lensCanvas.hidden = !(lens && lensOn.checked);
      if (lens && lensOn.checked) {
        const { gl } = lens;
        gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, backdrop);
        gl.generateMipmap(gl.TEXTURE_2D);
        lens.draw(time, popTime(), doneTime());
      }
    }

    function resize() {
      const ratio = Math.min(devicePixelRatio || 1, 1.75);
      for (const target of [backdrop, lensCanvas, canvas]) {
        target.width = Math.round(canvas.clientWidth * ratio);
        target.height = Math.round(canvas.clientHeight * ratio);
      }
      paintBackdrop();
      draw();
    }

    function animate(now) {
      frame = null;
      if (!running) return;
      const dt = (now - last) / 1000;
      last = now;
      time += dt * (slow.checked ? .32 : 1);
      // 다 터지고 고리도 사라졌으면 마지막 모습(아무것도 없음)에서 멈춥니다. 기다리는 동안에는 계속 돕니다.
      const end = endTime();
      const finished = end !== null && time >= end;
      if (finished) time = end;
      draw();
      if (!finished && (time < DURATION || !reducedMotion)) frame = requestAnimationFrame(animate);
      else running = false;
    }

    // 녹음을 시작한 것처럼 수면에서 다시 떠오릅니다.
    function replay() {
      if (frame !== null) cancelAnimationFrame(frame);
      time = 0;
      popAt = null;
      doneAt = null;
      last = performance.now();
      running = true;
      frame = requestAnimationFrame(animate);
    }

    // 멈춰 있으면 다시 움직이고, 지금 모습을 그립니다. 움직임 줄이기를 켰으면 그리기만 합니다.
    function keepRunning() {
      if (!reducedMotion && !running) {
        running = true;
        last = performance.now();
        frame = requestAnimationFrame(animate);
      }
      draw();
    }

    // 녹음이 끝난 것처럼 지금 자리에서 터뜨리고, 후처리를 기다리는 고리를 띄웁니다.
    // 움직임 줄이기를 켰으면 터지는 동작 없이 멈춘 고리만 보입니다.
    function pop() {
      if (popAt !== null) return;
      popAt = reducedMotion ? time - POP_LENGTH : time;
      keepRunning();
    }

    // 후처리가 끝난 것처럼 고리를 거둡니다. 움직임 줄이기를 켰으면 바로 사라집니다.
    function finish() {
      if (popAt === null || doneAt !== null) return;
      doneAt = reducedMotion ? time - DONE_LENGTH : time;
      keepRunning();
    }

    // 누를 때마다 터뜨리기, 처리 끝, 다시 보기 순서로 넘어갑니다.
    function poke() {
      if (popAt === null) pop();
      else if (doneAt === null) finish();
      else replay();
    }

    function seek(value) {
      running = false;
      if (frame !== null) cancelAnimationFrame(frame);
      frame = null;
      popAt = null;
      doneAt = null;
      time = Math.max(0, Number(value));
      draw();
    }

    // 터지는 동작이나 기다리는 동작의 한 순간에 멈춰 봅니다. 아직 터지지 않았으면 지금 자리에서 터뜨린 것으로 봅니다.
    function seekPop(value) {
      running = false;
      if (frame !== null) cancelAnimationFrame(frame);
      frame = null;
      doneAt = null;
      if (popAt === null) popAt = time;
      time = popAt + Math.max(0, Number(value));
      draw();
    }

    // 후처리가 끝난 뒤의 한 순간에 멈춰 봅니다. 아직 기다리는 중이면 지금 끝난 것으로 봅니다.
    function seekDone(value) {
      running = false;
      if (frame !== null) cancelAnimationFrame(frame);
      frame = null;
      if (popAt === null) popAt = time;
      if (doneAt === null) doneAt = time;
      time = doneAt + Math.max(0, Number(value));
      draw();
    }

    document.getElementById('replay').addEventListener('click', replay);
    popButton.addEventListener('click', pop);
    doneButton.addEventListener('click', finish);
    canvas.addEventListener('click', poke);
    canvas.addEventListener('keydown', event => {
      if (event.code === 'Space' || event.code === 'Enter') {
        event.preventDefault();
        poke();
      }
    });
    timeline.addEventListener('input', () => seek(timeline.value));
    popTimeline.addEventListener('input', () => seekPop(popTimeline.value));
    const backgrounds = document.querySelectorAll('[data-background]');
    backgrounds.forEach(button => {
      button.addEventListener('click', () => {
        background = Number(button.dataset.background);
        root.style.setProperty('--preview-background', BACKGROUNDS[background]);
        root.classList.toggle('light', background >= 2);
        backgrounds.forEach(option => option.setAttribute('aria-pressed', String(option === button)));
        paintBackdrop();
      });
    });
    sample.addEventListener('change', paintBackdrop);
    lensOn.addEventListener('change', paintBackdrop);
    canvas.addEventListener('webglcontextlost', event => {
      event.preventDefault();
      running = false;
      if (frame !== null) cancelAnimationFrame(frame);
      fail(t('unsupported'));
    });
    new ResizeObserver(resize).observe(canvas);
    document.addEventListener('visibilitychange', () => {
      if (document.hidden) {
        if (frame !== null) cancelAnimationFrame(frame);
        frame = null;
      } else if (running && frame === null) {
        last = performance.now();
        frame = requestAnimationFrame(animate);
      }
    });

    // 미리보기와 프레임 검사에서 같은 동작을 같은 시각으로 살펴봅니다.
    window.dropletPreview = {
      replay, seek, pop, finish, seekPop, seekDone,
      get time() { return time; },
      get popTime() { return popTime(); },
      get doneTime() { return doneTime(); },
      get running() { return running; },
      get renderer() { return layer.gl.getParameter(layer.gl.RENDERER); },
    };
    resize();
    root.dataset.ready = 'true';
    if (reducedMotion) seek(DURATION);
    else replay();
  }
} catch (error) {
  console.error(error);
  fail(t('failed'));
}
