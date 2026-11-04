//! Current native references supplement, rather than replace, the 3.0 corpus.
use super::{data, protocol_adapters::Backend};

fn rows(file: &str) -> Vec<Vec<String>> {
    data::read(&format!("ft8/upstream/wsjtx-3.3/{file}.tsv"))
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(|line| line.split('\t').map(String::from).collect())
        .collect()
}

pub fn current_pack(backend: Backend, input: &str, legacy: &[u8]) -> Vec<u8> {
    if matches!(backend, Backend::Jtdx) {
        return legacy.to_vec();
    }
    let row = rows("protocol")
        .into_iter()
        .find(|row| row[0] == input)
        .unwrap();
    row[2].bytes().map(|b| b - b'0').collect()
}

pub fn current_receive(backend: Backend, bits: &[u8], legacy: Option<&str>) -> Option<String> {
    if matches!(backend, Backend::Jtdx) {
        return legacy.map(String::from);
    }
    let key: String = bits.iter().map(|b| (b'0' + b) as char).collect();
    let row = rows("legacy-receive")
        .into_iter()
        .find(|row| row[0] == key)
        .unwrap();
    (row[2] == "1").then(|| row[1].clone())
}

#[test]
fn current_native_strict_codec_and_compatibility_boundary() {
    use crate::decode::packjt77::{pack77, pack77_strict, unpack77, HashCallBook};
    let native_book = || {
        let book = HashCallBook::new();
        for call in data::read("ft8/protocol/calls.txt").lines() {
            book.save(call);
        }
        book
    };
    for row in rows("protocol") {
        let book = native_book();
        assert_eq!(row.len(), 5);
        let bits = |s: &str| s.bytes().map(|b| b - b'0').collect::<Vec<_>>();
        assert_eq!(
            pack77_strict(&row[0]),
            (!row[1].is_empty()).then(|| bits(&row[1])),
            "strict {}",
            row[0]
        );
        let packed = pack77(&row[0]);
        assert_eq!(packed, bits(&row[2]), "compatibility {}", row[0]);
        let expected = (row[4] == "1").then_some(row[3].as_str());
        assert_eq!(
            unpack77(&packed, Some(&book)).as_deref(),
            expected,
            "receive {}",
            row[0]
        );
    }
    for row in rows("legacy-receive") {
        let book = native_book();
        let bits: Vec<_> = row[0].bytes().map(|b| b - b'0').collect();
        let expected = (row[2] == "1").then_some(row[1].as_str());
        assert_eq!(
            unpack77(&bits, Some(&book)).as_deref(),
            expected,
            "native wire {}",
            row[0]
        );
    }
}

#[test]
fn rejected_native_payloads_do_not_mutate_hash_knowledge() {
    use crate::decode::packjt77::{ihashcall, pack77, unpack77, HashCallBook};
    let book = HashCallBook::new();
    // A valid CQ/grid prefix, with a report substituted for the grid: the
    // native receiver rejects it after resolving the full callsign.
    let mut bits = pack77("CQ K1ABC FN42");
    let report = 32435usize;
    for i in 0..15 {
        bits[59 + i] = ((report >> (14 - i)) & 1) as u8;
    }
    assert!(unpack77(&bits, Some(&book)).is_none());
    assert!(book.lookup22(ihashcall("K1ABC", 22)).is_none());
    assert!(unpack77(&bits[..76], Some(&book)).is_none());
    bits[0] = 2;
    assert!(unpack77(&bits, Some(&book)).is_none());
}
