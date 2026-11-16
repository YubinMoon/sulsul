#version 300 es
precision highp float;

uniform vec2 u_resolution;
uniform float u_time;
uniform float u_light;
out vec4 fragColor;

const float PI = 3.14159265359;
const float FLOOR = -2.02;
const float RADIUS = 0.185;

float sat(float x) { return clamp(x, 0.0, 1.0); }
float sq(float x) { return x * x; }

float smoothUnion(float a, float b, float k) {
    float h = max(k - abs(a - b), 0.0) / k;
    return min(a, b) - h * h * k * 0.25;
}

float ellipse(vec2 p, vec2 r) {
    float k0 = length(p / r);
    float k1 = length(p / (r * r));
    return k0 * (k0 - 1.0) / max(k1, 0.00001);
}

vec2 bodyCenter(float time) {
    float rise = smoothstep(0.12, 1.65, time);
    float after = max(0.0, time - 1.30);
    float settle = 0.045 * sin(after * 10.5) * exp(-after * 5.0);
    return vec2(0.018 * sin(rise * PI), -2.21 + 1.26 * rise + settle);
}

float scene(vec2 p, float time) {
    vec2 center = bodyCenter(time);
    float release = smoothstep(1.15, 1.55, time);
    float tension = sin(sat((time - 0.25) / 1.05) * PI);
    float after = max(time - 1.15, 0.0);
    float recoil = sin(after * 17.0) * exp(-after * 3.8) * release;
    vec2 stretch = vec2(1.0 - 0.12 * tension + 0.22 * recoil, 1.0 + 0.22 * tension - 0.22 * recoil);
    vec2 q = (p - center) / (RADIUS * stretch);
    float angle = atan(q.y, q.x);
    // 작은 윤곽의 떨림과 가로세로 늘어남으로 말랑함을 표현합니다.
    float flutter = 0.028 * sin(angle * 3.0 + time * 5.0) * exp(-max(time - 1.4, 0.0) * 2.8);
    float contour = 0.014 * sin(angle * 3.0 + 0.6) + flutter;
    float body = (length(q) - 1.0 - contour) * RADIUS * min(stretch.x, stretch.y);

    // 수면과의 연결은 가운데부터 가늘어지고, 끊긴 뒤 되돌아갑니다.
    float height = max(center.y - FLOOR, 0.015);
    float along = clamp((p.y - FLOOR) / height, 0.0, 1.0);
    float thin = smoothstep(0.78, 1.40, time);
    float retract = smoothstep(1.38, 1.82, time);
    float neckRadius = mix(0.105, 0.095, along) * (1.0 - retract)
        - (0.033 + 0.125 * thin) * pow(sin(along * PI), 1.2) - retract * 0.06;
    float offset = 0.012 * sin(along * PI);
    float neck = max(abs(p.x - offset) - neckRadius, max(FLOOR - p.y, p.y - center.y));
    neck = max(neck, -p.y - 2.06);

    float surfaceRise = smoothstep(0.04, 0.38, time);
    float poolDrop = smoothstep(1.15, 2.30, time);
    float ripple = sin((time - 1.15) * 13.0) * exp(-max(time - 1.15, 0.0) * 4.5) * release;
    float pool = ellipse(p - vec2(0.0, -2.145 + 0.065 * surfaceRise - 0.05 * poolDrop + 0.009 * ripple), vec2(0.41, 0.115));
    float joined = smoothUnion(body, neck, mix(0.09, 0.043, thin));
    return smoothUnion(joined, pool, 0.08);
}

vec3 backdrop(vec2 p) {
    vec2 cloudP = (p - vec2(1.3, 0.9)) * vec2(0.45, 0.7);
    float cloud = exp(-dot(cloudP, cloudP));
    vec3 dark = mix(vec3(0.017, 0.022, 0.023), vec3(0.11, 0.135, 0.123), cloud);
    vec3 light = mix(vec3(0.87, 0.86, 0.82), vec3(0.57, 0.66, 0.65), cloud * 0.57);
    return mix(dark, light, u_light);
}

void main() {
    vec2 p = (gl_FragCoord.xy - 0.5 * u_resolution) / u_resolution.y * 4.0;
    vec3 color = backdrop(p);
    float pixelWidth = 4.0 / u_resolution.y;
    float distance = scene(p, u_time * 2.0);
    float coverage = 1.0 - smoothstep(-pixelWidth * 0.65, pixelWidth * 0.65, distance);

    // 광원의 반사나 구면 음영 없이 얇은 막과 한 줄의 윤곽으로 그립니다.
    float wash = sat((p.y - bodyCenter(u_time * 2.0).y) / (RADIUS * 2.0) + 0.5);
    vec3 tint = mix(vec3(0.40, 0.58, 0.53), vec3(0.86, 0.91, 0.88), wash);
    vec3 film = mix(color, tint, mix(0.15, 0.45, u_light));
    color = mix(color, film, coverage);
    float line = exp(-sq(distance / (pixelWidth * 0.70)));
    vec3 ink = mix(vec3(0.58, 0.71, 0.65), vec3(0.23, 0.34, 0.29), u_light);
    color = mix(color, ink, line * mix(0.33, 0.42, u_light));

    color = max(color, vec3(0.0));
    color = color / (1.0 + color);
    color = pow(color, vec3(1.0 / 2.2));
    fragColor = vec4(color, 1.0);
}
