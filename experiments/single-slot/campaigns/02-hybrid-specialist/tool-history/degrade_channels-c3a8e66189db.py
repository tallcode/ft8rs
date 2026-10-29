#!/usr/bin/env python3
"""Add held-out AWGN to existing single-signal native channel WAVs.

The dB parameter increases estimated *noise* power, not measured signal power.
The 1600–2600 Hz off-signal interval estimates the original white-noise variance.
Original fixtures remain unchanged; all outputs are private float32 WAVs.
"""
import hashlib
import json
import pathlib
import sys

import numpy as np
import scipy
from scipy.io import wavfile

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parent.parent
ROOT = pathlib.Path('/tmp/ft8rs-single-slot-lab/channel-degraded')


def main():
    ROOT.mkdir(exist_ok=False)
    source = REPO / 'crates/ft8rs-core/tests/decoder/data/ft8/channels'
    records = {}
    for manifest in sorted(source.glob('*.tsv')):
        for line in manifest.read_text().splitlines():
            if line.startswith('#'):
                continue
            p = line.split('\t')
            if 'disturbed' in p[1]:
                continue
            path = source / p[1]
            fs, audio = wavfile.read(path)
            assert fs == 12000 and audio.ndim == 1 and len(audio) == 180000
            if np.issubdtype(audio.dtype, np.integer):
                audio = audio.astype(np.float64)/(2**(np.iinfo(audio.dtype).bits-1))
            else:
                audio = audio.astype(np.float64)
            spectrum = np.fft.rfft(audio)
            bins = (np.fft.rfftfreq(len(audio), 1/fs) >= 1600) & (np.fft.rfftfreq(len(audio), 1/fs) <= 2600)
            noise_variance = float(np.mean(np.abs(spectrum[bins])**2)/len(audio))
            for db in [10, 12]:
                seed = 2026103401 + int(hashlib.sha256(path.name.encode()).hexdigest()[:8], 16) + db
                added_variance = noise_variance*(10**(db/10)-1)
                noisy = audio + np.random.default_rng(seed).normal(0, np.sqrt(added_variance), len(audio))
                output = ROOT / f'{path.stem}_noise{db}.wav'
                wavfile.write(output, fs, noisy.astype(np.float32))
                records[output.name] = dict(source=str(path), source_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
                                             output_sha256=hashlib.sha256(output.read_bytes()).hexdigest(),
                                             message=p[2], frequency=float(p[3]), dt=float(p[4]),
                                             source_nominal_snr=float(p[5]), added_noise_db=db,
                                             noise_variance_estimate=noise_variance, added_variance=added_variance, seed=seed)
    (ROOT / 'manifest.json').write_text(json.dumps(records, indent=2)+'\n')
    (ROOT / 'environment.json').write_text(json.dumps(dict(python=sys.version,numpy=np.__version__,scipy=scipy.__version__),indent=2)+'\n')
    print(ROOT / 'manifest.json')


if __name__ == '__main__':
    main()
