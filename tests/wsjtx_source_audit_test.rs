use std::fs;
use std::path::{Path, PathBuf};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_to_string(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
}

fn local_wsjtx_ft8_dir() -> Option<PathBuf> {
    let root = project_root();
    let dir = root.parent()?.join("wsjtx/lib/ft8");
    dir.is_dir().then_some(dir)
}

fn skip_without_local_wsjtx() -> Option<PathBuf> {
    match local_wsjtx_ft8_dir() {
        Some(dir) => Some(dir),
        None => {
            eprintln!("skipping WSJT-X source audit: ../wsjtx/lib/ft8 is not present");
            None
        }
    }
}

#[test]
fn wsjtx_mirrored_source_paths_exist() {
    let root = project_root();

    for path in [
        "src/ft8/lib/ft8_decode.rs",
        "src/ft8/lib/ft8/ft8b.rs",
        "src/ft8/lib/ft8/ft8_a7.rs",
        "src/ft8/lib/ft8/ft8_downsample.rs",
        "src/ft8/lib/ft8/ft8_params.rs",
        "src/ft8/lib/ft8/sync8.rs",
        "src/ft8/lib/ft8/sync8d.rs",
        "src/ft8/lib/ft8/bpdecode174_91.rs",
        "src/ft8/lib/ft8/chkcrc14a.rs",
        "src/ft8/lib/ft8/decode174_91.rs",
        "src/ft8/lib/ft8/get_crc14.rs",
        "src/ft8/lib/ft8/get_spectrum_baseline.rs",
        "src/ft8/lib/ft8/ldpc_174_91_c_generator.rs",
        "src/ft8/lib/ft8/ldpc_174_91_c_parity.rs",
        "src/ft8/lib/ft8/osd174_91.rs",
        "src/ft8/lib/ft8/subtractft8.rs",
        "src/ft8/lib/77bit/packjt77.rs",
        "src/ft8/lib/indexx.rs",
        "src/ft8/lib/nuttal_window.rs",
        "src/ft8/lib/platanh.rs",
    ] {
        assert!(
            root.join(path).is_file(),
            "missing mirrored Rust path {path}"
        );
    }

    for old_path in [
        "src/ft8/ap_decode.rs",
        "src/ft8/decode",
        "src/ft8/decode174_91.rs",
        "src/ft8/pack_jt77.rs",
        "src/ft8/unpack_jt77.rs",
        "src/ft8/lib/77bit/unpack77.rs",
        "src/ft8/lib/77bit/hashcall.rs",
        "src/ft8/lib/77bit/protocol.rs",
        "src/ft8/lib/ft8/constants.rs",
        "src/ft8/lib/ft8/ft8_decode.rs",
        "src/ft8/lib/ft8/symbols.rs",
        "src/ft8/lib/ft8/sync_templates.rs",
        "src/ft8/lib/ft8/workspace.rs",
        "src/ft8/subtract_ft8.rs",
        "src/ft8/indexx.rs",
        "src/ft8/ldpc_tables.rs",
    ] {
        assert!(
            !root.join(old_path).exists(),
            "old non-mirrored path should not remain: {old_path}"
        );
    }
}

#[test]
fn wsjtx_ft8_params_constants_match_rust_params() {
    let Some(wsjtx_ft8) = skip_without_local_wsjtx() else {
        return;
    };

    let wsjtx = read_to_string(&wsjtx_ft8.join("ft8_params.f90"));
    let rust = read_to_string(&project_root().join("src/ft8/lib/ft8/ft8_params.rs"));

    for expected in [
        "parameter (KK=91)",
        "parameter (ND=58)",
        "parameter (NS=21)",
        "parameter (NN=NS+ND)",
        "parameter (NSPS=1920)",
        "parameter (NMAX=15*12000)",
        "parameter (NFFT1=2*NSPS, NH1=NFFT1/2)",
        "parameter (NSTEP=NSPS/4)",
        "parameter (NHSYM=NMAX/NSTEP-3)",
        "parameter (NDOWN=60)",
    ] {
        assert!(
            wsjtx.contains(expected),
            "WSJT-X ft8_params missing {expected}"
        );
    }

    for expected in [
        "const NSPS: usize = 1920",
        "const NFFT1: usize = 2 * NSPS",
        "const NSTEP: usize = NSPS / 4",
        "const NMAX: usize = 15 * 12_000",
        "const NHSYM: usize = NMAX / NSTEP - 3",
        "const NDOWN: usize = 60",
        "const NN: usize = 79",
    ] {
        assert!(
            rust.contains(expected),
            "Rust ft8_params missing {expected}"
        );
    }
}

#[test]
fn wsjtx_ft8_downsample_shape_matches_shared_rust_helper() {
    let Some(wsjtx_ft8) = skip_without_local_wsjtx() else {
        return;
    };

    let wsjtx = read_to_string(&wsjtx_ft8.join("ft8_downsample.f90"));
    let params = read_to_string(&project_root().join("src/ft8/lib/ft8/ft8_params.rs"));
    let rust = read_to_string(&project_root().join("src/ft8/lib/ft8/ft8_downsample.rs"));
    let ap = read_to_string(&project_root().join("src/ft8/lib/ft8/ft8_a7.rs"));

    for expected in [
        "parameter (NFFT1=192000,NFFT2=3200)",
        "df=12000.0/NFFT1",
        "baud=12000.0/NSPS",
        "i0=nint(f0/df)",
        "ft=f0+8.5*baud",
        "it=min(nint(ft/df),NFFT1/2)",
        "fb=f0-1.5*baud",
        "ib=max(1,nint(fb/df))",
        "c1(0:100)=c1(0:100)*taper(100:0:-1)",
        "c1(k-1-100:k-1)=c1(k-1-100:k-1)*taper",
        "c1=cshift(c1,i0-ib)",
        "call four2a(c1,NFFT2,1,1,1)",
        "fac=1.0/sqrt(float(NFFT1)*NFFT2)",
    ] {
        assert!(
            wsjtx.contains(expected),
            "WSJT-X ft8_downsample missing {expected}"
        );
    }

    for expected in [
        "const NFFT1_LONG: usize = 192000",
        "const NFFT2: usize = 3200",
        "const DOWNSAMPLE_DF: f32 = SAMPLE_RATE as f32 / NFFT1_LONG as f32",
        "const DOWNSAMPLE_BAUD: f32 = SAMPLE_RATE as f32 / NSPS as f32",
        "const DOWNSAMPLE_FAC: f32",
    ] {
        assert!(
            params.contains(expected),
            "Rust ft8_params missing {expected}"
        );
    }

    for expected in [
        "pub(crate) fn ft8_downsample_from_cx",
        "let i0 = nint_wsjtx_real(f0 / df).max(0) as usize",
        "let ft = f0 + 8.5f32 * baud",
        "let it = (nint_wsjtx_real(ft / df).max(0) as usize).min(NFFT1_LONG / 2)",
        "let fb = f0 - 1.5f32 * baud",
        "let ib = 1.max(nint_wsjtx_real(fb / df).max(0) as usize)",
        "let tap = taper_data[TAPER_SIZE - 1 - i]",
        "let idx = end_tap - TAPER_SIZE + 1 + i",
        "let shift = i0 as isize - ib as isize",
        "four2a_c2c(&mut cd0_re[..NFFT2], &mut cd0_im[..NFFT2], 1)",
        "((cd0_re[i] as f32) * DOWNSAMPLE_FAC) as f64",
    ] {
        assert!(
            rust.contains(expected),
            "Rust ft8_downsample missing {expected}"
        );
    }

    assert!(
        ap.contains("ft8_downsample_from_cx("),
        "AP decode must reuse the shared ft8_downsample helper"
    );
}

#[test]
fn wsjtx_sync8d_shape_matches_shared_rust_costas_sync() {
    let Some(wsjtx_ft8) = skip_without_local_wsjtx() else {
        return;
    };

    let wsjtx = read_to_string(&wsjtx_ft8.join("sync8d.f90"));
    let rust = read_to_string(&project_root().join("src/ft8/lib/ft8/sync8d.rs"));
    let ft8b = read_to_string(&project_root().join("src/ft8/lib/ft8/ft8b.rs"));
    let ap = read_to_string(&project_root().join("src/ft8/lib/ft8/ft8_a7.rs"));

    for expected in [
        "subroutine sync8d(cd0,i0,ctwk,itwk,sync)",
        "parameter(NP2=2812,NDOWN=60)",
        "data icos7/3,1,4,0,6,5,2/",
        "i1=i0+i*32",
        "i2=i1+36*32",
        "i3=i1+72*32",
        "if(itwk.eq.1) csync2=ctwk*csync2",
        "z1=sum(cd0(i1:i1+31)*conjg(csync2))",
        "sync = sync + p(z1) + p(z2) + p(z3)",
    ] {
        assert!(wsjtx.contains(expected), "WSJT-X sync8d missing {expected}");
    }

    for expected in [
        "pub(crate) fn sync8d(",
        "pub(crate) fn sync8d_twk(",
        "let stride = 36 * COSTAS_SYMBOL_LEN",
        "let mut i_start = i0 + (i as isize) * (COSTAS_SYMBOL_LEN as isize)",
        "z_re += d_re * s_re + d_im * s_im",
        "sync += z_re * z_re + z_im * z_im",
    ] {
        assert!(rust.contains(expected), "Rust sync8d missing {expected}");
    }

    assert!(
        ft8b.contains("sync8d("),
        "regular ft8b must use shared sync8d"
    );
    assert!(ap.contains("sync8d("), "AP ft8_a7d must use shared sync8d");
    assert!(
        ap.contains("sync8d_twk("),
        "AP ft8_a7d must use shared sync8d_twk"
    );
}

#[test]
fn wsjtx_osd174_91_deep_path_shape_matches_rust_osd() {
    let Some(wsjtx_ft8) = skip_without_local_wsjtx() else {
        return;
    };

    let wsjtx = read_to_string(&wsjtx_ft8.join("osd174_91.f90"));
    let rust = read_to_string(&project_root().join("src/ft8/lib/ft8/osd174_91.rs"));

    for expected in [
        "subroutine osd174_91(llr,k,apmask,ndeep,message91,cw,nhardmin,dmin)",
        "if(ndeep.gt.6) ndeep=6",
        "elseif(ndeep.eq.3) then",
        "npre2=1",
        "ntau=14",
        "elseif(ndeep.eq.4) then",
        "ntau=17",
        "elseif(ndeep.eq.5) then",
        "ntau=15",
        "if(npre2.eq.1) then",
        "call boxit91(reset,mi(1:ntau),ntau,ntotal,i1,i2)",
        "call fetchit91(reset,r2pat(1:ntau),ntau,in1,in2)",
    ] {
        assert!(
            wsjtx.contains(expected),
            "WSJT-X osd174_91 missing {expected}"
        );
    }

    for expected in [
        "3 => (1usize, true, true, 40usize, 12usize, 14usize)",
        "4 => (2usize, true, true, 40usize, 12usize, 17usize)",
        "5 => (3usize, true, true, 40usize, 12usize, 15usize)",
        "if npre2 {",
        "boxit91_pattern(&genmrb, n, k, ntau, i1, i2)",
        "fetchit91_pattern(&e2sub[..ntau])",
        "mi[in1] = 1",
        "mi[in2] = 1",
        "fn mrbencode91(",
    ] {
        assert!(
            rust.contains(expected),
            "Rust decode174_91 OSD missing {expected}"
        );
    }
}
