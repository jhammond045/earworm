"""Run Basic Pitch's own Python code with its ONNX model (float32 on the CPU, onnxruntime) over every
WAV in a directory, with its default settings, and leave its notes beside each file as
`<name>.ref.tsv`: onset s, offset s, MIDI pitch, amplitude. `cargo run --release -p analysis --bin
poly_eval` compares the Rust port's notes with these.

  python reference.py <dir> [--dump <name>]

`--dump <name>` also writes, for that one file, the 22050 Hz audio the model reads
(`<name>.ref22k.f32`) and the model's unwrapped outputs (`<name>.ref.{note,onset,contour}.f32`,
frames x bins, float32), to compare the port stage by stage. Needs
`pip install "basic-pitch[onnx]"`.
"""

import glob
import os
import sys
import time

import numpy as np
import librosa
from basic_pitch import ICASSP_2022_MODEL_PATH
from basic_pitch.constants import AUDIO_SAMPLE_RATE
from basic_pitch.inference import Model, predict

model = Model(os.path.join(os.path.dirname(str(ICASSP_2022_MODEL_PATH)), "nmp.onnx"))
dump = sys.argv[sys.argv.index("--dump") + 1] if "--dump" in sys.argv else None
for wav in sorted(glob.glob(os.path.join(sys.argv[1], "*.wav"))):
    stem = wav[: -len(".wav")]
    name = os.path.basename(stem)
    if dump and name != dump:
        continue
    t0 = time.perf_counter()
    out, _, notes = predict(wav, model)
    secs = time.perf_counter() - t0
    with open(stem + ".ref.tsv", "w") as f:
        for on, off, pitch, amp, _bends in sorted(notes, key=lambda n: (n[0], n[2])):
            f.write(f"{on:.6f}\t{off:.6f}\t{pitch}\t{amp:.6f}\n")
    if dump:
        audio, _ = librosa.load(wav, sr=AUDIO_SAMPLE_RATE, mono=True)
        audio.astype("<f4").tofile(stem + ".ref22k.f32")
        for k in ("note", "onset", "contour"):
            out[k].astype("<f4").tofile(f"{stem}.ref.{k}.f32")
    print(f"{name}: {len(notes)} notes, {secs:.2f} s", flush=True)
