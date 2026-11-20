// The Klont: a circle displaced by noise. Spike height follows Bright and Grit,
// the wobble follows Motion, the halo follows Space, and the grain follows noise mix.
// The same numbers are the live macros when the engine is running.

const VERT = `#version 300 es
layout(location=0) in vec2 a;
void main() { gl_Position = vec4(a, 0.0, 1.0); }`;

const FRAG = `#version 300 es
precision mediump float;
uniform vec2 u_res;
uniform float u_time;
uniform float u_bright;
uniform float u_grit;
uniform float u_motion;
uniform float u_space;
uniform float u_noise;
out vec4 o;
float hash(vec2 p) { return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453); }
float noise(vec2 p) {
  vec2 i = floor(p);
  vec2 f = fract(p);
  float a = hash(i), b = hash(i + vec2(1.0, 0.0)), c = hash(i + vec2(0.0, 1.0)), d = hash(i + vec2(1.0, 1.0));
  vec2 u = f * f * (3.0 - 2.0 * f);
  return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}
void main() {
  vec2 uv = (gl_FragCoord.xy / u_res) * 2.0 - 1.0;
  uv.x *= u_res.x / max(1.0, u_res.y);
  float wob = u_motion * 0.18 * sin(u_time * (0.5 + u_motion * 2.5));
  vec2 p = uv + vec2(wob, 0.0);
  float n = noise(p * (3.0 + u_grit * 5.0) + u_time * 0.15);
  n += 0.5 * noise(p * 6.0 + 4.0);
  n += 0.25 * noise(p * 12.0);
  float spike = 0.06 + u_bright * 0.22 + u_grit * 0.16;
  float edge = 0.52 + spike * (n - 0.5);
  float body = smoothstep(edge, edge - 0.04, length(p));
  float halo = (0.12 + u_space * 0.6) * (1.0 - smoothstep(edge, edge + 0.28 + u_space * 0.45, length(p)));
  float grain = (hash(gl_FragCoord.xy + u_time) - 0.5) * u_noise * 0.35;
  vec3 col = vec3(0.94, 0.70, 0.16) * body + vec3(0.55, 0.72, 0.95) * halo + grain;
  o = vec4(col, 1.0);
}`;

export interface KlontDrive {
  bright: number;
  grit: number;
  motion: number;
  space: number;
  noise: number;
}

function shader(gl: WebGL2RenderingContext, type: number, src: string): WebGLShader | null {
  const sh = gl.createShader(type);
  if (!sh) return null;
  gl.shaderSource(sh, src);
  gl.compileShader(sh);
  if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) { gl.deleteShader(sh); return null; }
  return sh;
}

export class KlontGlyph {
  private gl: WebGL2RenderingContext | null = null;
  private prog: WebGLProgram | null = null;
  private readonly start = performance.now();
  readonly ok: boolean;

  constructor(private readonly canvas: HTMLCanvasElement) {
    const gl = canvas.getContext('webgl2', { alpha: false, antialias: true });
    this.gl = gl;
    if (!gl) { this.ok = false; return; }
    const prog = gl.createProgram();
    const vs = shader(gl, gl.VERTEX_SHADER, VERT);
    const fs = shader(gl, gl.FRAGMENT_SHADER, FRAG);
    if (!prog || !vs || !fs) { this.ok = false; return; }
    gl.attachShader(prog, vs);
    gl.attachShader(prog, fs);
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) { this.ok = false; return; }
    this.prog = prog;
    const buf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buf);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
    this.ok = true;
  }

  draw(v: KlontDrive, reduceMotion: boolean): void {
    const gl = this.gl;
    const prog = this.prog;
    if (!gl || !prog) return;
    const dpr = window.devicePixelRatio || 1;
    const w = Math.max(1, Math.round(this.canvas.clientWidth * dpr));
    const h = Math.max(1, Math.round(this.canvas.clientHeight * dpr));
    if (this.canvas.width !== w || this.canvas.height !== h) { this.canvas.width = w; this.canvas.height = h; }
    gl.viewport(0, 0, w, h);
    gl.useProgram(prog);
    const loc = (n: string) => gl.getUniformLocation(prog, n);
    gl.uniform2f(loc('u_res'), w, h);
    gl.uniform1f(loc('u_time'), reduceMotion ? 0 : (performance.now() - this.start) / 1000);
    gl.uniform1f(loc('u_bright'), v.bright);
    gl.uniform1f(loc('u_grit'), v.grit);
    gl.uniform1f(loc('u_motion'), v.motion);
    gl.uniform1f(loc('u_space'), v.space);
    gl.uniform1f(loc('u_noise'), v.noise);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  }
}
