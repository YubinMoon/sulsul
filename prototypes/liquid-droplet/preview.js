const root = document.documentElement;
const canvas = document.getElementById('water');
const timeline = document.getElementById('time');
const slow = document.getElementById('slow');
const clock = document.getElementById('clock');
const DURATION = 1.3;
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

const gl = canvas.getContext('webgl2', {
  alpha: false, antialias: false, depth: false, stencil: false,
  powerPreference: 'high-performance', preserveDrawingBuffer: true,
});

let running = false;
let time = 0;
let last = 0;
let light = false;
let frame = null;

function fail(message) {
  const error = document.getElementById('error');
  error.textContent = message;
  error.hidden = false;
  root.dataset.ready = 'error';
}

if (!gl) {
  fail(t('unsupported'));
} else {
  try {
    const fragmentSource = await (await fetch('water.frag')).text();
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
    const vao = gl.createVertexArray();
    gl.bindVertexArray(vao);
    const uniforms = Object.fromEntries(['resolution', 'time', 'light'].map(name => [name, gl.getUniformLocation(program, `u_${name}`)]));

    function draw() {
      gl.uniform2f(uniforms.resolution, canvas.width, canvas.height);
      gl.uniform1f(uniforms.time, time);
      gl.uniform1f(uniforms.light, light ? 1 : 0);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
      timeline.value = time;
      clock.value = time.toFixed(2);
      root.dataset.time = time.toFixed(3);
    }

    function resize() {
      const ratio = Math.min(devicePixelRatio || 1, 1.75);
      canvas.width = Math.round(canvas.clientWidth * ratio);
      canvas.height = Math.round(canvas.clientHeight * ratio);
      gl.viewport(0, 0, canvas.width, canvas.height);
      draw();
    }

    function animate(now) {
      frame = null;
      if (!running) return;
      const dt = (now - last) / 1000;
      last = now;
      time = Math.min(DURATION, time + dt * (slow.checked ? .32 : 1));
      draw();
      if (time < DURATION) frame = requestAnimationFrame(animate);
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
      time = Math.max(0, Math.min(DURATION, Number(value)));
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
    document.getElementById('background').addEventListener('click', () => {
      light = !light;
      root.classList.toggle('light', light);
      draw();
    });
    canvas.addEventListener('webglcontextlost', event => {
      event.preventDefault();
      running = false;
      if (frame !== null) cancelAnimationFrame(frame);
      fail(t('unsupported'));
    });
    new ResizeObserver(resize).observe(canvas);
    document.addEventListener('visibilitychange', () => {
      if (document.hidden) {
        running = false;
        if (frame !== null) cancelAnimationFrame(frame);
        frame = null;
      }
    });

    // 미리보기와 프레임 검사에서 같은 동작을 같은 시각으로 살펴봅니다.
    window.dropletPreview = {
      replay, seek,
      get time() { return time; },
      get running() { return running; },
      get renderer() { return gl.getParameter(gl.RENDERER); },
    };
    resize();
    root.dataset.ready = 'true';
    if (matchMedia('(prefers-reduced-motion: reduce)').matches) seek(DURATION);
    else replay();
  } catch (error) {
    console.error(error);
    fail(t('failed'));
  }
}
