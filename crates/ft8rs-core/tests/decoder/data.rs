//! Default tests use committed oracles. An explicit root allows validating a
//! regenerated candidate without replacing any accepted fixture.
use std::path::PathBuf;

pub fn root() -> PathBuf {
    std::env::var_os("FT8RS_DECODER_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/decoder/data"))
}

pub fn read(relative: &str) -> String {
    let path = root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}
