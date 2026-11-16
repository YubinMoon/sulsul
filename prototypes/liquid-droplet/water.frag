#version 300 es
precision highp float;

// 물방울 레이어만 그립니다. 배경을 입력으로 받지 않고, 같은 시각이면 언제나 같은 RGBA를 냅니다.
uniform vec2 u_resolution;
uniform float u_time;
out vec4 fragColor;

const float PI = 3.14159265359;
const float FLOOR = -2.02;
const float RADIUS = 0.185;
// 등장 동작 길이(초). 이 뒤로는 수면과 연결부를 계산하지 않습니다.
const float INTRO = 1.30;
// 안쪽 무늬가 기준 그림과 같은 자리를 지나는 시각(초)
const float REST = 2.0;
// 본체가 뒤를 가리는 정도. 가운데가 가장 맑고 테두리로 갈수록 덜 비칩니다. 반사와 흰 선은 불투명합니다.
const float CLEAR_CENTER = 0.42;
const float CLEAR_RIM = 0.72;

float sat(float x) { return clamp(x, 0.0, 1.0); }
float sq(float x) { return x * x; }

// 평면에서 a 라디안만큼 돌립니다.
vec2 spin(vec2 v, float a) {
    float c = cos(a);
    float s = sin(a);
    return vec2(c * v.x - s * v.y, s * v.x + c * v.y);
}

// 각도(도)를 -180~180 사이로 접습니다.
float wrapDegrees(float a) { return mod(a + 180.0, 360.0) - 180.0; }

float smoothUnion(float a, float b, float k) {
    float h = max(k - abs(a - b), 0.0) / k;
    return min(a, b) - h * h * k * 0.25;
}

float ellipse(vec2 p, vec2 r) {
    float k0 = length(p / r);
    float k1 = length(p / (r * r));
    return k0 * (k0 - 1.0) / max(k1, 0.00001);
}

// 한 픽셀보다 가는 선은 굵기를 유지하는 대신 옅게 그려 깜박이지 않게 합니다.
float line(float s, float width, float px) {
    float w = max(width, px * 0.75);
    return exp(-sq(s / w)) * (width / w);
}

// 물방울 중심에서 본 방향이 지정한 각도 근처일 때 1에 가까워집니다.
float toward(vec2 dir, float degrees, float spread) {
    float a = radians(degrees);
    return exp(-(1.0 - dot(dir, vec2(cos(a), sin(a)))) * 2.0 / sq(spread));
}

// 다 올라온 높이를 1로 둔 상승 곡선. 천천히 떠오른 속도 그대로 작게 한 번 튕기고 잦아듭니다.
float riseCurve(float t) {
    float u = sat((t - 0.06) / 0.74);
    float after = max(t - 0.80, 0.0);
    return u * u * (2.29 - 1.29 * u) + 0.06 * exp(-7.0 * after) * sin(16.0 * after);
}

// 수면에 붙잡혀 끌려 올라오는 동안 세로로 늘어나고, 끊어지면 되돌아가며 출렁입니다.
vec2 bodyStretch(float t) {
    float after = max(t - 0.62, 0.0);
    float recoil = exp(-7.5 * after) * (cos(20.0 * after) + 0.375 * sin(20.0 * after));
    float pull = 0.30 * smoothstep(0.10, 0.62, t) * recoil;
    return vec2(1.0 - 0.52 * pull, 1.0 + pull);
}

// 떠오르는 길. 다 올라온 자리가 기준 위치입니다.
vec2 risePath(float t) {
    float rise = riseCurve(t);
    return vec2(0.018 * sin(rise * PI), -2.21 + 0.63 * rise);
}

// 자리를 잡은 뒤 무중력에서처럼 느리게 떠다니는 정도. REST 시각에 기준 위치를 지납니다.
vec2 bodyDrift(float t) {
    float tt = t - REST;
    vec2 drift = vec2(0.044 * sin(0.47 * tt) + 0.024 * sin(0.83 * tt) + 0.010 * sin(1.61 * tt),
        0.032 * sin(0.59 * tt) + 0.018 * sin(1.03 * tt) + 0.008 * sin(1.87 * tt));
    return drift * smoothstep(0.85, 2.3, t);
}

vec2 bodyCenter(float t) { return risePath(t) + bodyDrift(t); }

// 윤곽이 원에서 벗어나는 정도. 끊어진 직후의 떨림과, 계속 이어지는 말랑한 출렁임을 더합니다.
// 출렁임은 방향이 천천히 도는 세 가지 모양이 저마다 부풀었다 꺼지는 식이고, REST 시각에는 정확한 원을 지납니다.
float bodyContour(float angle, float t) {
    float tt = t - REST;
    float flutter = 0.022 * sin(angle * 3.0 + t * 10.0) * exp(-max(t - 0.70, 0.0) * 5.6);
    // 떠오르는 동안에는 작게, 끊어진 뒤부터 크게 출렁입니다.
    float alive = mix(0.4, 1.0, smoothstep(0.70, 1.9, t));
    return alive * (0.042 * sin(angle * 2.0 + 0.6 + 0.9 * sin(tt * 0.31)) * sin(tt * 2.05)
        + 0.016 * sin(angle * 3.0 - 0.9 + 0.8 * sin(tt * 0.23)) * sin(tt * 2.90)
        + 0.007 * sin(angle * 4.0 + 1.7 + 0.7 * sin(tt * 0.41)) * sin(tt * 3.70)) + flutter;
}

// 물방울이 구슬처럼 천천히 돌고 기울 때, 안쪽 무늬의 어느 자리가 이 자리에 오는지 구합니다. REST 시각에는 그대로입니다.
vec2 innerPoint(vec2 q, float t) {
    float tt = t - REST;
    float roll = 0.42 * sin(0.29 * tt) + 0.20 * sin(0.53 * tt) + 0.08 * sin(1.01 * tt);
    vec2 tilt = vec2(0.20 * sin(0.41 * tt) + 0.08 * sin(0.89 * tt), 0.15 * sin(0.35 * tt) + 0.07 * sin(0.73 * tt));
    vec3 s = vec3(q, sqrt(max(1.0 - dot(q, q), 0.0)));
    s.xz = spin(s.xz, tilt.x);
    s.yz = spin(s.yz, tilt.y);
    return spin(s.xy, -roll);
}

float scene(vec2 p, float t) {
    vec2 center = bodyCenter(t);
    vec2 stretch = bodyStretch(t);
    vec2 q = (p - center) / (RADIUS * stretch);
    float body = (length(q) - 1.0 - bodyContour(atan(q.y, q.x), t)) * RADIUS * min(stretch.x, stretch.y);
    if (t >= INTRO) return body;

    // 수면과의 연결은 가운데부터 가늘어지고, 끊긴 뒤 되돌아갑니다.
    float height = max(center.y - FLOOR, 0.015);
    float along = clamp((p.y - FLOOR) / height, 0.0, 1.0);
    float thin = smoothstep(0.39, 0.70, t);
    float retract = smoothstep(0.69, 0.91, t);
    float neckRadius = mix(0.105, 0.095, along) * (1.0 - retract)
        - (0.033 + 0.125 * thin) * pow(sin(along * PI), 1.2) - retract * 0.06;
    float offset = 0.012 * sin(along * PI);
    float neck = max(abs(p.x - offset) - neckRadius, max(FLOOR - p.y, p.y - center.y));
    neck = max(neck, -p.y - 2.06);

    float release = smoothstep(0.575, 0.775, t);
    float surfaceRise = smoothstep(0.02, 0.19, t);
    float poolDrop = smoothstep(0.575, 1.15, t);
    float ripple = sin((t - 0.575) * 26.0) * exp(-max(t - 0.575, 0.0) * 9.0) * release;
    float pool = ellipse(p - vec2(0.0, -2.145 + 0.065 * surfaceRise - 0.16 * poolDrop + 0.009 * ripple), vec2(0.41, 0.115));
    float joined = smoothUnion(body, neck, mix(0.09, 0.043, thin));
    return smoothUnion(joined, pool, 0.08);
}

// 흰 바탕 위에서 보이는 색을 정한 불투명도의 레이어 색으로 바꿉니다. 실제 배경과는 무관한 고정된 식입니다.
vec4 layerFromWhite(vec3 seen, float alpha) {
    return vec4(clamp((seen - (1.0 - alpha)) / alpha, 0.0, 1.0), alpha);
}

// 본체의 색과 불투명도. q는 물방울 반지름을 1로 둔 좌표, edge는 가장자리에서 안쪽으로 들어온 깊이입니다.
vec4 shadeBody(vec2 q, float edge, float px, float t) {
    float r = length(q);
    vec2 dir = q / max(r, 0.0001);
    float angle = degrees(atan(q.y, q.x));
    float tt = t - REST;
    // 안쪽 무늬는 본체에만 그리고, 수면과 이어진 부분은 바탕색으로 둡니다.
    float inside = 1.0 - smoothstep(1.02, 1.30, r);

    // 물방울이 출렁이는 만큼 겉에 비친 빛이 테두리를 따라 미끄러집니다(도).
    // A는 왼쪽 위의 큰 반사, B는 아래쪽에 고인 빛, C는 테두리와 오른쪽 위에 비친 빛입니다.
    float slideA = 7.0 * sin(0.83 * tt) + 4.0 * sin(1.31 * tt);
    float slideB = 11.0 * sin(0.57 * tt) + 5.0 * sin(1.13 * tt);
    float slideC = 6.0 * sin(0.71 * tt) + 3.0 * sin(1.49 * tt);
    vec2 qA = spin(q, radians(-slideA));
    vec2 dirA = spin(dir, radians(-slideA));
    float angleA = wrapDegrees(angle - slideA);
    vec2 dirB = spin(dir, radians(-slideB));
    float angleB = wrapDegrees(angle - slideB);
    vec2 qC = spin(q, radians(-slideC));
    vec2 dirC = spin(dir, radians(-slideC));
    float angleC = wrapDegrees(angle - slideC);

    // 안쪽 무늬는 물방울과 함께 돌고 기웁니다. 아래에서 x, y는 그렇게 돌린 안쪽 좌표입니다.
    vec2 qi = innerPoint(q, t);
    float x = qi.x;
    float y = qi.y;
    vec2 diri = qi / max(length(qi), 0.0001);

    // 가운데는 밝고 가장자리로 갈수록 푸른 바탕. 오른쪽 위가 가장 짙습니다.
    vec3 rimTone = mix(vec3(0.53, 0.80, 0.975), vec3(0.415, 0.715, 0.925), toward(dirC, 65.0, 0.9));
    vec3 col = mix(vec3(0.682, 0.886, 0.980), rimTone, smoothstep(0.05, 0.92, r));
    // 왼쪽 위 가운데는 조금 짙고, 오른쪽 위 안쪽은 옅게 밝습니다.
    col = mix(col, vec3(0.45, 0.74, 0.92), 0.35 * exp(-sq((qA.x + 0.33) / 0.26) - sq((qA.y - 0.42) / 0.20)));
    col = mix(col, vec3(0.76, 0.905, 0.995), 0.45 * exp(-sq((qC.x - 0.42) / 0.30) - sq((qC.y - 0.50) / 0.18))
        * (1.0 - smoothstep(0.70, 0.88, r)));

    // 비단처럼 접힌 띠의 경계선들. 천천히 흔들리며 기준 그림의 자리를 오갑니다.
    float shift = 0.070 * sin(0.52 * tt);
    float lift = 0.045 * sin(0.71 * tt);
    float swell = 0.028 * sin(0.63 * tt);
    float wave = 0.022 * sin(2.4 * x - 0.95 * tt) + swell * sin(3.3 * x - 1.7 * tt + 0.5);
    float waveSlope = 0.0528 * cos(2.4 * x - 0.95 * tt) + 3.3 * swell * cos(3.3 * x - 1.7 * tt + 0.5);
    float tanhT = tanh((x - 0.02 - shift) / 0.29);
    float lower = -0.09 + lift + 0.30 * tanhT - 1.8 * sq(max(x - 0.60, 0.0)) - 8.0 * sq(max(x - 0.74, 0.0)) + wave;
    float lowerSlope = 1.034 * (1.0 - sq(tanhT)) - 3.6 * max(x - 0.60, 0.0) - 16.0 * max(x - 0.74, 0.0) + waveSlope;
    float tanhS = tanh((x - 0.2526 - shift) / 0.15);
    float upper = 0.205 + lift + 0.245 * tanhS - 4.5 * sq(max(-0.58 - x, 0.0)) - 1.5 * sq(max(x - 0.60, 0.0)) + wave;
    float upperSlope = 1.633 * (1.0 - sq(tanhS)) + 9.0 * max(-0.58 - x, 0.0) - 3.0 * max(x - 0.60, 0.0) + waveSlope;
    float floorLine = -0.40 - 0.21 * smoothstep(-0.45, 0.30, x) + 0.9 * sq(max(x - 0.35, 0.0))
        + 0.018 * sin(2.1 * x - 0.80 * tt) - 0.6 * lift;
    float tanhD = tanh((x + 0.35 - 0.6 * shift) / 0.22);
    float deepLine = -0.62 - 0.26 * tanhD + 0.014 * sin(2.6 * x + 0.66 * tt);

    float sLower = (y - lower) / sqrt(1.0 + sq(lowerSlope));
    float sUpper = (y - upper) / sqrt(1.0 + sq(upperSlope));
    float sFloor = y - floorLine;
    float sDeep = (y - deepLine) / sqrt(1.0 + sq(1.182 * (1.0 - sq(tanhD))));

    // 왼쪽 아래의 짙은 물빛. 오른쪽으로 갈수록 청록빛이 됩니다.
    float deep = (1.0 - smoothstep(-0.012, 0.004, sDeep)) * inside;
    vec3 deepColor = mix(vec3(0.305, 0.645, 0.925), vec3(0.335, 0.755, 0.958), smoothstep(-0.55, -0.25, x));
    deepColor = mix(deepColor, vec3(0.43, 0.85, 0.985), smoothstep(-0.20, 0.30, x));
    // 경계 바로 아래는 옅고, 안으로 들어갈수록 짙어집니다.
    deepColor = mix(vec3(0.52, 0.76, 0.975), deepColor, smoothstep(0.0, 0.11, -sDeep));
    col = mix(col, deepColor, deep);

    // 짙은 물빛 위의 밝은 띠
    float band = smoothstep(-0.004, 0.012, sDeep) * (1.0 - smoothstep(-0.012, 0.012, sFloor)) * inside;
    vec3 bandColor = mix(vec3(0.46, 0.86, 0.985), vec3(0.72, 0.97, 1.0), smoothstep(-0.10, 0.50, x));
    bandColor = mix(vec3(0.74, 0.955, 0.996), bandColor, smoothstep(-0.60, -0.32, x));
    col = mix(col, bandColor, band);

    // 오른쪽 아래로 갈수록 밝아지는 청록빛 웅덩이와 그 안의 옅은 겹
    float pond = smoothstep(-0.006, 0.012, sFloor) * (1.0 - smoothstep(-0.010, 0.004, sLower)) * inside;
    float pondLight = smoothstep(0.0, 1.0, dot(qi - vec2(0.20, -0.05), vec2(0.34, -0.94)) / 0.52);
    vec3 pondColor = mix(vec3(0.385, 0.775, 0.950), vec3(0.62, 0.955, 0.995), pondLight);
    pondColor = mix(vec3(0.80, 0.97, 0.995), pondColor, smoothstep(-0.50, -0.12, x));
    float ply = smoothstep(-0.05, 0.15, x) * (1.0 - smoothstep(0.76, 0.92, x));
    float sPly = y - lower + 0.17 * smoothstep(-0.05, 0.40, x);
    vec3 plyColor = mix(vec3(0.50, 0.83, 0.975), vec3(0.62, 0.84, 0.99), smoothstep(0.35, 0.75, x));
    pondColor = mix(pondColor, plyColor, 0.80 * ply * smoothstep(-0.012, 0.0, sPly) * (1.0 - pondLight));
    col = mix(col, pondColor, pond);
    col = mix(col, vec3(0.86, 0.98, 1.0), 0.40 * ply * pond * line(sPly, 0.008, px));

    // 두 경계선 사이의 얇은 막. 왼쪽은 아래 가장자리가 밝고, 꼬이는 자리에서 하나의 선으로 모입니다.
    float gap = max(upper - lower, 0.0);
    float veilEdge = mix(0.014, 0.075, smoothstep(0.28, 0.50, x));
    float veil = smoothstep(-0.004, 0.010, sLower) * (1.0 - smoothstep(-veilEdge, 0.010, sUpper)) * inside;
    float side = smoothstep(0.05, 0.30, x);
    float veilLeft = (0.16 + 0.55 * exp(-max(y - lower, 0.0) / 0.09) + 0.18 * exp(-max(upper - y, 0.0) / 0.035))
        * (1.0 - 0.55 * smoothstep(-0.40, 0.0, x)) + 0.25 * smoothstep(-0.30, -0.80, x);
    float fiber = 0.11 * pow(0.5 + 0.5 * cos(sat((y - lower) / max(gap, 0.001)) * 15.7), 6.0) * (1.0 - side);
    // 오른쪽은 부채꼴로 넓게 퍼지고, 아래 경계선 바로 위가 가장 밝습니다.
    float veilRight = 0.15 + 0.25 * smoothstep(0.20, 0.50, x) + 0.22 * exp(-sq(max(y - lower - 0.05, 0.0) / 0.075));
    float veilAmount = veil * sat(mix(veilLeft + fiber, veilRight, side)) * smoothstep(0.0, 0.03, gap);
    // 위쪽 경계선 너머로도 옅게 번집니다.
    veilAmount += 0.20 * exp(-max(sUpper, 0.0) / 0.07) * smoothstep(0.0, 0.02, sUpper) * smoothstep(0.15, 0.40, x)
        * (1.0 - smoothstep(0.80, 0.95, r)) * inside;
    // 옅은 곳은 푸른 기가 남고, 겹칠수록 희어집니다.
    veilAmount = sat(veilAmount) * (1.0 - smoothstep(0.84, 0.93, r));
    col = mix(col, mix(vec3(0.74, 0.90, 0.99), vec3(0.93, 0.99, 1.0), smoothstep(0.25, 0.80, veilAmount)), veilAmount);

    // 경계선 자체의 밝은 선
    float lowerLine = line(sLower, 0.011, px) * mix(0.50, 1.0, smoothstep(-0.40, 0.0, x))
        * (1.0 - 0.25 * smoothstep(0.30, 0.60, x)) * (1.0 - smoothstep(0.86, 0.94, x))
        + 0.80 * exp(-sq(sLower / 0.030)) * smoothstep(0.62, 0.74, x) * (1.0 - smoothstep(0.82, 0.92, x));
    // 위쪽 경계선은 꼬이는 자리에서 가파르게 올라가는 동안만 또렷하고, 그 밖에서는 부드러운 빛입니다.
    float upperLine = line(sUpper, 0.009, px) * smoothstep(-0.05, 0.10, x) * (1.0 - smoothstep(0.16, 0.32, x))
        + 0.32 * line(sUpper, 0.022, px) * smoothstep(-0.88, -0.70, x) * (1.0 - smoothstep(0.0, 0.20, x))
        + 0.22 * line(sUpper, 0.030, px) * smoothstep(0.10, 0.25, x) * (1.0 - smoothstep(0.36, 0.56, x));
    float twist = exp(-sq((x - 0.14 - shift) / 0.20)) * (line(sLower, 0.022, px) + line(sUpper, 0.022, px)) * 0.30;
    float floorGlow = line(sFloor, 0.010, px) * 0.34 * smoothstep(-0.45, -0.20, x) * (1.0 - smoothstep(0.30, 0.55, x));
    float deepGlow = (line(sDeep, 0.010, px) * 0.60 + 0.30 * exp(-max(sDeep, 0.0) / 0.05) * step(0.0, sDeep))
        * (1.0 - smoothstep(-0.10, 0.25, x));
    // 선 둘레로 번지는 은은한 빛
    float sheen = 0.20 * exp(-abs(sLower) / 0.04) * smoothstep(-0.55, -0.25, x) * (1.0 - smoothstep(0.45, 0.80, x));
    float lines = sat(lowerLine + upperLine + twist + floorGlow + deepGlow + sheen) * inside;
    col = mix(col, vec3(1.0), lines);

    // 왼쪽 테두리에 닿는 하얀 접힘과 그 끝의 반짝임
    float fold = exp(-sq((y - lower - 0.070) / 0.065)) * smoothstep(-0.28, -0.60, x) * inside;
    col = mix(col, vec3(0.92, 0.985, 1.0), 0.95 * fold);
    col = mix(col, vec3(1.0, 0.925, 0.995), 0.60 * toward(diri, 203.0, 0.16) * exp(-sq((r - 0.89) / 0.10)) * inside);
    float spark = toward(diri, 201.5, 0.085) * exp(-sq((r - 0.875) / 0.095)) * inside;
    col = mix(col, vec3(1.0), spark);

    // 오른쪽 아래 안쪽에 고이는 흰 빛과 바닥을 따라 도는 청록빛
    col = mix(col, vec3(0.40, 0.86, 0.99), 0.75 * exp(-sq((r - 0.885) / 0.035)) * toward(dirB, 268.0, 0.50) * inside);
    col = mix(col, vec3(0.60, 0.965, 0.997), 0.70 * exp(-sq((r - 0.87) / 0.05)) * toward(dirB, 332.0, 0.30) * inside);
    // 왼쪽 아래 테두리 안쪽을 따라 도는 밝은 청록빛 줄
    col = mix(col, vec3(0.80, 0.99, 0.997), 0.95 * sat(1.3 * toward(dirB, 238.0, 0.50))
        * smoothstep(-146.0, -136.0, angleB) * exp(-sq((r - 0.92) / 0.020)) * inside);
    col = mix(col, vec3(0.85, 0.985, 1.0), 0.45 * exp(-sq((r - 0.78) / 0.09)) * toward(dirB, 282.0, 0.38) * inside);
    float pooled = exp(-sq(max(0.86 - r, 0.0) / 0.20) - sq(max(r - 0.86, 0.0) / 0.10)) * toward(dirB, 300.0, 0.36);
    col = mix(col, vec3(0.95, 0.995, 1.0), sat(1.05 * pooled) * inside);

    // 오른쪽 위 안쪽을 따라 도는 청록빛 호와 그 둘레의 옅은 빛
    col = mix(col, vec3(0.385, 0.685, 0.915), 0.22 * exp(-sq((r - 0.81) / 0.055)) * toward(dirC, 62.0, 0.50) * inside);
    col = mix(col, vec3(0.60, 0.81, 0.99),
        0.50 * toward(dirC, 32.0, 0.25) * exp(-sq(max(abs(r - 0.88) - 0.07, 0.0) / 0.03)) * inside);
    float arcSpan = smoothstep(4.0, 16.0, angleC) * (1.0 - smoothstep(58.0, 78.0, angleC));
    float arcR = 0.943 - 0.00125 * angleC;
    float arcWidth = mix(0.024, 0.013, smoothstep(45.0, 62.0, angleC));
    // 호 바깥의 짙은 띠와, 아래로 갈수록 희어지는 호
    col = mix(col, vec3(0.43, 0.715, 0.945), 0.85 * exp(-sq((r - arcR - 0.048) / 0.022))
        * smoothstep(26.0, 40.0, angleC) * (1.0 - smoothstep(66.0, 84.0, angleC)) * inside);
    col = mix(col, mix(vec3(0.50, 0.94, 1.0), vec3(0.97, 1.0, 1.0), smoothstep(44.0, 26.0, angleC)),
        0.92 * exp(-sq(sq((r - arcR) / arcWidth))) * arcSpan * inside);
    // 오른쪽 테두리 안쪽의 옅은 띠
    col = mix(col, vec3(0.75, 0.90, 1.0), 0.30 * exp(-sq((r - 0.825) / 0.035)) * toward(dirC, 2.0, 0.25) * inside);

    // 테두리 가까이에 비치는 연보랏빛과 분홍빛
    float violetLeft = exp(-sq(max(abs(r - 0.85) - 0.04, 0.0) / 0.04)) * toward(diri, 211.0, 0.16) * inside;
    col = mix(col, vec3(0.70, 0.83, 0.995), 0.95 * violetLeft * (1.0 - spark));
    float violetRight = exp(-sq(max(0.93 - r, 0.0) / 0.10) - sq(max(r - 0.93, 0.0) / 0.05)) * toward(dirC, 3.0, 0.22) * inside;
    col = mix(col, vec3(0.66, 0.74, 0.985), 0.60 * violetRight);
    col = mix(col, vec3(1.0, 0.93, 1.0), 0.65 * toward(dirC, 17.5, 0.13) * exp(-sq((r - 0.89) / 0.085)) * inside);
    float blush = toward(dirC, 16.0, 0.11) * exp(-sq((r - 0.912) / 0.035)) * inside;
    col = mix(col, vec3(1.0), 0.95 * blush);

    // 유리 껍질: 가장자리 안쪽의 옅은 띠와 흰 띠, 그 사이의 가는 선, 바깥 윤곽선
    float lightAmt = 0.15 + 0.60 * toward(dirC, 116.0, 0.30) + 0.75 * toward(dirC, 288.0, 0.35) + 0.20 * toward(dirC, 75.0, 0.50);
    col = mix(col, vec3(0.88, 0.975, 1.0), sat(lightAmt * (1.0 - smoothstep(0.030, 0.105, edge))));
    // 위쪽에는 안쪽으로 한 줄이 더 비칩니다.
    col = mix(col, vec3(0.90, 0.97, 1.0), 0.60 * line(edge - 0.060 + 0.015 * smoothstep(100.0, 135.0, angleC), 0.008, px)
        * sat(toward(dirC, 85.0, 0.45) + toward(dirC, 142.0, 0.35)));
    col = mix(col, vec3(0.55, 0.78, 0.96), (0.22 + 0.15 * toward(dirC, 95.0, 0.45)) * line(edge - 0.038, 0.007, px));
    // 가장자리 바로 안쪽의 흰 줄. 오른쪽 아래에서는 거의 사라지고, 조금 더 안쪽에 따로 비칩니다.
    float whiteAmt = 0.88 - 0.78 * toward(dirC, 345.0, 0.35) - 0.50 * toward(dirC, 12.0, 0.15);
    float narrow = toward(dirC, 170.0, 0.50);
    col = mix(col, vec3(1.0), sat(whiteAmt) * exp(-sq((edge - 0.016 + 0.006 * narrow) / (0.013 - 0.004 * narrow))));
    col = mix(col, vec3(0.92, 0.985, 1.0), 0.80 * line(edge - 0.030, 0.009, px) * toward(dirC, 352.0, 0.14));
    // 오른쪽 테두리에 비치는 푸른 보랏빛 줄
    col = mix(col, vec3(0.53, 0.68, 0.985), 0.85 * exp(-sq((edge - 0.038) / 0.016)) * toward(dirC, 9.0, 0.20));
    float outline = line(edge - 0.002, 0.0050, px) * (0.92 - 0.85 * toward(dirC, 70.0, 1.1));
    col = mix(col, vec3(0.43, 0.70, 0.945), outline);

    // 왼쪽 위의 큰 반사. 테두리를 따라 휘어진 둥근 모양입니다.
    // 바깥쪽 경계는 또렷하고, 비스듬한 직선에 가까운 안쪽 경계는 부드럽게 풀립니다.
    // 물방울이 출렁이면 도톰하고 짧아졌다가 얇고 길어집니다.
    float puff = 0.55 * sin(0.93 * tt) + 0.35 * sin(1.57 * tt);
    float outer = 0.927 - 0.0024 * max(abs(angleA - 121.0) - 3.0, 0.0) - 0.00012 * sq(max(angleA - 136.0, 0.0));
    float along = dot(qA - vec2(-0.353, 0.640), vec2(0.834, 0.552)) * (1.0 + 0.14 * puff);
    float chord = dot(qA, vec2(-0.552, 0.834)) - 0.702 + 0.040 * puff + 0.012 * max(1.0 - sq(along / 0.25), 0.0)
        - 0.034 * sq(max(along - 0.18, 0.0) / 0.177) - 0.019 * sq(max(-along - 0.27, 0.0) / 0.12);
    float glint = (1.0 - smoothstep(outer - 0.006, outer + 0.008, r)) * (1.0 - exp(-max(chord + 0.044, 0.0) / 0.0455))
        * step(0.0, qA.y) * inside;
    // 큰 반사와 작은 반사 사이의 짙은 그늘
    col = mix(col, vec3(0.41, 0.72, 0.91), 0.70 * toward(dirA, 151.0, 0.13) * exp(-sq((r - 0.895) / 0.04)) * inside);
    // 왼쪽 테두리 가까이의 작은 반사
    float glintSmall = (1.0 - smoothstep(0.50, 1.0, sq(wrapDegrees(angleA - 169.5) / 7.5) + sq((r - 0.892) / 0.032)))
        * step(0.0, -qA.x) * inside;
    float shine = sat(glint + glintSmall);
    col = mix(col, vec3(1.0), shine);

    // 안쪽에 떠 있는 아주 작은 기포
    for (int i = 0; i < 2; i++) {
        vec2 at = i == 0 ? vec2(0.210, 0.365) : vec2(0.555, -0.050);
        at += 0.012 * vec2(sin(tt * 0.9 + float(i) * 2.1), cos(tt * 0.7 + float(i) * 1.3));
        vec2 s = (qi - at) / 0.015;
        float bubble = (1.0 - smoothstep(1.0 - px / 0.015, 1.0 + px / 0.015, length(s))) * inside;
        col = mix(col, mix(vec3(1.0), vec3(0.36, 0.66, 0.93), smoothstep(-0.2, 0.7, dot(s, vec2(-0.707, 0.707)))), bubble);
    }

    // 본체는 뒤가 비치는 맑은 물입니다. 가운데가 가장 맑고, 테두리로 갈수록 그리고 색이 짙을수록 덜 비칩니다.
    // 반사와 흰 선은 불투명합니다. 어느 쪽이든 흰 바탕 위에서 보이는 색은 같습니다.
    float solid = sat(max(max(shine, max(spark, blush)), max(outline, 0.8 * lines)));
    float clear = mix(CLEAR_CENTER, CLEAR_RIM, smoothstep(0.50, 1.0, r));
    float dense = 1.12 * (1.0 - min(col.r, min(col.g, col.b)));
    return layerFromWhite(col, mix(max(clear, dense), 1.0, solid));
}

// 곁에 떠 있는 작은 물방울. 가운데는 맑고 테두리가 짙습니다.
vec4 shadeBead(vec2 s, float px) {
    float r = length(s);
    vec3 col = mix(vec3(1.0), vec3(0.45, 0.745, 0.955), smoothstep(-0.55, 0.85, s.y));
    float ring = smoothstep(0.55, 0.96, r);
    col = mix(col, vec3(0.27, 0.60, 0.89), 0.92 * ring * (0.75 + 0.25 * abs(s.y)));
    vec2 glintAt = s - vec2(-0.36, 0.42);
    float glint = exp(-dot(glintAt, glintAt) / 0.045);
    col = mix(col, vec3(1.0), glint);
    float coverage = 1.0 - smoothstep(1.0 - px, 1.0 + px, r);
    vec4 layer = layerFromWhite(col, mix(0.55, 1.0, max(ring, glint)));
    return vec4(layer.rgb, layer.a * coverage);
}

void main() {
    vec2 p = (gl_FragCoord.xy - 0.5 * u_resolution) / u_resolution.y * 4.0;
    float pixel = 4.0 / u_resolution.y;
    float t = u_time;
    vec2 center = bodyCenter(t);
    // 물방울에서 먼 곳은 계산하지 않습니다.
    if (abs(p.x - center.x) > 0.60 || p.y > center.y + RADIUS * 2.2) {
        fragColor = vec4(0.0);
        return;
    }
    float distance = scene(p, t);
    float px = pixel / RADIUS;

    // 테두리 밖으로 번지는 푸른 빛. 아래쪽이 더 짙습니다.
    float outside = max(distance, 0.0) / RADIUS;
    float below = smoothstep(-0.9, 0.9, -normalize(p - center + vec2(0.0, 0.00001)).y);
    float glow = mix(0.075, 0.22, below) * exp(-pow(outside / 0.19, 1.5)) * (1.0 - smoothstep(0.45, 0.62, outside));
    vec3 color = vec3(0.25, 0.70, 0.96) * glow;
    float alpha = glow;

    if (distance < pixel) {
        float coverage = 1.0 - smoothstep(-0.7 * pixel, 0.7 * pixel, distance);
        vec2 q = (p - center) / (RADIUS * bodyStretch(t));
        // 윤곽이 출렁여도 무늬가 가장자리를 따라가도록 반지름을 맞춥니다.
        q /= 1.0 + bodyContour(atan(q.y, q.x), t);
        vec4 body = shadeBody(q, -distance / RADIUS, px, t);
        float cover = coverage * body.a;
        color = body.rgb * cover + vec3(0.25, 0.70, 0.96) * glow * (1.0 - coverage);
        alpha = cover + glow * (1.0 - coverage);
    }

    // 끊어진 뒤 본체 곁으로 떨어져 나와 조금 늦게 따라 움직이는 작은 물방울 둘
    float born = smoothstep(0.66, 1.05, t);
    if (born > 0.0) {
        vec2 anchor = risePath(t - 0.12) + bodyDrift(t);
        float tt = t - REST;
        for (int i = 0; i < 2; i++) {
            vec2 rest = i == 0 ? vec2(0.944, 0.963) : vec2(0.867, 0.761);
            float size = (i == 0 ? 0.059 : 0.028) * born;
            // 본체와 함께 떠다니면서 저마다 조금씩 떠돕니다. REST 시각에는 기준 자리에 있습니다.
            vec2 bob = i == 0
                ? vec2(0.085 * sin(0.61 * tt) + 0.035 * sin(1.43 * tt), 0.070 * sin(0.79 * tt) + 0.035 * sin(1.21 * tt))
                : vec2(0.050 * sin(0.87 * tt) + 0.025 * sin(1.69 * tt), 0.045 * sin(0.69 * tt) + 0.025 * sin(1.37 * tt));
            vec2 at = anchor + (rest * mix(0.62, 1.0, born) + bob) * RADIUS;
            vec2 s = (p - at) / (size * RADIUS);
            if (dot(s, s) < 1.5) {
                vec4 bead = shadeBead(s, px / size);
                color = color * (1.0 - bead.a) + bead.rgb * bead.a;
                alpha = alpha * (1.0 - bead.a) + bead.a;
            }
        }
    }
    fragColor = vec4(color / max(alpha, 0.00001), alpha);
}
