"""물방울 오버레이의 움직임에 맞춘 효과음을 만듭니다.

표준 라이브러리만 씁니다. 저장소 루트에서 실행하면 이 폴더에 WAV 파일 네 개를 다시 씁니다.

    python prototypes/liquid-droplet/sounds/make_sounds.py

- start.wav: 녹음 시작. 물방울이 수면에서 떠오르는 "블룹"과 떨어져 나오는 "똑".
- pop.wav: 녹음 끝. 막이 찢어지는 "뽁"과 흩어지는 작은 물방울들의 "또르륵".
- done.wav: 받아쓰기 완료. 고리가 빨려 들어가는 소리, 합쳐지는 "뽀", 사라지는 "뿅".
- fail.wav: 실패. 바람 빠지듯 내려가는 두 물소리.

각 소리의 시각은 water.frag의 동작 시각에 맞춥니다. 앱(`crates/sulsul-app/src/sound.rs`)과 미리보기가
이 파일들을 그대로 쓰므로, 고친 뒤에는 이 스크립트를 다시 실행하고 `cargo test -p sulsul-app sound`로 확인합니다.
"""

import math
import random
import struct
import wave
from pathlib import Path

RATE = 44_100
HERE = Path(__file__).resolve().parent

# water.frag의 시각(초)
POP_RUPTURE = 0.07  # 녹음이 끝난 뒤 막이 찢어지는 시각
DONE_MERGE = 0.28  # 후처리가 끝난 뒤 고리의 물방울들이 하나로 합쳐지는 시각
DONE_POP = 0.40  # 합쳐진 물방울이 뿅 하고 사라지는 시각
# 시작음은 녹음과 함께 나므로 마이크에 크게 들어가지 않게 조금 더 작게 둡니다.
START_PEAK = 0.17
PEAK = 0.2


def blank(seconds):
    return [0.0] * int(seconds * RATE)


def tone(out, start, f0, f1, glide, length, decay, amp, attack=0.003, overtone=0.12):
    """사인파 하나를 더합니다. 높이는 glide초 동안 f0에서 f1로 미끄러지고(지수 곡선), 소리는 decay 빠르기로 줄어듭니다."""
    first = int(start * RATE)
    count = int(length * RATE)
    release = 0.012
    phase = 0.0
    for n in range(count):
        i = first + n
        if i >= len(out):
            break
        t = n / RATE
        frequency = f0 * (f1 / f0) ** min(t / glide, 1.0)
        phase += 2.0 * math.pi * frequency / RATE
        envelope = min(t / attack, 1.0) * math.exp(-decay * t) * min((length - t) / release, 1.0)
        out[i] += amp * envelope * (math.sin(phase) + overtone * math.sin(2.0 * phase))


def swirl(out, start, length, f0, f1, amp, seed):
    """빨려 들어가며 점점 빨리 도는 소리. 높이가 오르고, 떨림(도는 빠르기)이 빨라지며 커집니다."""
    rng = random.Random(seed)
    first = int(start * RATE)
    count = int(length * RATE)
    phase = 0.0
    wobble = 0.0
    hiss = 0.0
    for n in range(count):
        t = n / RATE
        u = t / length
        frequency = f0 * (f1 / f0) ** (u * u)
        phase += 2.0 * math.pi * frequency / RATE
        wobble += 2.0 * math.pi * (9.0 + 26.0 * u * u) / RATE
        # 물이 쓸리는 듯한 아주 옅은 바람 소리
        hiss = 0.92 * hiss + 0.08 * (rng.random() * 2.0 - 1.0)
        level = amp * u ** 1.6 * min((length - t) / 0.02, 1.0)
        out[first + n] += level * ((0.65 + 0.35 * math.sin(wobble)) * math.sin(phase) + 0.5 * hiss)


def burst(out, start, length, center, q, fall, amp, seed):
    """막이 찢어지는 짧은 소리. 흰 소음을 대역 통과 필터로 거르고 아주 빨리 줄입니다."""
    rng = random.Random(seed)
    w = 2.0 * math.pi * center / RATE
    alpha = math.sin(w) / (2.0 * q)
    b0, b2 = alpha, -alpha
    a0, a1, a2 = 1.0 + alpha, -2.0 * math.cos(w), 1.0 - alpha
    x1 = x2 = y1 = y2 = 0.0
    first = int(start * RATE)
    for n in range(int(length * RATE)):
        t = n / RATE
        x = rng.random() * 2.0 - 1.0
        y = (b0 * x + b2 * x2 - a1 * y1 - a2 * y2) / a0
        x2, x1, y2, y1 = x1, x, y1, y
        envelope = min(t / 0.0008, 1.0) * math.exp(-t / fall)
        out[first + n] += amp * envelope * y


def drops(out, start, end, count, low, high, amp, seed):
    """흩어지는 작은 물방울들의 높고 짧은 소리. 뒤로 갈수록 작아집니다."""
    rng = random.Random(seed)
    for k in range(count):
        at = start + (end - start) * (k + rng.random() * 0.8) / count
        pitch = low + (high - low) * rng.random()
        tone(out, at, pitch, pitch * 1.25, 0.02, 0.05, 80.0, amp * (1.0 - 0.7 * k / count) * (0.6 + 0.4 * rng.random()),
             attack=0.0015, overtone=0.0)


def start_sound():
    out = blank(0.36)
    # 수면이 부풀어 물방울이 떠오르는 "블룹"
    tone(out, 0.0, 230.0, 640.0, 0.12, 0.22, 15.0, 1.0)
    # 수면에서 떨어져 나오는 "똑" (등장 약 0.21초)
    tone(out, 0.205, 820.0, 1380.0, 0.035, 0.13, 36.0, 0.42, overtone=0.05)
    return out, START_PEAK


def pop_sound():
    out = blank(0.42)
    at = POP_RUPTURE - 0.025  # 재생이 조금 늦게 시작되는 만큼 앞당깁니다.
    # 막이 찢어지는 "뽁": 짧은 소음과 아래로 떨어지는 높은 음, 그 밑의 작은 울림
    burst(out, at, 0.05, 2600.0, 0.9, 0.006, 1.6, seed=7)
    tone(out, at, 1500.0, 640.0, 0.03, 0.10, 52.0, 0.75, attack=0.001)
    tone(out, at, 300.0, 170.0, 0.05, 0.12, 32.0, 0.35, attack=0.002, overtone=0.0)
    # 사방으로 흩어지는 작은 물방울들
    drops(out, at + 0.035, 0.30, 7, 2600.0, 4800.0, 0.22, seed=11)
    return out, PEAK


def done_sound():
    out = blank(0.66)
    # 고리가 점점 빨리 돌며 가운데로 빨려 들어갑니다.
    swirl(out, 0.0, DONE_MERGE, 420.0, 980.0, 0.30, seed=3)
    # 하나로 합쳐지는 "뽀"
    tone(out, DONE_MERGE - 0.01, 380.0, 760.0, 0.05, 0.11, 30.0, 0.5)
    # 사라지는 "뿅": 빠르게 올라가는 음
    tone(out, DONE_POP - 0.01, 520.0, 1900.0, 0.075, 0.21, 17.0, 1.0, overtone=0.18)
    # 남는 작은 반짝임
    drops(out, DONE_POP + 0.03, 0.57, 4, 3600.0, 6000.0, 0.13, seed=5)
    return out, PEAK


def fail_sound():
    out = blank(0.44)
    # 바람 빠지듯 내려가는 두 물소리
    tone(out, 0.0, 560.0, 340.0, 0.12, 0.17, 10.0, 1.0)
    tone(out, 0.17, 400.0, 210.0, 0.18, 0.26, 9.0, 0.9)
    return out, START_PEAK


def write(name, samples, peak):
    top = max(abs(s) for s in samples)
    scale = peak / top
    frames = b"".join(struct.pack("<h", round(max(-1.0, min(1.0, s * scale)) * 32767)) for s in samples)
    with wave.open(str(HERE / name), "wb") as file:
        file.setnchannels(1)
        file.setsampwidth(2)
        file.setframerate(RATE)
        file.writeframes(frames)
    print(f"{name}: {len(samples) / RATE:.2f}초")


if __name__ == "__main__":
    write("start.wav", *start_sound())
    write("pop.wav", *pop_sound())
    write("done.wav", *done_sound())
    write("fail.wav", *fail_sound())
