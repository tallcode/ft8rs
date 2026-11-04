#!/usr/bin/env python3
"""Freeze versioned native payloads for the fixed DX waveform regressions."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile

MESSAGES = ['K1JT BG5ATV PM00', 'K1JT BG5ATV -04', 'K1JT BG5ATV R-07',
            'K1JT BG5ATV RR73', 'K1JT BG5ATV 73', 'CQ BG5ATV PM00',
            'K1JT BG5ATV -16', 'K1JT BG5ATV R-12', 'CQ K1ABC FN42',
            'CQ W9XYZ EN50', 'CQ G4AAA IO91']
DRIVER = '''program fixed_payload
  use packjt77
  implicit none
  character(len=37) :: msg,sent
  integer :: i3,n3,tones(79)
  integer(kind=1) :: bits(77)
  read(*,'(a)') msg
  call genft8(msg,i3,n3,sent,bits,tones)
  write(*,'(a)') trim(sent)
  write(*,'(77i1)') bits
  write(*,'(79i1)') tones
end program fixed_payload
'''

def digest(p):
    return hashlib.sha256(p.read_bytes()).hexdigest()


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    for name in ['source', 'fc', 'boost-include', 'runtime-dir', 'output']:
        ap.add_argument('--' + name, type=Path, required=True)
    ap.add_argument('--variant', choices=['legacy', 'current'], default='current')
    args = ap.parse_args()
    source, output = args.source.resolve(), args.output.resolve()
    assert not output.exists(), 'choose a fresh output directory'
    names = ['crc.f90', 'packjt.f90']
    if args.variant == 'current':
        names += ['77bit/packjt77_schema.f90', '77bit/packjt77_grammar.f90']
    names += ['77bit/packjt77.f90']
    if args.variant == 'current':
        names += ['77bit/decoder_codec_context.f90', 'stdcall_suffix.f90']
    names += ['fmtmsg.f90', 'grid2deg.f90', 'deg2grid.f90', 'chkcall.f90',
              'ft8/encode174_91.f90', 'ft8/genft8.f90']
    files = [source / 'lib' / name for name in names]
    extra_sources = []
    flags = ['-O2', '-cpp', '-ffree-line-length-none', '-fallow-argument-mismatch', '-fwrapv',
             '-I', str(source / 'lib'), '-I', str(source / 'lib/ft8')]
    with tempfile.TemporaryDirectory(prefix='ft8rs-dx-payload-') as temp:
        work = Path(temp)
        def run(cmd, **kw):
            return subprocess.run([str(x) for x in cmd], cwd=work, check=True, **kw)
        (work / 'driver.f90').write_text(DRIVER)
        extra_files = []
        if args.variant == 'current':
            stdcall = source / 'lib/qra/q65/q65_set_list.f90'
            text = stdcall.read_text()
            start = text.index('subroutine stdcall(')
            end = text.index('end subroutine stdcall', start) + len('end subroutine stdcall')
            (work / 'stdcall.f90').write_text(text[start:end] + '\n')
            extra_files.append(work / 'stdcall.f90')
            extra_sources.append(stdcall)
        run(['clang++', '-O2', '-I', args.boost_include.resolve(), '-c', source / 'lib/crc14.cpp', '-o', 'crc14.o'])
        for p in files + extra_files + [work / 'driver.f90']:
            run([args.fc.resolve(), *flags, '-c', p])
        run(['clang++', '-o', 'probe', *sorted(work.glob('*.o')),
             *[args.runtime_dir.resolve() / n for n in ['libgfortran.a', 'libquadmath.a', 'libgcc.a']]])
        rows = ['# message\tbits77\ttones79']
        for message in MESSAGES:
            sent, bits, tones = run([work / 'probe'], input=message+'\n', text=True, capture_output=True).stdout.splitlines()
            assert sent.strip() == message and len(bits) == 77 and len(tones) == 79
            if message == 'K1JT BG5ATV RR73':
                expected = 32373 if args.variant == 'legacy' else 32403
                assert int(bits[59:74], 2) == expected, 'native source does not match selected RR73 encoding'
            rows.append('\t'.join([message, bits, tones]))
        output.mkdir(parents=True)
        stem = 'dx-payloads' if args.variant == 'legacy' else 'dx-current-payloads'
        p = output / (stem + '.tsv')
        p.write_text('\n'.join(rows)+'\n')
        metadata = {'kind':f'Frozen {args.variant} native DX synthetic waveform inputs',
                    'rr73_field':32373 if args.variant == 'legacy' else 32403,
                    'revision':run(['git','-C',source,'rev-parse','HEAD'],text=True,capture_output=True).stdout.strip(),
                    'sources':{str(p.relative_to(source)):digest(p) for p in files+extra_sources+[source/'lib/crc14.cpp',source/'lib/ft8/ldpc_174_91_c_generator.f90']},
                    'compiler':run([args.fc.resolve(),'--version'],text=True,capture_output=True).stdout.splitlines()[0],
                    'flags':flags,'driver_sha256':hashlib.sha256(DRIVER.encode()).hexdigest(),
                    'generator_sha256':digest(Path(__file__)), 'payload_sha256':digest(p),'cases':len(MESSAGES)}
        (output / (stem + '.provenance.json')).write_text(json.dumps(metadata,indent=2)+'\n')
        print(f'Captured {len(MESSAGES)} {args.variant} native payloads and tones')


if __name__ == '__main__':
    main()
