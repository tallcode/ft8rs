#!/usr/bin/env python3
"""Capture codec and A8 references from unchanged, pinned WSJT-X sources.

Keep the older full protocol/waveform corpus intact. New references explicitly
record current encoding and the current native interpretation of legacy bits.
"""
import argparse
import hashlib
import json
import os
import platform
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
TEST = ROOT / 'crates/ft8rs-core/tests/decoder'


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(command, work, **kwargs):
    return subprocess.run([str(x) for x in command], cwd=work, check=True, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--fc', type=Path, required=True)
    parser.add_argument('--boost-include', type=Path, required=True)
    parser.add_argument('--runtime-dir', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    source, output = args.source.resolve(), args.output.resolve()
    if output.exists():
        parser.error('choose a new output directory')
    output.mkdir(parents=True)
    names = ['crc.f90', 'packjt.f90', '77bit/packjt77_schema.f90',
             '77bit/packjt77_grammar.f90', '77bit/packjt77.f90',
             '77bit/decoder_codec_context.f90', 'fmtmsg.f90', 'grid2deg.f90',
             'deg2grid.f90', 'chkcall.f90', 'ft8/encode174_91.f90',
             'ft8/genft8.f90', 'stdcall_suffix.f90']
    files = [source / 'lib' / name for name in names]
    driver = TEST / 'reference/upstream.F90'
    upstream_getmsg = source / 'lib/ft8/ft8_a7.f90'
    text = upstream_getmsg.read_text()
    start = text.index('subroutine getmsg(')
    end = text.index('end subroutine getmsg', start) + len('end subroutine getmsg')
    getmsg_source = text[start:end] + '\n'
    a7_start = text.index("     msg=trim(call_1)//' '//trim(call_2)", text.index('subroutine ft8_a7d('))
    a7_end = text.index('     i3=-1', a7_start)
    a7_source = text[a7_start:a7_end]
    a7_wrapper = """subroutine capture_a7_messages()
  implicit none
  character(len=12) :: call_1,call_2
  character(len=4) :: grid4
  character(len=37) :: msg
  logical :: std_1,std_2
  integer :: imsg,i,j0,isnr
  read(*,'(a)') call_1
  read(*,'(a)') call_2
  read(*,'(a)') grid4
  call stdcall(call_1,std_1)
  if(call_1(1:3).eq.'CQ ') std_1=.true.
  call stdcall(call_2,std_2)
  do imsg=1,206
""" + a7_source + """    write(*,'(i0,a,a)') imsg,achar(9),trim(msg)
  enddo
end subroutine capture_a7_messages
"""
    stdcall_path = source / 'lib/qra/q65/q65_set_list.f90'
    text = stdcall_path.read_text()
    start = text.index('subroutine stdcall(')
    end = text.index('end subroutine stdcall', start) + len('end subroutine stdcall')
    stdcall_source = text[start:end] + '\n'
    legacy = [line.split('\t') for line in (TEST / 'data/ft8/protocol/wsjtx.tsv').read_text().splitlines()
              if line and not line.startswith('#')]
    extra = [
        'K1ABC W9XYZ RR73', 'K1ABC W9XYZ/R', 'K1ABC W9XYZ/P FN42',
        'K1ABC/R W9XYZ/P FN42', 'CQ N0CALL FN42', 'CQ K1ABC -10', 'CQ K1ABC RR73',
        'CQ K1ABC R FN42', 'CQ 123 K1ABC FN42', 'CQ DX PJ4/KA1ABC FN42',
        'K1ABC W9XYZ R 6A WI', '0123456789ABCDEF01',
        '<PA9XYZ> <G4ABC/P> R 592047 IO91NP',
        '<PA9XYZ> <G4ABC/P> R 592048 IO91NP',
        '<PA9XYZ> <G4ABC/P> 590000 IO91NP',
        '<PA9XYZ> <G4ABC/P> X 590003 IO91NP',
        'K1ABC W9XYZ 579 0013', 'K1ABC W9XYZ 579 13',
        '<K1ABC> W9XYZ/R', 'K1ABC <W9XYZ/R> -10',
    ]
    inputs = list(dict.fromkeys([row[0] for row in legacy] + extra))
    wire_cases = [row[2] for row in legacy]
    def replace_field(bits, start, width, value):
        return bits[:start] + format(value, f'0{width}b') + bits[start+width:]
    type1 = next(row[2] for row in legacy if row[0] == 'K1ABC W9XYZ EN37')
    type3 = next(row[2] for row in legacy if row[2][-3:] == '011')
    type5 = next(row[2] for row in legacy if row[2][-3:] == '101')
    wire_cases += [replace_field(type1, 59, 15, 32767),
                   replace_field(type3, 1, 28, 2),
                   replace_field(type3, 1, 28, 0),
                   replace_field(type3, 61, 13, 8000),
                   replace_field(type5, 38, 11, 0),
                   '0' * 71 + '101000', '1' * 77]
    calls = (TEST / 'data/ft8/protocol/calls.txt').read_text()
    with tempfile.TemporaryDirectory(prefix='wsjtx-upstream-reference-') as temp:
        work = Path(temp)
        (work / 'calls.txt').write_text(calls)
        (work / 'getmsg.f90').write_text(getmsg_source + '\n' + stdcall_source + '\n' + a7_wrapper)
        run(['clang++', '-O2', '-I', args.boost_include.resolve(), '-c',
             source / 'lib/crc14.cpp', '-o', 'crc14.o'], work)
        flags = ['-O2', '-cpp', '-ffree-line-length-none', '-fallow-argument-mismatch',
                 '-fwrapv', '-I', source / 'lib', '-I', source / 'lib/ft8']
        for path in files + [work / 'getmsg.f90', driver]:
            run([args.fc.resolve(), *flags, '-c', path], work)
        runtime = args.runtime_dir.resolve()
        run(['clang++', '-o', 'probe', *sorted(work.glob('*.o')),
             runtime / 'libgfortran.a', runtime / 'libquadmath.a', runtime / 'libgcc.a'], work)

        def probe(mode, value):
            lines = run([work / 'probe', mode], work, input=value + '\n',
                        text=True, capture_output=True).stdout.splitlines()
            assert len(lines) == 3, (mode, value, lines)
            valid, bits, message = lines
            return valid == 'T', bits if set(bits) <= {'0', '1'} else '', message.strip()

        rows = ['# input\tstrict_bits\tcompatibility_bits\tdecoded_compatibility\tvalid']
        for value in inputs:
            _, strict, _ = probe('strict', value)
            valid, compatibility, decoded = probe('legacy', value)
            rows.append('\t'.join([value, strict, compatibility, decoded, str(int(valid))]))
        (output / 'protocol.tsv').write_text('\n'.join(rows) + '\n')
        rows = ['# legacy_bits\tdecoded_current_native\tvalid']
        for value in wire_cases:
            valid, bits, decoded = probe('unpack', value)
            rows.append('\t'.join([bits, decoded, str(int(valid))]))
        (output / 'legacy-receive.tsv').write_text('\n'.join(rows) + '\n')
        rows = ['# mycall\tdxcall\tgrid\tindex\tmessage']
        for mycall, dxcall in [('K1ABC', 'W9XYZ'), ('K1ABC', 'W9XYZ/R'),
                               ('K1ABC', 'W9XYZ/P'), ('PJ4/K1ABC', 'W9XYZ/R'),
                               ('K1ABC/R', 'W9XYZ/P'), ('K1ABC', 'PJ4/W9XYZ')]:
            result = run([work / 'probe', 'a8'], work,
                         input=f'{mycall}\n{dxcall}\nEN37\n', text=True, capture_output=True)
            lines = result.stdout.splitlines()
            assert len(lines) == 411
            rows += ['\t'.join([mycall, dxcall, 'EN37', line]) for line in lines]
        (output / 'a8.tsv').write_text('\n'.join(rows) + '\n')
        rows = ['# call1\tcall2\tgrid\tindex\tmessage']
        for call1, call2, grid in [('CQ', 'D1DX', 'KN87'), ('CQ', 'OH8CMS', 'KP24'),
                                   ('K1ABC', 'W9XYZ', 'EN37'), ('K1ABC', 'W9XYZ/R', 'EN37'),
                                   ('EA5/DH0YAH', 'RK4FF/P', 'JN00'), ('CQ_DX', 'OH8CMS', 'KP24')]:
            result = run([work / 'probe', 'a7'], work,
                         input=f'{call1}\n{call2}\n{grid}\n', text=True, capture_output=True)
            lines = result.stdout.splitlines()
            assert len(lines) == 206
            rows += ['\t'.join([call1, call2, grid, line]) for line in lines]
        (output / 'a7.tsv').write_text('\n'.join(rows) + '\n')
    metadata = {
        'revision': run(['git', '-C', source, 'rev-parse', 'HEAD'], ROOT,
                        text=True, capture_output=True).stdout.strip(),
        'compiler': run([args.fc.resolve(), '--version'], ROOT,
                        text=True, capture_output=True).stdout.splitlines()[0],
        'flags': [str(x) for x in flags],
        'sources': {str(p.relative_to(source)): digest(p)
                    for p in files + [source / 'lib/crc14.cpp', upstream_getmsg, stdcall_path,
                        source / 'lib/ft8/ldpc_174_91_c_generator.f90']},
        'decoder_sources': {str(p.relative_to(source)): digest(p) for p in [
            source / 'lib/ft8_decode.f90', source / 'lib/four2a.f90',
            *[source / 'lib/ft8' / name for name in [
                'ft8_a8d.f90', 'ft8apset.f90', 'ft8b.f90', 'sync8.f90',
                'sync8d.f90', 'ft8_downsample.f90', 'subtractft8.f90',
                'decode174_91.f90', 'bpdecode174_91.f90', 'osd174_91.f90',
                'gen_ft8wave.f90', 'ft8_waveform.f90']]]},
        'a7_message_source_sha256': hashlib.sha256(a7_source.encode()).hexdigest(),
        'getmsg_extracted_sha256': hashlib.sha256(getmsg_source.encode()).hexdigest(),
        'stdcall_extracted_sha256': hashlib.sha256(stdcall_source.encode()).hexdigest(),
        'driver_sha256': digest(driver), 'generator_sha256': digest(Path(__file__)),
        'files': {p.name: digest(p) for p in output.glob('*.tsv')},
        'legacy_protocol_sha256': digest(TEST / 'data/ft8/protocol/wsjtx.tsv'),
        'boost_crc_sha256': digest(args.boost_include / 'boost/crc.hpp'),
        'boost_config_sha256': digest(args.boost_include / 'boost/config.hpp'),
        'platform': platform.platform(),
        'macos_deployment_target': os.environ.get('MACOSX_DEPLOYMENT_TARGET'),
        'cxx': run(['clang++', '--version'], ROOT, text=True, capture_output=True).stdout.splitlines()[0],
        'static_runtimes': {name: digest(args.runtime_dir / name)
                            for name in ['libgfortran.a', 'libquadmath.a', 'libgcc.a']},
    }
    (output / 'provenance.json').write_text(json.dumps(metadata, indent=2) + '\n')
    print(f'Captured {len(inputs)} codec cases, {len(wire_cases)} receive cases, 2466 A8 and 1236 A7 hypotheses')


if __name__ == '__main__':
    main()
