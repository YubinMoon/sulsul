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
const DURATION = 1.3;
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
document.title = `${t('title')} · ${t('subtitle')}`;
document.querySelectorAll('[data-i18n]').forEach(node => { node.textContent = t(node.dataset.i18n); });
canvas.setAttribute('aria-label', t('canvas'));
timeline.setAttribute('aria-label', t('timeline'));
document.querySelector('.backgrounds').setAttribute('aria-label', t('background'));

let running = false;
let time = 0;
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
  return {
    gl, program,
    draw(seconds) {
      gl.viewport(0, 0, target.width, target.height);
      gl.uniform2f(resolution, target.width, target.height);
      gl.uniform1f(clockTime, seconds);
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

    function draw() {
      if (lens && lensOn.checked) lens.draw(time);
      layer.draw(time);
      const introTime = Math.min(time, DURATION);
      if (root.dataset.time !== introTime.toFixed(3)) {
        timeline.value = introTime;
        clock.value = introTime.toFixed(2);
        root.dataset.time = introTime.toFixed(3);
      }
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
        lens.draw(time);
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
      draw();
      if (time < DURATION || !reducedMotion) frame = requestAnimationFrame(animate);
      else running = false;
    }

    function replay() {
      if (frame !== null) cancelAnimationFrame(frame);
      time = 0;
      last = performance.now();
      running = true;
      frame = requestAnimationFrame(animate);
    }

    function seek(value) {
      running = false;
      if (frame !== null) cancelAnimationFrame(frame);
      frame = null;
      time = Math.max(0, Number(value));
      draw();
    }

    document.getElementById('replay').addEventListener('click', replay);
    canvas.addEventListener('click', replay);
    canvas.addEventListener('keydown', event => {
      if (event.code === 'Space' || event.code === 'Enter') {
        event.preventDefault();
        replay();
      }
    });
    timeline.addEventListener('input', () => seek(timeline.value));
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
      replay, seek,
      get time() { return time; },
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
