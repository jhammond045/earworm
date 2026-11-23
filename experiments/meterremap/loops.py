"""Make the real-recordings directory for the meter remap prototype from the Apple Loops that
GarageBand installs (macOS only; uses afconvert). Each loop is decoded to 48 kHz mono 16-bit WAV,
and index.tsv lists file, category, beat count, time signature and genre folder, read from the
loop's own metadata. The loops are not part of the repository; keep the output outside it.

    python3 -I experiments/meterremap/loops.py "/Library/Audio/Apple Loops/Apple" <out dir>
"""

import os
import struct
import subprocess
import sys


def meta(path):
    """Key/value metadata of an Apple Loop: CAF 'info' and 'uuid' chunks of null-terminated pairs."""
    with open(path, 'rb') as f:
        d = f.read()
    if d[:4] != b'caff':
        return {}
    out, i = {}, 8
    while i + 12 <= len(d):
        kind, n = d[i:i + 4], struct.unpack('>q', d[i + 4:i + 12])[0]
        body = d[i + 12:i + 12 + n] if n >= 0 else d[i + 12:]
        if kind == b'uuid':
            body = body[16:]
        if kind in (b'info', b'uuid') and len(body) >= 4:
            count = struct.unpack('>I', body[:4])[0]
            parts = body[4:].split(b'\0')
            if len(parts) >= 2 * count:
                for j in range(count):
                    out[parts[2 * j].decode('utf8', 'replace')] = parts[2 * j + 1].decode('utf8', 'replace')
        if n < 0:
            break
        i += 12 + n
    return out


def main(src, dst):
    os.makedirs(dst, exist_ok=True)
    rows = []
    for folder, _, files in sorted(os.walk(src)):
        for name in sorted(files):
            if not name.endswith('.caf'):
                continue
            m = meta(os.path.join(folder, name))
            if 'beat count' not in m:
                continue
            wav = name[:-4].replace('/', '-') + '.wav'
            out = os.path.join(dst, wav)
            if not os.path.exists(out):
                subprocess.run(['afconvert', '-f', 'WAVE', '-d', 'LEI16@48000', '-c', '1', os.path.join(folder, name), out], check=True)
            rows.append((wav, m.get('category', ''), m['beat count'], m.get('time signature', ''), os.path.basename(folder)))
    with open(os.path.join(dst, 'index.tsv'), 'w') as f:
        f.write('file\tcategory\tbeats\ttime signature\tgenre\n')
        for r in rows:
            f.write('\t'.join(r) + '\n')
    print(f'{len(rows)} loops in {dst}')


if __name__ == '__main__':
    main(sys.argv[1], sys.argv[2])
