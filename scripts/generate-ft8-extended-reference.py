#!/usr/bin/env python3
"""Generate native test28, Watterson, and real-recording degradation candidates.

This extends existing data; it never regenerates or replaces the base corpus.
Only this explicit tool needs Fortran, Boost, and single-precision FFTW.
"""
import argparse
import array
import hashlib
import json
import math
import os
from pathlib import Path
import random
import shutil
import subprocess
import sys
import tempfile
import wave

ROOT = Path(__file__).resolve().parents[1]
TEST = ROOT / 'crates/ft8rs-core/tests/decoder'


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(args, cwd, **kwargs):
    kwargs.setdefault('capture_output', True)
    try:
        return subprocess.run([str(a) for a in args], cwd=cwd, check=True, **kwargs)
    except subprocess.CalledProcessError as error:
        for stream in [error.stdout, error.stderr]:
            if stream:
                print(stream.decode(errors='replace') if isinstance(stream, bytes) else stream, file=sys.stderr)
        raise


def write_wav(path, pcm):
    data = array.array('h', pcm)
    if sys.byteorder != 'little':
        data.byteswap()
    with wave.open(str(path), 'wb') as output:
        output.setparams((1, 2, 12000, 0, 'NONE', 'not compressed'))
        output.writeframes(data.tobytes())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base-data', type=Path, default=TEST/'data')
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--decoder', choices=['wsjtx', 'jtdx'], required=True)
    parser.add_argument('--calls-corpus', type=Path, required=True)
    parser.add_argument('--recordings', type=Path, default=TEST.parent / 'ft8')
    parser.add_argument('--fc', required=True)
    parser.add_argument('--cxx', default='clang++')
    parser.add_argument('--boost-include', type=Path, required=True)
    parser.add_argument('--fftw-prefix', type=Path, required=True)
    parser.add_argument('--runtime-dir', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    source, output = args.source.resolve(), args.output.resolve()
    base = args.base_data.resolve()
    if output == base or output.is_relative_to(base):
        parser.error('choose a candidate directory outside accepted data')
    # Reuse the existing accepted protocol/waveform corpus byte-for-byte.
    # Adding channel/boundary cases never forces base-data regeneration.
    for name in ['protocol', 'audio', 'waveforms', 'provenance']:
        target = output/'ft8'/name
        if not target.exists():
            shutil.copytree(base/'ft8'/name, target)
    lib = source / 'lib'
    ft8 = lib / ('ft8v2' if args.decoder == 'jtdx' else 'ft8')
    protocol = lib / ('ft8v2' if args.decoder == 'jtdx' else '77bit')
    calls_dir, channel_dir = output/'ft8/calls', output/'ft8/channels'
    recording_dir, provenance = output/'ft8/degraded', output/'ft8/extended-provenance'
    for path in [calls_dir/f'{args.decoder}.tsv', provenance/f'{args.decoder}.json']:
        if path.exists():
            parser.error('backend already exists; use a fresh candidate directory')
    for path in [calls_dir, channel_dir, recording_dir, provenance]:
        path.mkdir(parents=True, exist_ok=True)
    inputs = [line.strip() for line in args.calls_corpus.read_text().splitlines() if line.strip()]
    boundaries = [0, 1, 2, 3, 1002, 1003, 1004, 532443, 532444,
                  2063591, 2063592, 6257895, 6257896, 268435455]
    sources = [lib/'crc.f90', lib/'packjt.f90', protocol/'packjt77.f90',
               lib/'fmtmsg.f90', lib/'grid2deg.f90', lib/'deg2grid.f90', lib/'chkcall.f90',
               ft8/'encode174_91.f90']
    if args.decoder == 'jtdx':
        sources += [lib/'ft8_mod1.f90', lib/'genft8.f90']
    else:
        sources += [ft8/'genft8.f90']
    sources += [lib/'timer_module.f90',
                lib/('gen_ft8wave.f90' if args.decoder == 'jtdx' else 'ft8/gen_ft8wave.f90'),
                lib/('gfsk_pulse.f90' if args.decoder == 'jtdx' else 'ft2/gfsk_pulse.f90'),
                lib/'fftw3mod.f90', lib/'four2a.f90',
                lib/('watterson.f90' if args.decoder == 'jtdx' else 'ft8/watterson.f90')]
    # degrade_snr is a WSJT-X utility. Emit the common real-recording corpus
    # once; every decoder adapter consumes it.
    if args.decoder == 'wsjtx':
        sources += [lib/'degrade_snr.f90']
    else:
        # JTDX doesn't carry this utility; the Watterson executable contains no
        # degradation entry, resolved by compiling that driver branch out.
        sources += []
    emitted = []
    with tempfile.TemporaryDirectory(prefix='ft8rs-extended-native-') as tmp:
        work = Path(tmp)
        flags = ['-O2', '-cpp', '-ffree-line-length-none', '-fallow-argument-mismatch',
                 '-fwrapv', '-I', ft8, '-I', lib, '-I', args.fftw_prefix.resolve()/'include']
        if args.decoder == 'jtdx':
            flags += ['-DJTDX']
        run([args.cxx, '-std=c++11', '-O2', '-I', args.boost_include.resolve(),
             '-c', lib/'crc14.cpp', '-o', 'crc14.o'], work)
        run(['clang', '-O2', '-c', lib/'gran.c', '-o', 'gran.o'], work)
        for path in sources:
            run([args.fc, *flags, '-c', path], work)
        objects = sorted(work.glob('*.o'))
        for name in ['calls', 'channels']:
            driver = TEST/f'reference/{name}.F90'
            run([args.fc, *flags, '-c', driver, '-o', f'{name}-driver.o'], work)
            link = [*objects, work/f'{name}-driver.o', args.fftw_prefix.resolve()/'lib/libfftw3f.a']
            if args.runtime_dir:
                runtime = args.runtime_dir.resolve()
                run([args.cxx, '-o', name, *link, runtime/'libgfortran.a',
                     runtime/'libquadmath.a', runtime/'libgcc.a'], work)
            else:
                run([args.fc, '-o', name, *link, '-lstdc++'], work)
        call_rows = ['# kind\tinput\tn28\tcold_text\tcold_valid\tseeded_text\tseeded_valid']
        for kind, value in [('pack', token) for token in inputs] + [('unpack', str(n)) for n in boundaries]:
            result = run([work/'calls', kind], work, input=value+'\n', text=True, capture_output=True)
            lines = result.stdout.splitlines()
            assert len(lines) == 5, (value, lines)
            call_rows.append('\t'.join([kind, value, *lines]))
        path = calls_dir/f'{args.decoder}.tsv'
        path.write_text('\n'.join(call_rows)+'\n')
        emitted.append(path)
        corpus = calls_dir/'inputs.txt'
        text = '\n'.join(inputs)+'\n'
        if corpus.exists():
            assert corpus.read_text() == text, 'shared calls corpus changed between backends'
        corpus.write_text(text)
        emitted.append(corpus)
        channel_rows = ['# id\tfile\tmessage\tfreq\tdt\tsnr\tdelay_ms\tspread_hz\tseed\trequired\tsha256']
        # Two seeds per ITU-like condition. MQ/MM mirror upstream ft8sim presets.
        # Disturbed is a diagnostic survey; no universal decode-success claim.
        for condition, delay, spread, snr, required in [
                ('mq', 0.5, 0.1, -10, 'yes'), ('mm', 1.0, 0.5, -12, 'yes'),
                ('md', 2.0, 1.0, -10, 'yes'), ('disturbed', 6.0, 10.0, -8, 'no')]:
            for seed in [2026102801, 2026102809]:
                result = run([work/'channels', 'watterson'], work,
                             input=f'{seed}\nCQ K1ABC FN42\n1000 {delay} {spread}\n',
                             text=True, capture_output=True)
                carrier = array.array('f')
                carrier.frombytes((work/'watterson.f32').read_bytes())
                assert len(carrier) == 180000
                # Match the native simulator: average signal power over NSIG,
                # then normalize to SNR in 2500 Hz. Include delayed tail power.
                power = sum(v*v for v in carrier)/151680
                gain = math.sqrt((2500/6000)*10**(snr/10)/power)
                rng = random.Random(2026102800)
                pcm = [round((rng.gauss(0, 1)+gain*v)*1000) for v in carrier]
                name = f'{args.decoder}_{condition}_{seed}.wav'
                path = channel_dir/name
                write_wav(path, pcm)
                emitted.append(path)
                channel_rows.append('\t'.join(map(str, [f'{condition}-{seed}', name,
                    result.stdout.strip(), 1000, 0, snr, delay, spread, seed, required, digest(path)])))
        path = channel_dir/f'{args.decoder}.tsv'
        path.write_text('\n'.join(channel_rows)+'\n')
        emitted.append(path)
        recordings = []
        if args.decoder == 'wsjtx':
            manifest = ['# id\tfile\tsource\tdb\tbw\tseed\tsource_sha256\tsha256']
            for stem, levels in [('210703_133430', [0.5, 1.0, 2.0, 3.0]), ('230208_140300', [1.0, 3.0])]:
                original = args.recordings.resolve()/f'{stem}.wav'
                with wave.open(str(original), 'rb') as wav:
                    assert (wav.getnchannels(), wav.getsampwidth(), wav.getframerate()) == (1, 2, 12000)
                    raw = wav.readframes(wav.getnframes())
                pcm = array.array('h')
                pcm.frombytes(raw)
                if sys.byteorder != 'little':
                    pcm.byteswap()
                (work/'input.pcm').write_bytes(pcm.tobytes())
                for level in levels:
                    seed = 2026102801
                    run([work/'channels', 'degrade'], work,
                        input=f'{seed}\n{len(pcm)} {level} 6000\n', text=True)
                    degraded = array.array('h')
                    degraded.frombytes((work/'degraded.pcm').read_bytes())
                    name = f'{stem}_plus_{level:g}db.wav'
                    path = recording_dir/name
                    write_wav(path, degraded)
                    emitted.append(path)
                    manifest.append('\t'.join(map(str, [name[:-4], name, stem, level, 6000, seed, digest(original), digest(path)])))
                recordings.append({'file': original.name, 'sha256': digest(original)})
            path = recording_dir/'recordings.tsv'
            path.write_text('\n'.join(manifest)+'\n')
            emitted.append(path)
    recorded_sources = sources+[lib/'crc14.cpp', lib/'gran.c', lib/'pfx.f90', ft8/'ldpc_174_91_c_generator.f90']
    metadata = {
        'decoder': args.decoder, 'revision': run(['git', '-C', source, 'rev-parse', 'HEAD'], output, text=True, capture_output=True).stdout.strip(),
        'compiler': run([args.fc, '--version'], output, text=True, capture_output=True).stdout.splitlines()[0],
        'compile_flags': [str(flag) for flag in flags],
        'python': sys.version, 'platform': sys.platform, 'native_byteorder': sys.byteorder,
        'macos_deployment_target': os.environ.get('MACOSX_DEPLOYMENT_TARGET'),
        'calls_cases': len(inputs)+len(boundaries), 'channel_cases': 8,
        'calls_corpus_sha256': digest(args.calls_corpus), 'recordings': recordings,
        'sources': {str(p.relative_to(source)): digest(p) for p in recorded_sources},
        'generator_sha256': digest(Path(__file__)),
        'drivers': {name: digest(TEST/f'reference/{name}.F90') for name in ['calls', 'channels']},
        'fftw_library_sha256': digest(args.fftw_prefix/'lib/libfftw3f.a'),
        'fftw_header_sha256': digest(args.fftw_prefix/'include/fftw3.h'),
        'seed_contract': 'unchanged native gran.c + host C srand/rand; fresh process per case; record platform; Python AWGN uses MT19937 seed 2026102800',
        'degradation_contract': 'unchanged WSJT-X degrade_snr, per <=180000-sample slot, bw=6000; nominal total-audio degradation, not individual-signal SNR',
        'artifacts': {str(p.relative_to(output)): digest(p) for p in emitted},
    }
    if args.calls_corpus.resolve().is_relative_to(source):
        metadata['sources'][str(args.calls_corpus.resolve().relative_to(source))] = digest(args.calls_corpus)
    (provenance/f'{args.decoder}.json').write_text(json.dumps(metadata, indent=2)+'\n')
    print(f'{args.decoder}: calls={len(inputs)+len(boundaries)} Watterson=8 real_recordings={len(recordings)}')


if __name__ == '__main__':
    main()
