#version 460 core
#include <flutter/runtime_effect.glsl>

precision mediump float;

// Set from Dart, in this order.
uniform vec2 uSize;     // canvas size in pixels
uniform float uTime;    // seconds
uniform float uPan;     // camera x in metres, for parallax
uniform float uHorizon; // where the ground line sits, 0 = top, 1 = bottom

out vec4 fragColor;

float hash(vec2 p) {
  return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453);
}

// Smooth value noise.
float noise(vec2 p) {
  vec2 i = floor(p);
  vec2 f = fract(p);
  f = f * f * (3.0 - 2.0 * f);
  return mix(mix(hash(i), hash(i + vec2(1.0, 0.0)), f.x),
             mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), f.x), f.y);
}

// Three layers of noise at rising frequency: cloud-like detail.
float fbm(vec2 p) {
  float v = 0.0;
  float a = 0.5;
  for (int i = 0; i < 3; i++) {
    v += a * noise(p);
    p *= 2.03;
    a *= 0.5;
  }
  return v;
}

void main() {
  vec2 uv = FlutterFragCoord().xy / uSize; // 0..1, y down
  float aspect = uSize.x / uSize.y;
  float h = uHorizon - uv.y;               // height above the ground line
  float t = clamp(h / max(uHorizon, 0.001), 0.0, 1.0);

  // dusk gradient: orange at the horizon, violet, then deep blue
  vec3 col = mix(vec3(0.98, 0.58, 0.34), vec3(0.47, 0.29, 0.47), smoothstep(0.0, 0.45, t));
  col = mix(col, vec3(0.07, 0.09, 0.22), smoothstep(0.35, 1.0, t));

  // stars, only high up
  float star = step(0.9975, hash(floor(uv * uSize / 3.0)));
  col += star * smoothstep(0.55, 1.0, t) * (0.6 + 0.4 * sin(uTime * 2.0 + uv.x * 90.0));

  // sun with a soft glow, drifting slightly with the camera
  vec2 sun = vec2((0.74 - uPan * 0.004) * aspect, uHorizon - 0.10);
  float d = distance(vec2(uv.x * aspect, uv.y), sun);
  col += vec3(1.0, 0.72, 0.42) * 0.30 / (1.0 + 45.0 * d * d);
  col = mix(col, vec3(1.0, 0.94, 0.78), smoothstep(0.055, 0.045, d));

  // drifting clouds
  vec2 cp = vec2(uv.x * aspect * 2.2 + uTime * 0.02 + uPan * 0.02, uv.y * 5.0);
  float cloud = smoothstep(0.45, 0.75, fbm(cp)) * smoothstep(0.05, 0.35, t);
  col = mix(col, vec3(1.0, 0.80, 0.76), cloud * 0.5);

  // two ranges of hills; the nearer one moves more with the camera
  float far = 0.11 + 0.09 * fbm(vec2(uv.x * aspect * 1.8 + uPan * 0.03, 1.0));
  if (h < far) col = mix(col, vec3(0.22, 0.17, 0.33), 0.8);
  float near = 0.04 + 0.06 * fbm(vec2(uv.x * aspect * 3.1 + uPan * 0.07, 7.0));
  if (h < near) col = vec3(0.12, 0.11, 0.21);

  fragColor = vec4(col, 1.0);
}
