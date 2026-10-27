use std::path::PathBuf;

fn jtdx_source() -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("FT8RS_JTDX_SOURCE_DIR") {
        let root = PathBuf::from(root);
        assert!(root.join("lib/ft8b.f90").is_file());
        return Some(root);
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .ancestors()
        .map(|ancestor| ancestor.join("jtdx"))
        .find(|root| root.join("lib/ft8b.f90").is_file());
    assert!(
        root.is_some() || std::env::var_os("FT8RS_REQUIRE_JTDX_SOURCE").is_none(),
        "JTDX source audit is required; set FT8RS_JTDX_SOURCE_DIR"
    );
    if root.is_none() {
        eprintln!("skipping JTDX source audit: no reference checkout");
    }
    root
}

fn quoted_words(source: &str) -> Vec<String> {
    source
        .split('"')
        .skip(1)
        .step_by(2)
        .map(|word| word.trim().to_string())
        .collect()
}

#[test]
fn jtdx_contest_tables_match_reference_sections_and_multipliers() {
    let Some(root) = jtdx_source() else {
        return;
    };
    let reference = std::fs::read_to_string(root.join("lib/ft8v2/packjt77.f90")).unwrap();
    let rust = include_str!("../src/decode/lib_jtdx/ft8v2/packjt77/mod.rs");
    for (name, count) in [("csec", 85), ("cmult", 65)] {
        let fortran_data = reference
            .split_once(&format!("data {name}/"))
            .unwrap()
            .1
            .split('/')
            .next()
            .unwrap();
        let rust_data = rust
            .split_once(&format!("const {}:", name.to_uppercase()))
            .unwrap()
            .1
            .split_once("];")
            .unwrap()
            .0;
        let expected = quoted_words(fortran_data);
        assert_eq!(expected.len(), count, "reference {name} size changed");
        assert_eq!(
            quoted_words(rust_data),
            expected,
            "{name} differs from JTDX"
        );
    }
}

fn compact(source: &str) -> String {
    source
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase()
}

#[test]
fn jtdx_filter_context_gates_come_from_reference_ft8b_caller() {
    let Some(root) = jtdx_source() else {
        return;
    };
    let caller = compact(&std::fs::read_to_string(root.join("lib/ft8b.f90")).unwrap());
    let rust = compact(include_str!("../src/decode/lib_jtdx/chkfalse8.rs"));
    for (fortran, port) in [
        ("qual.lt.0.39", "context.quality<0.39"),
        ("xsnr.lt.-20.5", "context.xsnr<-20.5"),
        ("rxdt.lt.-0.5", "context.rxdt<-0.5"),
        ("rxdt.gt.1.9", "context.rxdt>1.9"),
    ] {
        assert!(caller.contains(fortran), "original caller lost {fortran}");
        assert!(rust.contains(port), "Rust caller gate lost {port}");
    }
    for ap_type in [11, 21, 40, 41] {
        assert!(caller.contains(&format!("iaptype.eq.{ap_type}")));
    }
    assert!(rust.contains("matches!(iaptype,11|21|40|41)"));
    assert!(caller.contains("iaptype.gt.0.and.iaptype.lt.4"));
    assert!(rust.contains("(1..4).contains(&iaptype)"));
    assert!(caller.contains("callchkfalse8(msg37,i3,n3,nbadcrc,iaptype,lcall1hash)"));
}
