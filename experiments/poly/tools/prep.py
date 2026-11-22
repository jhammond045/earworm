"""Turn public labelled recordings into what `cargo run --release -- real <dir>`
reads: one 48 kHz mono WAV per excerpt and, beside it, a note list
`<name>.notes.tsv` with one played note per line: onset seconds, offset
seconds, MIDI pitch (fractional when the labels measured it), and velocity
(0 to 127, or -1 when the labels have none).

Needs numpy, scipy, soundfile and mido; docs/poly.md "Stage P0" has the
whole sequence, downloads included.

  python prep.py guitarset <GuitarSet dir with annotation/ and mic/> <out>
  python prep.py maestro <out> [<cache dir>]
  python prep.py trios <TRIOS Dataset dir> <out>

Nothing here is committed: keep the downloads and the output outside the
repository, or under experiments/poly/data, which git ignores.
"""

import glob
import io
import json
import os
import sys
import urllib.request
import zipfile

import mido
import numpy as np
import soundfile as sf
from scipy.signal import resample_poly

SR = 48_000


def write_excerpt(out, name, audio, sr, notes):
    if audio.ndim > 1:
        audio = audio.mean(axis=1)
    if sr != SR:
        g = np.gcd(SR, sr)
        audio = resample_poly(audio, SR // g, sr // g)
    sf.write(os.path.join(out, name + ".wav"), audio.astype(np.float32), SR, subtype="FLOAT")
    notes = sorted(notes)
    with open(os.path.join(out, name + ".notes.tsv"), "w") as f:
        for on, off, midi, vel in notes:
            f.write(f"{on:.4f}\t{off:.4f}\t{midi:.3f}\t{vel}\n")
    return len(notes), len(audio) / SR


def guitarset(root, out):
    """Every excerpt, mic recording; notes from the per-string note_midi labels."""
    total = 0.0
    for jpath in sorted(glob.glob(os.path.join(root, "annotation", "*.jams"))):
        name = os.path.basename(jpath)[: -len(".jams")]
        j = json.load(open(jpath))
        notes = []
        for a in j["annotations"]:
            if a["namespace"] != "note_midi":
                continue
            for d in a["data"]:
                notes.append((d["time"], d["time"] + d["duration"], d["value"], -1))
        audio, sr = sf.read(os.path.join(root, "mic", name + "_mic.wav"))
        n, secs = write_excerpt(out, name, audio, sr, notes)
        total += secs
    print(f"guitarset: {total / 60:.1f} min")


def midi_notes(path, pedal=True):
    """(onset, offset, pitch, velocity) from a MIDI file. With `pedal`, a note
    held by the sustain pedal (CC64 >= 64) sounds until the pedal lifts or the
    same key is struck again, as piano transcription labels usually count it."""
    t = 0.0
    down = {}
    held = []
    sustain = False
    notes = []

    def end(i, at):
        on, p, v = held[i]
        notes.append((on, at, float(p), v))

    for msg in mido.MidiFile(path):
        t += msg.time
        if msg.type == "control_change" and msg.control == 64 and pedal:
            sustain = msg.value >= 64
            if not sustain:
                for i in reversed(range(len(held))):
                    if held[i][1] not in down:
                        end(i, t)
                        held.pop(i)
        elif msg.type == "note_on" and msg.velocity > 0:
            for i in reversed(range(len(held))):
                if held[i][1] == msg.note:
                    end(i, t)
                    held.pop(i)
            down[msg.note] = True
            held.append((t, msg.note, msg.velocity))
        elif msg.type in ("note_off", "note_on"):
            down.pop(msg.note, None)
            if not sustain:
                for i in reversed(range(len(held))):
                    if held[i][1] == msg.note:
                        end(i, t)
                        held.pop(i)
    for i in range(len(held)):
        end(i, t)
    return notes


class HttpFile(io.RawIOBase):
    """A read-only file over HTTP range requests, so zipfile can pull single
    members out of the 108 GB MAESTRO archive."""

    def __init__(self, url):
        self.url, self.pos = url, 0
        with urllib.request.urlopen(urllib.request.Request(url, method="HEAD")) as r:
            self.size = int(r.headers["Content-Length"])

    def seekable(self):
        return True

    def readable(self):
        return True

    def tell(self):
        return self.pos

    def seek(self, off, whence=0):
        self.pos = [off, self.pos + off, self.size + off][whence]
        return self.pos

    def readinto(self, b):
        if self.pos >= self.size:
            return 0
        end = min(self.pos + len(b), self.size) - 1
        req = urllib.request.Request(self.url, headers={"Range": f"bytes={self.pos}-{end}"})
        with urllib.request.urlopen(req) as r:
            data = r.read()
        b[: len(data)] = data
        self.pos += len(data)
        return len(data)


MAESTRO = "https://storage.googleapis.com/magentadata/datasets/maestro/v3.0.0/"
# Eight pieces from the test split, different composers, textures and years
# (the years are different recording set-ups), about 22 minutes.
MAESTRO_PIECES = [
    "2009/MIDI-Unprocessed_02_R1_2009_03-06_ORIG_MID--AUDIO_02_R1_2009_02_R1_2009_04_WAV",
    "2018/MIDI-Unprocessed_Recital16_MID--AUDIO_16_R1_2018_wav--3",
    "2014/MIDI-UNPROCESSED_01-03_R1_2014_MID--AUDIO_01_R1_2014_wav--1",
    "2011/MIDI-Unprocessed_02_R1_2011_MID--AUDIO_R1-D1_10_Track10_wav",
    "2015/MIDI-Unprocessed_R2_D1-2-3-6-7-8-11_mid--AUDIO-from_mp3_03_R2_2015_wav--1",
    "2011/MIDI-Unprocessed_24_R1_2011_MID--AUDIO_R1-D9_11_Track11_wav",
    "2008/MIDI-Unprocessed_17_R2_2008_01-04_ORIG_MID--AUDIO_17_R2_2008_wav--1",
    "2011/MIDI-Unprocessed_18_R1_2011_MID--AUDIO_R1-D7_07_Track07_wav",
]


def maestro(out, cache):
    os.makedirs(cache, exist_ok=True)
    z = None
    total = 0.0
    for i, stem in enumerate(MAESTRO_PIECES):
        files = {}
        for ext in (".wav", ".midi"):
            local = os.path.join(cache, os.path.basename(stem) + ext)
            if not os.path.exists(local):
                if z is None:
                    z = zipfile.ZipFile(io.BufferedReader(HttpFile(MAESTRO + "maestro-v3.0.0.zip"), 1 << 22))
                with z.open("maestro-v3.0.0/" + stem + ext) as src, open(local + ".part", "wb") as dst:
                    while chunk := src.read(1 << 22):
                        dst.write(chunk)
                os.rename(local + ".part", local)
            files[ext] = local
        audio, sr = sf.read(files[".wav"])
        n, secs = write_excerpt(out, f"maestro{i}", audio, sr, midi_notes(files[".midi"]))
        total += secs
        print(f"maestro{i}: {n} notes, {secs:.0f} s, {stem}")
    print(f"maestro: {total / 60:.1f} min")


TRIOS_DRUMS = {"kick", "ride", "snare"}


def trios(root, out):
    """Each piece's mix, with the notes of every pitched part. Take Five's mix
    has drums in it too; they are left in the audio and out of the labels."""
    for piece in sorted(os.listdir(root)):
        d = os.path.join(root, piece)
        if not os.path.isdir(d):
            continue
        notes = []
        for mid in sorted(glob.glob(os.path.join(d, "*.mid"))):
            if os.path.basename(mid)[:-4] not in TRIOS_DRUMS:
                notes += midi_notes(mid)
        audio, sr = sf.read(os.path.join(d, "mix.wav"))
        n, secs = write_excerpt(out, f"trios_{piece}", audio, sr, notes)
        print(f"trios_{piece}: {n} notes, {secs:.0f} s")


if __name__ == "__main__":
    kind = sys.argv[1]
    if kind == "guitarset":
        os.makedirs(sys.argv[3], exist_ok=True)
        guitarset(sys.argv[2], sys.argv[3])
    elif kind == "maestro":
        os.makedirs(sys.argv[2], exist_ok=True)
        maestro(sys.argv[2], sys.argv[3] if len(sys.argv) > 3 else os.path.join(sys.argv[2], "cache"))
    elif kind == "trios":
        os.makedirs(sys.argv[3], exist_ok=True)
        trios(sys.argv[2], sys.argv[3])
    else:
        sys.exit(__doc__)
