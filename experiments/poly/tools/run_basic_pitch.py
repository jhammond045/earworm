"""Run Spotify's Basic Pitch (pip install basic-pitch) over every WAV in a
directory that tools/prep.py wrote, with its default settings, and leave its
notes beside each file as `<name>.bp.tsv` (onset s, offset s, MIDI pitch,
amplitude) and its inference time as `<name>.bp.secs`. `cargo run --release
-- real <dir>` then scores it as one more offline row.

  python run_basic_pitch.py <dir>
"""

import glob
import os
import sys
import time

from basic_pitch import ICASSP_2022_MODEL_PATH
from basic_pitch.inference import Model, predict

model = Model(ICASSP_2022_MODEL_PATH)
for wav in sorted(glob.glob(os.path.join(sys.argv[1], "*.wav"))):
    stem = wav[: -len(".wav")]
    t0 = time.perf_counter()
    _, _, notes = predict(wav, model)
    secs = time.perf_counter() - t0
    with open(stem + ".bp.tsv", "w") as f:
        for on, off, pitch, amp, _bends in sorted(notes, key=lambda n: n[0]):
            f.write(f"{on:.4f}\t{off:.4f}\t{pitch}\t{amp:.4f}\n")
    with open(stem + ".bp.secs", "w") as f:
        f.write(f"{secs:.4f}\n")
    print(f"{os.path.basename(stem)}: {len(notes)} notes, {secs:.2f} s", flush=True)
