#!/usr/bin/env python3
"""Regenerate checked-in FT8 oracles using unchanged upstream Fortran/C++.

Only regeneration needs a Fortran compiler and Boost headers; cargo tests do not.
Output goes to a staging directory first. Review all diffs before replacing data.
"""
import argparse
import array
import hashlib
import json
import math
import os
from pathlib import Path
import random
import re
import shutil
import sys
import subprocess
import tempfile
import wave


def run(args, cwd, **kwargs):
    return subprocess.run([str(a) for a in args], cwd=cwd, check=True, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--decoder', choices=['wsjtx', 'jtdx'], required=True)
    parser.add_argument('--messages', type=Path, required=True,
                        help='WSJT-X lib/ft8/ft8_testmsg.f90 (shared 49-case corpus)')
    parser.add_argument('--fc', required=True)
    parser.add_argument('--cxx', default='clang++')
    parser.add_argument('--boost-include', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--runtime-dir', type=Path,
                        help='Relocated macOS compiler static runtime directory')
    args = parser.parse_args()
    source = args.source.resolve()
    output = args.output.resolve()
    protocol_output = output / 'ft8/protocol'
    audio_output = output / 'ft8/audio'
    provenance_output = output / 'ft8/provenance'
    waveform_output = output / 'ft8/waveforms'
    if (protocol_output / f'{args.decoder}.tsv').exists():
        parser.error('output already contains this decoder; choose a fresh candidate directory')
    for directory in [protocol_output, audio_output, provenance_output, waveform_output]:
        directory.mkdir(parents=True, exist_ok=True)
    test_root = Path(__file__).resolve().parents[1] / 'crates/ft8rs-core/tests/decoder'
    messages = re.findall(r'"([^"]+)"', args.messages.read_text())
    declared = re.search(r'NTEST\s*=\s*(\d+)', args.messages.read_text(), re.I)
    assert declared and len(messages) == int(declared[1]), 'message corpus is incomplete'
    calls = ['K1ABC', 'W9XYZ', 'KH1/KH7Z', 'PJ4/K1ABC', 'YW18FIFA', 'KA1ABC',
             'PA9XYZ', 'G4ABC/P', 'G3AAA', 'KA0DEF', 'BG5ATV', 'K1JT']
    scenarios = [
        ('cq', 'CQ K1ABC FN42', 1000.0, 0.0, -10.0, 'awgn'),
        ('report', 'W9XYZ K1ABC -11', 1513.0, 0.7, -14.0, 'awgn'),
        ('roger', 'K1ABC W9XYZ R-09', 244.0, -0.2, -14.0, 'awgn'),
        ('rr73', 'K1ABC W9XYZ RR73', 2852.0, 0.3, -14.0, 'awgn'),
        ('free_text', 'TNX BOB 73 GL', 600.0, 0.1, -10.0, 'awgn'),
        ('telemetry', '123456789ABCDEF012', 1800.0, 0.2, -10.0, 'awgn'),
        ('field_day', 'K1ABC W9XYZ 6A WI', 2200.0, 0.0, -10.0, 'awgn'),
        ('nonstandard', 'CQ KH1/KH7Z', 1200.0, 0.0, -10.0, 'awgn'),
        ('late', 'CQ K1ABC FN42', 2990.0, 1.2, -12.0, 'awgn'),
        ('multipath', 'CQ K1ABC FN42', 1000.0, 0.0, -14.0, 'echo'),
        ('fading', 'CQ K1ABC FN42', 1000.0, 0.0, -14.0, 'fading'),
        ('weak', 'CQ K1ABC FN42', 1000.0, 0.0, -20.0, 'awgn'),
        ('neighbor', 'CQ W9XYZ EN37', 1025.0, 0.0, -6.0, 'awgn'),
    ]
    lib = source / 'lib'
    protocol = lib / ('ft8v2' if args.decoder == 'jtdx' else '77bit')
    ft8 = lib / ('ft8v2' if args.decoder == 'jtdx' else 'ft8')
    files = [lib / 'crc.f90', lib / 'packjt.f90', protocol / 'packjt77.f90',
             lib / 'fmtmsg.f90', lib / 'grid2deg.f90', lib / 'deg2grid.f90', lib / 'chkcall.f90',
             ft8 / 'encode174_91.f90']
    if args.decoder == 'jtdx':
        files += [lib / 'ft8_mod1.f90', lib / 'genft8.f90']
    else:
        files += [ft8 / 'genft8.f90']
    files += [lib / 'timer_module.f90',
              lib / ('gen_ft8wave.f90' if args.decoder == 'jtdx' else 'ft8/gen_ft8wave.f90'),
              lib / ('gfsk_pulse.f90' if args.decoder == 'jtdx' else 'ft2/gfsk_pulse.f90')]
    with tempfile.TemporaryDirectory(prefix='ft8rs-reference-') as tmp:
        work = Path(tmp)
        (work / 'calls.txt').write_text('\n'.join(calls) + '\n')
        run([args.cxx, '-std=c++11', '-O2', '-I', args.boost_include.resolve(),
             '-c', lib / 'crc14.cpp', '-o', 'crc14.o'], work)
        flags = ['-O2', '-cpp', '-ffree-line-length-none', '-fallow-argument-mismatch',
                 '-fwrapv', '-I', ft8, '-I', lib]
        if args.decoder == 'jtdx':
            flags += ['-DJTDX']
        for path in files + [test_root / 'reference/probe.F90']:
            run([args.fc, *flags, '-c', path], work)
        objects = sorted(work.glob('*.o'))
        if args.runtime_dir:
            runtime = args.runtime_dir.resolve()
            run(['clang++', '-o', 'probe', *objects, runtime / 'libgfortran.a',
                 runtime / 'libquadmath.a', runtime / 'libgcc.a'], work)
        else:
            run([args.fc, '-o', 'probe', *objects, '-lstdc++'], work)

        def probe(message, frequency=1000.0):
            result = run([work / 'probe'], work,
                         input=f'{message}\n{frequency}\n', text=True, capture_output=True)
            lines = result.stdout.splitlines()
            assert len(lines) == 5, lines
            return lines

        vectors = ['# input\tdecoded\tbits77\tcodeword174\ttones79\ttype']
        for message in messages:
            decoded, bits, codeword, tones, kind = probe(message)
            vectors.append('\t'.join([message, decoded.strip(), bits, codeword, tones, kind]))
        (protocol_output / f'{args.decoder}.tsv').write_text('\n'.join(vectors) + '\n')
        (protocol_output / 'calls.txt').write_text('\n'.join(calls) + '\n')
        (protocol_output / 'messages.txt').write_text('\n'.join(messages) + '\n')
        hash_source = source / 'tests/unit/packjt77/test_packjt77_hash_vectors.f90'
        if args.decoder == 'wsjtx' and hash_source.exists():
            hashes = re.findall(r"call expect_hash_vector\('([^']*)', (\d+), (\d+), (\d+)\)", hash_source.read_text())
            assert hashes
            (protocol_output / 'hash.tsv').write_text('# call\thash10\thash12\thash22\n' +
                '\n'.join('\t'.join(row) for row in hashes) + '\n')
        elif not (protocol_output / 'hash.tsv').exists():
            shutil.copy2(test_root / 'data/ft8/protocol/hash.tsv', protocol_output / 'hash.tsv')
        probe('CQ K1ABC FN42')
        clean = array.array('f')
        clean.frombytes((work / 'carrier.f32').read_bytes())
        clean_pcm = array.array('h', [0]*6000 + [round(v*12000) for v in clean] + [0]*(180000-6000-len(clean)))
        if sys.byteorder != 'little':
            clean_pcm.byteswap()
        with wave.open(str(waveform_output / f'{args.decoder}.wav'), 'wb') as wav:
            wav.setparams((1, 2, 12000, 0, 'NONE', 'not compressed'))
            wav.writeframes(clean_pcm.tobytes())
        manifest = ['# id\tfile\tmessage\tfreq\tdt\tsnr\tchannel\tsha256']
        for index, (name, message, freq, dt, snr, channel) in enumerate(scenarios):
            decoded = probe(message, freq)[0].strip()
            carrier = array.array('f')
            carrier.frombytes((work / 'carrier.f32').read_bytes())
            assert len(carrier) == 151680
            if channel == 'echo':
                delay = 12  # 1 ms, deterministic two-path channel, not Watterson
                carrier = [v + 0.45 * (carrier[i-delay] if i >= delay else 0)
                           for i, v in enumerate(carrier)]
            if channel == 'fading':
                carrier = [v * (0.6 + 0.4 * math.cos(2*math.pi*i/12000/3))
                           for i, v in enumerate(carrier)]
            power = sum(v*v for v in carrier) / len(carrier)
            amplitude = math.sqrt((2500 / 6000) * 10**(snr/10) / power)
            rng = random.Random(2026102800 + index)
            audio = [rng.gauss(0, 1) for _ in range(180000)]
            start = round((dt + 0.5) * 12000)
            for i, value in enumerate(carrier):
                dest = start + i
                if 0 <= dest < len(audio):
                    audio[dest] += value * amplitude
            # array('h') rejects overflow; never silently clip a reference fixture.
            pcm = array.array('h', (round(v*1000) for v in audio))
            if sys.byteorder != 'little':
                pcm.byteswap()
            filename = f'{args.decoder}_{name}.wav'
            with wave.open(str(audio_output / filename), 'wb') as wav:
                wav.setparams((1, 2, 12000, 0, 'NONE', 'not compressed'))
                wav.writeframes(pcm.tobytes())
            digest = hashlib.sha256((audio_output / filename).read_bytes()).hexdigest()
            manifest.append('\t'.join(map(str, [name, filename, decoded, freq, dt, snr, channel, digest])))
        (audio_output / f'{args.decoder}.tsv').write_text('\n'.join(manifest) + '\n')
    revision = subprocess.check_output(['git', '-C', source, 'rev-parse', 'HEAD'], text=True).strip()
    recorded_sources = files + [lib / 'crc14.cpp', lib / 'pfx.f90', ft8 / 'ldpc_174_91_c_generator.f90']
    if args.decoder == 'wsjtx':
        recorded_sources += [ft8 / 'ft8_params.f90']
    if args.messages.resolve().is_relative_to(source):
        recorded_sources += [args.messages.resolve()]
    if hash_source.exists():
        recorded_sources += [hash_source]
    metadata = {'decoder': args.decoder, 'revision': revision, 'compiler': run([args.fc, '--version'], output, text=True, capture_output=True).stdout.splitlines()[0],
                'protocol_cases': len(messages), 'audio_cases': len(scenarios),
                'python': sys.version, 'native_byteorder': sys.byteorder,
                'macos_deployment_target': os.environ.get('MACOSX_DEPLOYMENT_TARGET'),
                'cxx': run([args.cxx, '--version'], output, text=True, capture_output=True).stdout.splitlines()[0],
                'boost_crc_sha256': hashlib.sha256((args.boost_include / 'boost/crc.hpp').read_bytes()).hexdigest(),
                'boost_version_sha256': hashlib.sha256((args.boost_include / 'boost/version.hpp').read_bytes()).hexdigest(),
                'compile_flags': [str(flag) for flag in flags],
                'generator_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                'driver_sha256': hashlib.sha256((test_root / 'reference/probe.F90').read_bytes()).hexdigest(),
                'protocol_sha256': hashlib.sha256((protocol_output / f'{args.decoder}.tsv').read_bytes()).hexdigest(),
                'waveform_sha256': hashlib.sha256((waveform_output / f'{args.decoder}.wav').read_bytes()).hexdigest(), 'messages_sha256': hashlib.sha256(args.messages.read_bytes()).hexdigest(),
                'sources': {str(p.relative_to(source)): hashlib.sha256(p.read_bytes()).hexdigest()
                            for p in recorded_sources},
                'noise': 'Python random.Random MT19937, seed=2026102800+scenario_index; gauss(0,1)',
                'snr': '2500 Hz reference, active-signal RMS; PCM gain=1000, no clipping',
                'channels': 'echo/fading are deterministic test-layer transforms, not Watterson'}
    (provenance_output / f'{args.decoder}.json').write_text(json.dumps(metadata, indent=2) + '\n')


if __name__ == '__main__':
    main()
