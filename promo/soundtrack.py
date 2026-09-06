"""Original 128 BPM electronic score, arranged to the launch film's scene cuts.

Requires NumPy. All instruments are synthesized; no samples or external music.
"""
from pathlib import Path
import wave
import numpy as np

RATE = 48000
DURATION = 36
BPM = 128
BEAT = 60 / BPM
DROP = 3.8
RNG = np.random.default_rng(128)
SIZE = RATE * DURATION
music = np.zeros((SIZE, 2), dtype=np.float64)
drums = np.zeros_like(music)
accents = np.zeros_like(music)
kicks = []


def note(midi):
    return 440 * 2 ** ((midi - 69) / 12)


def clock(duration):
    return np.arange(int(duration * RATE)) / RATE


def highpass(signal, pole=0.9):
    # A short moving-average subtraction keeps noise out of the low end.
    width = max(2, int(1 / (1 - pole)))
    return signal - np.convolve(signal, np.ones(width) / width, mode='same')


def place(bus, start, signal, gain=1.0, pan=0.0):
    offset = int(start * RATE)
    if offset < 0:
        signal = signal[-offset:]
        offset = 0
    length = min(len(signal), SIZE - offset)
    if length <= 0:
        return
    if signal.ndim == 1:
        angle = (pan + 1) * np.pi / 4
        signal = np.column_stack((signal * np.cos(angle), signal * np.sin(angle)))
    bus[offset:offset + length] += signal[:length] * gain


def delay(bus, delays, feedback):
    dry = bus.copy()
    for seconds, gain in zip(delays, feedback):
        shift = int(seconds * RATE)
        bus[shift:] += dry[:-shift, ::-1] * gain


def kick(start, strength=1):
    t = clock(.47)
    phase = 2 * np.pi * (47 * t + 6.5 * (1 - np.exp(-t * 34)))
    body = np.sin(phase) * np.exp(-t * 11) * (1 - np.exp(-t * 1400))
    click = highpass(RNG.normal(size=len(t)), .8) * np.exp(-t * 430) * .075
    place(drums, start, np.tanh((body + click) * 1.4), .63 * strength)
    kicks.append(start)


def clap(start, strength=1):
    t = clock(.33)
    noise = highpass(RNG.normal(size=len(t)), .88)
    env = sum(np.where(t >= x, np.exp(-np.maximum(0, t - x) * 155), 0)
              for x in (0, .009, .019))
    env += np.where(t > .027, np.exp(-np.maximum(0, t - .027) * 24), 0) * .55
    tone = np.sin(2 * np.pi * 185 * t) * np.exp(-t * 45)
    signal = noise * env * .17 + tone * .16
    place(drums, start, signal, .45 * strength)
    place(accents, start + .018, signal, .09 * strength, -.65)
    place(accents, start + .032, signal, .075 * strength, .65)


def hat(start, opened=False, strength=1, pan=0):
    t = clock(.27 if opened else .075)
    noise = highpass(RNG.normal(size=len(t)), .65)
    metal = sum(np.sin(2 * np.pi * f * t) for f in (6173, 8311, 10477)) * .065
    env = (1 - np.exp(-t * 2000)) * np.exp(-t * (18 if opened else 85))
    place(drums, start, (noise * .13 + metal) * env, .25 * strength, pan)


def bass(start, midi, duration, gain=.24):
    t = clock(duration)
    f = note(midi)
    env = np.minimum(t / .008, 1) * np.minimum((duration - t) / .06, 1)
    cutoff = np.exp(-t * 8)
    signal = np.sin(2 * np.pi * f * t) * .9
    for harmonic in range(2, 9):
        signal += np.sin(2 * np.pi * f * harmonic * t) * cutoff ** (harmonic / 5) / harmonic * .43
    place(music, start, np.tanh(signal * 1.4) * env, gain)


def keys(start, notes, duration=1.3, gain=.085):
    t = clock(duration)
    stereo = np.zeros((len(t), 2))
    env = (1 - np.exp(-t * 90)) * np.exp(-t * 3.4) * np.minimum((duration - t) / .12, 1)
    for i, midi in enumerate(notes):
        f = note(midi)
        for channel, detune in enumerate((-.0017, .0017)):
            phase = 2 * np.pi * f * (1 + detune) * t
            # Soft electric-piano attack followed by a warm, filtered body.
            signal = np.sin(phase + 1.05 * np.sin(phase * 2) * np.exp(-t * 8))
            signal += .13 * np.sin(phase * 3) * np.exp(-t * 7)
            stereo[:, channel] += signal * env / np.sqrt(len(notes))
    place(music, start, stereo, gain)


def pad(start, notes, duration, gain=.026):
    t = clock(duration)
    env = np.minimum(t / .65, 1) * np.minimum((duration - t) / 1.1, 1)
    stereo = np.zeros((len(t), 2))
    for midi in notes:
        f = note(midi)
        for channel, detune in enumerate((-.003, .003)):
            phase = 2 * np.pi * f * (1 + detune) * t
            stereo[:, channel] += (np.sin(phase) + .2 * np.sin(phase * 2)) * env
    place(music, start, stereo, gain / np.sqrt(len(notes)))


def lead(start, midi, gain=.075):
    t = clock(.8)
    f = note(midi)
    env = (1 - np.exp(-t * 100)) * np.exp(-t * 6)
    phase = 2 * np.pi * f * t
    signal = np.sin(phase + .8 * np.sin(phase * 2) * np.exp(-t * 11)) * env
    place(music, start, signal, gain, .24)
    place(music, start + BEAT * .75, signal, gain * .28, -.55)
    place(music, start + BEAT * 1.5, signal, gain * .12, .6)


def transition(at, strength=.6):
    duration = .72
    t = clock(duration)
    noise = highpass(RNG.normal(size=len(t)), .97)
    sweep = np.sin(2 * np.pi * (150 * t + 2000 * t ** 3))
    env = (t / duration) ** 2 * np.minimum((duration - t) / .008, 1)
    place(accents, at - duration, (noise * .1 + sweep * .055) * env, strength, -.25)
    tail = clock(.55)
    place(accents, at, highpass(RNG.normal(size=len(tail)), .9) * np.exp(-tail * 9), .034 * strength, .3)


# Em9 → Cmaj9 → Gmaj9 → D6/9, three-bar harmonic phrases.
chords = [[52, 59, 62, 66, 67], [48, 55, 59, 62, 64],
          [43, 54, 57, 59, 62], [50, 57, 59, 64, 66]]
roots = [28, 36, 31, 38]
# A melodic opening, followed by a rhythm section at the first app reveal.
pad(0, chords[0], 4.7, .045)
for beat, midi in [(0, 76), (.75, 71), (1.5, 74), (2.5, 78), (3, 79)]:
    lead(.25 + beat * .6, midi, .07)
for scene in [3.8, 9.4, 15.1, 20.8, 26.1, 31.7]:
    transition(scene, .58 if scene != 26.1 else .85)

for bar in range(15):
    start = DROP + bar * 4 * BEAT
    if start >= 31.55:
        break
    section = (bar // 3) % 4
    chord, root = chords[section], roots[section]
    stripped = 20.8 <= start < 25.6
    pad(start, chord, 4 * BEAT + .65)
    for position, velocity in [(0, 1), (1.75, .82), (2.5, .9)]:
        hit = start + position * BEAT
        if hit < 31.45 and (not stripped or position == 0):
            kick(hit, velocity)
    if not stripped:
        for position in [1, 3]:
            clap(start + position * BEAT, .9 + RNG.uniform(-.06, .06))
        for i in range(8):
            swing = .022 if i % 2 else 0
            hat(start + i * BEAT / 2 + swing, opened=(i == 5),
                strength=.8 if i % 2 else .54, pan=(-.32 if i % 2 else .28))
        if bar % 3 == 2:
            for position in [3.25, 3.5, 3.75]:
                hat(start + position * BEAT, strength=.4, pan=.6)
    for position, length, pitch in [(0, .62, 0), (.75, .65, 0), (1.75, .45, 12), (2.5, .6, 0), (3.25, .42, 7)]:
        bass(start + position * BEAT, root + pitch, length * BEAT, .12 if stripped else .22)
    for position, velocity in [(.5, 1), (1.5, .65), (2.75, .85)]:
        keys(start + position * BEAT, chord, gain=.058 if stripped else .09 * velocity)
    if bar % 3 in (1, 2) and not stripped:
        melody = [(0.5, 76), (1.25, 74), (2.25, 71), (3.5, 78)] if bar % 2 else [(0.5, 79), (1.5, 78), (2.75, 74)]
        for position, midi in melody:
            lead(start + position * BEAT, midi, .055)

# A short fill into the responsive-layout scene, then a clean brand resolution.
for i in range(6):
    clap(25.4 + i * .105, .18 + i * .065)
kick(31.7, .8)
pad(31.7, chords[0], 4.3, .07)
keys(31.7, [52, 59, 66, 67, 74], 2.2, .14)
for i, midi in enumerate([83, 78, 76]):
    lead(32 + i * .27, midi, .06)

# Subtle cross-channel echoes and kick-dependent ducking leave space for percussion.
delay(music, [BEAT * .75, BEAT * 1.5, BEAT * 2.25], [.16, .075, .035])
delay(accents, [.043, .097, .173], [.28, .16, .09])
duck = np.ones(SIZE)
for at in kicks:
    first = int(at * RATE)
    t = clock(.3)
    length = min(len(t), SIZE - first)
    if length > 0:
        duck[first:first + length] *= 1 - .38 * np.exp(-t[:length] * 15)
score = music * duck[:, None] + drums + accents
score -= score.mean(axis=0)
score = np.tanh(score * 1.15)
time = np.arange(SIZE) / RATE
fade = np.minimum(np.minimum(time / .035, 1), np.clip((DURATION - time) / 1.3, 0, 1))
score *= fade[:, None]
score *= .89 / max(np.abs(score).max(), 1e-12)
output = Path(__file__).with_name('soundtrack.wav')
with wave.open(str(output), 'wb') as track:
    track.setparams((2, 2, RATE, 0, 'NONE', 'not compressed'))
    track.writeframes((score * 32767).astype('<i2').tobytes())
print(f'{output}: {DURATION}s, {BPM} BPM, stereo electronic / garage score')
