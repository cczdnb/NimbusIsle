'use strict';
const stage = document.getElementById('host');
const api = window.createCat ? window.createCat(stage, { coat: 'tuxedo', sleepMinutes: 5 }) : null;
const keys = new Set();
const buttons = new Set();
let opts = { purrPet: true, purrSleep: false, privacy: false, layout: '65', opacity: 1 };
let appliedLayout = '65';
const mood = {
  enabled: true, annoy: 0, cross: false, crossSince: 0, lastPoke: -1e15,
  strokes: [], wasAsleep: false, petSince: -1, phase: 0, prevPhase: 0,
  hop: 0, hopV: 0, mark: null, spot: null,
};
const MASH_MS = 150;
const CROSS_AT = 0.6;

function poke(amount, t) {
  if (!mood.enabled || amount <= 0) return;
  mood.annoy = Math.min(1, mood.annoy + amount);
  mood.lastPoke = t;
}

function noteKey(t) {
  mood.strokes.push(t);
  if (mood.strokes.filter((at) => t - at < MASH_MS).length >= 4) poke(0.3, t);
}

function angerMark(scene) {
  if (mood.mark) return mood.mark;
  const canvas = document.createElement('canvas');
  canvas.width = canvas.height = 128;
  const g = canvas.getContext('2d');
  g.lineCap = 'round';
  g.translate(64, 64);
  g.strokeStyle = '#e23b32';
  g.lineWidth = 10;
  for (let i = 0; i < 4; i++) {
    g.save();
    g.rotate((i * Math.PI) / 2 + 0.45);
    g.beginPath();
    g.moveTo(16, -10);
    g.lineTo(36, -22);
    g.moveTo(16, 10);
    g.lineTo(36, 22);
    g.stroke();
    g.restore();
  }
  const sprite = new THREE.Sprite(new THREE.SpriteMaterial({
    map: new THREE.CanvasTexture(canvas), transparent: true, depthTest: false,
  }));
  sprite.renderOrder = 50;
  sprite.visible = false;
  scene.add(sprite);
  mood.mark = sprite;
  mood.spot = new THREE.Vector3();
  return sprite;
}

window.yunyuMood = function (ctx) {
  const t = ctx.t;
  const dt = ctx.dt;
  while (mood.strokes.length && t - mood.strokes[0] > 2000) mood.strokes.shift();
  const rate = mood.strokes.length / 2;
  if (mood.enabled && rate > 9) poke((rate - 9) * 0.05 * dt, t);
  if (ctx.petting) {
    if (mood.petSince < 0) mood.petSince = t;
    if (mood.enabled && t - mood.petSince > 4000) poke(0.3 * dt, t);
  } else mood.petSince = -1;
  if (mood.wasAsleep && !ctx.asleep) poke(0.35, t);
  mood.wasAsleep = ctx.asleep;
  if (!mood.enabled || ctx.asleep) {
    mood.annoy = Math.max(0, mood.annoy - dt * 0.4);
    mood.cross = false;
  } else if (t - mood.lastPoke > 2000) {
    mood.annoy = Math.max(0, mood.annoy - 0.1 * dt);
  }
  if (!mood.cross && mood.annoy >= CROSS_AT) {
    mood.cross = true;
    mood.crossSince = t;
    mood.phase = 0;
    mood.prevPhase = 0;
  }
  if (mood.cross && ctx.stopAct) ctx.stopAct();
  if (mood.cross && mood.annoy < CROSS_AT * 0.42 && t - mood.crossSince > 3000) mood.cross = false;
  window.yunyuBusy = mood.cross || mood.annoy > 0.05;
  if (!mood.cross) {
    mood.hopV *= Math.exp(-dt * 6);
    mood.hop *= Math.exp(-dt * 8);
    ctx.board.position.y -= mood.hop * 0.04;
    if (mood.mark) mood.mark.visible = false;
    return;
  }
  mood.prevPhase = mood.phase;
  mood.phase += dt * 2.6;
  const beat = Math.floor(mood.phase);
  const along = mood.phase - beat;
  const resting = beat % 4 === 3;
  const prevAlong = mood.prevPhase - Math.floor(mood.prevPhase);
  const down = resting ? 0.2 : along < 0.72 ? 0.04 : Math.min(1, ((along - 0.72) / 0.28) ** 0.45);
  ctx.poseTail(down, Math.sin(mood.phase * Math.PI) * 0.35);
  ctx.tail.rotation.z += Math.sin(mood.phase * Math.PI * 2) * 0.08;
  if (!resting && prevAlong < 0.72 && along >= 0.72) mood.hopV += 7;
  mood.hopV += -mood.hop * 80 * dt;
  mood.hopV *= Math.exp(-dt * 7);
  mood.hop += mood.hopV * dt;
  ctx.board.position.y -= Math.max(0, mood.hop) * 0.05;
  const mark = angerMark(ctx.scene);
  ctx.head.getWorldPosition(mood.spot);
  mood.spot.y += 0.62;
  mood.spot.x -= 0.2;
  mark.position.copy(mood.spot);
  const pulse = 0.75 + 0.25 * Math.sin(mood.phase * Math.PI * 2);
  mark.scale.setScalar(0.42 * pulse);
  mark.material.opacity = 0.9;
  mark.visible = true;
};
let smoothX = 0.5, smoothY = 0.5;
let audio, purrSource, purrGain;

function setPurr(level) {
  if (!level) {
    if (purrGain) purrGain.gain.value = 0;
    return;
  }
  if (!audio) {
    audio = new AudioContext();
    const rate = audio.sampleRate;
    const buf = audio.createBuffer(1, rate, rate);
    const data = buf.getChannelData(0);
    let noise = 0;
    for (let i = 0; i < rate; i++) {
      noise = noise * 0.97 + (Math.random() * 2 - 1) * 0.03;
      const hum = Math.sin((2 * Math.PI * 26 * i) / rate);
      const breath = 0.55 + 0.45 * Math.sin((2 * Math.PI * 3.1 * i) / rate);
      data[i] = (noise * 7 + hum * 0.4) * breath;
    }
    purrSource = audio.createBufferSource();
    purrSource.buffer = buf;
    purrSource.loop = true;
    purrGain = audio.createGain();
    purrGain.gain.value = 0;
    purrSource.connect(purrGain);
    purrGain.connect(audio.destination);
    purrSource.start();
  }
  if (audio.state === 'suspended') audio.resume();
  purrGain.gain.value = level;
}

function releaseKeys() {
  keys.clear();
  if (api) api.clear();
}

function apply(message) {
  const privacy = !!message.privacy;
  const privacyChanged = !!opts.privacy !== privacy;
  opts = Object.assign(opts, message);
  opts.privacy = privacy;
  mood.enabled = message.annoy !== false;
  stage.style.opacity = String(opts.opacity == null ? 1 : opts.opacity);
  if (!api) return;
  if (privacyChanged) releaseKeys();
  api.setOptions({
    showKeyboard: true,
    showMouse: true,
    followKeys: true,
    leftMouse: !!opts.leftMouse,
    sleepMinutes: opts.sleep ? opts.sleepMinutes : 0,
    showKeyLabels: !!opts.labels && !opts.privacy,
    coat: opts.coat || 'tuxedo',
  });
  const layout = opts.layout === '104' ? '104' : '65';
  if (layout !== appliedLayout) {
    appliedLayout = layout;
    releaseKeys();
    api.setLayout(layout);
  }
}

function gaze(nx, ny, dx, dy) {
  if (!api) return;
  const k = 1 - Math.exp(-0.033 * 14);
  smoothX += (nx - smoothX) * k;
  smoothY += (ny - smoothY) * k;
  const center = opts.layout === '104' ? 0.49 : 0.47;
  api.setPointerNdc(Math.tanh((smoothX - center) * 1.1), Math.tanh((smoothY - 0.33) * 1.1));
  if (dx || dy) {
    api.nudgeMouse(Math.max(-0.16, Math.min(0.16, dx)), Math.max(-0.12, Math.min(0.12, dy)));
    api.mouseMoved();
    api.setMouseActive();
  }
}

if (window.chrome && chrome.webview) {
  chrome.webview.addEventListener('message', (event) => {
    const message = typeof event.data === 'string' ? JSON.parse(event.data) : event.data;
    if (!message) return;
    if (message.t === 'opt') apply(message);
    else if (message.t === 'key' && api) {
      if (opts.privacy) {
        if (keys.size) releaseKeys();
        if (message.down && !message.repeat) {
          noteKey(performance.now());
          api.tap(message.side === 'L' ? 'Left' : 'Right');
        }
        return;
      }
      if (!message.code) {
        if (message.down && !message.repeat) {
          noteKey(performance.now());
          api.tap(message.side === 'L' ? 'Left' : 'Right');
        }
        return;
      }
      if (message.down) {
        if (message.repeat) return;
        keys.add(message.code);
        noteKey(performance.now());
      } else keys.delete(message.code);
      api.setInput(keys, buttons);
    } else if (message.t === 'mouse' && api) {
      buttons.clear();
      if (message.btn & 1) buttons.add(0);
      if (message.btn & 2) buttons.add(2);
      api.setInput(opts.privacy ? new Set() : keys, buttons);
      if (message.nx != null) gaze(message.nx, message.ny, message.dx || 0, message.dy || 0);
    }
  });
}

let framed = '';
function postFrame() {
  if (!api || !window.chrome || !chrome.webview) return;
  const sig = `${opts.layout}:${opts.leftMouse ? 1 : 0}`;
  if (framed === sig) return;
  const regions = api.hitRegions();
  if (!regions.length) return;
  let x0 = 1, y0 = 1, x1 = 0, y1 = 0;
  for (const region of regions) {
    x0 = Math.min(x0, region[0]);
    y0 = Math.min(y0, region[1]);
    x1 = Math.max(x1, region[0] + region[2]);
    y1 = Math.max(y1, region[1] + region[3]);
  }
  const w = x1 - x0;
  const h = y1 - y0;
  if (!(w >= 0.2 && h >= 0.2 && x0 >= 0 && y0 >= 0 && x1 <= 1 && y1 <= 1)) return;
  framed = sig;
  chrome.webview.postMessage({ t: 'frame', r: [x0, y0, w, h] });
}

setInterval(() => {
  if (!api || !window.chrome || !chrome.webview) return;
  postFrame();
  const regions = api.hitRegions();
  if (regions.length) chrome.webview.postMessage({ t: 'hit', r: regions });
  const state = api.state();
  const level = state === 'petting' && opts.purrPet ? 0.45 : state === 'sleeping' && opts.purrSleep ? 0.18 : 0;
  setPurr(level);
}, 180);
