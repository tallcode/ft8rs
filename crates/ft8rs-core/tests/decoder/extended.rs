//! Shared native-channel and real-recording retention checks.
use super::{
    adapters::{Adapter, ADAPTERS},
    data,
    support::{self, normal, replay},
};
use ft8rs::{SlotTimestamp, StreamDecodeConfig, StreamDecodedMessage};
use std::collections::BTreeMap;

pub fn table(relative: &str) -> Vec<Vec<String>> {
    data::read(relative)
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.split('\t').map(str::to_owned).collect())
        .collect()
}

#[test]
fn native_watterson_channels_preserve_known_content() {
    let timestamp = SlotTimestamp::parse("261028_120000").unwrap();
    let mut errors = Vec::new();
    for source in ["wsjtx", "jtdx"] {
        for fields in table(&format!("ft8/channels/{source}.tsv")) {
            assert_eq!(fields.len(), 11);
            let samples = support::audio(data::root().join("ft8/channels").join(&fields[1]));
            assert_eq!(samples.len(), 180000);
            for adapter in ADAPTERS {
                let result = replay(
                    adapter
                        .session(StreamDecodeConfig {
                            hiscall: Some("K1ABC".into()),
                            nfqso: 1000.0,
                            ..Default::default()
                        })
                        .as_mut(),
                    &timestamp,
                    &samples,
                );
                let correct = result.rows.iter().any(|row| {
                    normal(&row.msg) == normal(&fields[2])
                        && (row.freq - 1000.0).abs() <= 5.0
                        && row.dt.abs() <= 0.3
                });
                eprintln!(
                    "{} {source}/{}: correct={correct} {}",
                    adapter.name,
                    fields[0],
                    result.describe()
                );
                if fields[9] == "yes" && !correct {
                    errors.push(format!(
                        "{} {source}/{} lost known signal",
                        adapter.name, fields[0]
                    ));
                }
                for row in &result.rows {
                    if normal(&row.msg) != normal(&fields[2]) {
                        errors.push(format!(
                            "{} {source}/{} false decode {:?}",
                            adapter.name, fields[0], row.msg
                        ));
                    }
                }
            }
        }
    }
    assert!(
        errors.is_empty(),
        "Watterson failures:\n{}",
        errors.join("\n")
    );
}

pub struct Observation {
    pub slot: usize,
    pub row: StreamDecodedMessage,
    pub class: &'static str,
}

pub struct Retention {
    pub adapter: &'static str,
    pub variant: String,
    pub source: String,
    pub db: String,
    pub target_total: usize,
    pub config: StreamDecodeConfig,
    pub observations: Vec<Observation>,
    pub first_ms: Option<f64>,
    pub finished_ms: f64,
}

fn recording(fields: &[String], adapter: Adapter) -> Retention {
    assert_eq!(fields.len(), 8);
    let long = fields[2] == "230208_140300";
    eprintln!("retention replay: {} {}", adapter.name, fields[1]);
    let samples = support::audio(data::root().join("ft8/degraded").join(&fields[1]));
    let config = StreamDecodeConfig {
        nfa: if long { 200.0 } else { 100.0 },
        jtdx_threads: if long && adapter.name == "jtdx" { 4 } else { 1 },
        hiscall: adapter
            .target_only
            .then(|| if long { "UA3QNA" } else { "F5RXL" }.into()),
        mycall: (adapter.target_only && long).then(|| "F1MLZ".into()),
        ..Default::default()
    };
    let csv =
        std::fs::read_to_string(support::root().join(format!("ft8/{}.csv", fields[2]))).unwrap();
    let start = SlotTimestamp::parse(&fields[2]).unwrap();
    let mut references = BTreeMap::<(String, String), bool>::new();
    for line in csv.lines().skip(1) {
        let parts: Vec<_> = line.split(',').collect();
        let marker = parts.get(5).map_or("", |p| p.trim());
        let included = match adapter.name {
            "wsjtx" => !matches!(marker, "J" | "E"),
            "jtdx" => !matches!(marker, "W" | "E"),
            _ => marker != "E",
        } && (!adapter.target_only
            || normal(parts[4])
                .split_whitespace()
                .any(|word| Some(word) == config.hiscall.as_deref()));
        references.insert((parts[0].into(), normal(parts[4])), included);
    }
    let target_total = references.values().filter(|&&included| included).count();
    let mut session = adapter.session(config.clone());
    let mut observations = Vec::new();
    let mut first_ms = None;
    let mut finished_ms = 0.0;
    for (slot, chunk) in samples.chunks(180000).enumerate() {
        let mut padded = vec![0.0; 180000];
        padded[..chunk.len()].copy_from_slice(chunk);
        let timestamp = start.add_seconds(slot as i64 * 15);
        let result = replay(session.as_mut(), &timestamp, &padded);
        if let Some(first) = result.first_message {
            first_ms.get_or_insert(first.as_secs_f64() * 1000.0);
        }
        finished_ms += result.finished.as_secs_f64() * 1000.0;
        for row in result.rows {
            let class = match references.get(&(timestamp.format(), normal(&row.msg))) {
                Some(true) => "target",
                Some(false) => "other-reference",
                None => "unlabelled",
            };
            observations.push(Observation { slot, row, class });
        }
    }
    Retention {
        adapter: adapter.name,
        variant: fields[1].clone(),
        source: fields[2].clone(),
        db: fields[3].clone(),
        target_total,
        config,
        observations,
        first_ms,
        finished_ms,
    }
}

#[allow(dead_code)] // Explicit report-tool control; not an extra default replay.
pub fn baseline_measurements(long: bool) -> Vec<Retention> {
    let source = if long {
        "230208_140300"
    } else {
        "210703_133430"
    };
    let path = support::root().join(format!("ft8/{source}.wav"));
    let fields = vec![
        format!("{source}-control"),
        path.to_string_lossy().into_owned(),
        source.into(),
        "0".into(),
        "6000".into(),
        "0".into(),
        String::new(),
        String::new(),
    ];
    ADAPTERS
        .into_iter()
        .map(|adapter| recording(&fields, adapter))
        .collect()
}

pub fn measurements(long: bool) -> Vec<Retention> {
    let mut result = Vec::new();
    for fields in table("ft8/degraded/recordings.tsv") {
        if (fields[2] == "230208_140300") != long {
            continue;
        }
        for adapter in ADAPTERS {
            result.push(recording(&fields, adapter));
        }
    }
    result
}

#[cfg(test)]
fn assert_retained(long: bool) {
    let expected = table("ft8/degraded/retention.tsv");
    let cases = table("ft8/degraded/retention-cases.tsv");
    let mut errors = Vec::new();
    for result in measurements(long) {
        assert_eq!(
            result.config.jtdx_threads,
            if long && result.adapter == "jtdx" {
                4
            } else {
                1
            }
        );
        // Supplemental Hybrid configurations must retain every calibrated
        // Hybrid identity. Added unlabelled rows never become new goldens here.
        let required_adapter = if result.adapter == "hybrid-exp" {
            "hybrid"
        } else {
            result.adapter
        };
        let case = cases
            .iter()
            .find(|row| row[0] == required_adapter && row[1] == result.variant)
            .unwrap_or_else(|| panic!("uncalibrated case {} {}", result.adapter, result.variant));
        let required: Vec<_> = expected
            .iter()
            .filter(|row| row[0] == required_adapter && row[1] == result.variant)
            .collect();
        assert_eq!(required.len(), case[2].parse::<usize>().unwrap());
        let mut used = vec![false; result.observations.len()];
        for row in &required {
            let slot: usize = row[2].parse().unwrap();
            let frequency: f64 = row[4].parse().unwrap();
            let dt: f64 = row[5].parse().unwrap();
            let matched = result.observations.iter().enumerate().find(|(i, actual)| {
                !used[*i]
                    && actual.slot == slot
                    && normal(&actual.row.msg) == row[3]
                    && (actual.row.freq - frequency).abs() <= 5.0
                    && (actual.row.dt - dt).abs() <= 0.3
            });
            if let Some((index, _)) = matched {
                used[index] = true;
            } else {
                errors.push(format!(
                    "{} {} slot {slot}: lost {}",
                    result.adapter, result.variant, row[3]
                ));
            }
        }
        let targets = result
            .observations
            .iter()
            .filter(|o| o.class == "target")
            .count();
        let unlabelled = result
            .observations
            .iter()
            .filter(|o| o.class == "unlabelled")
            .count();
        eprintln!("{} {} (source={}, nominal +{} dB): retained {targets}/{} reference rows, required={}, unlabelled={unlabelled}, first_ms={:?}, finished_ms={:.1}",
            result.adapter,result.variant,result.source,result.db,result.target_total,required.len(),result.first_ms,result.finished_ms);
    }
    assert!(
        errors.is_empty(),
        "real-recording retention regressions:\n{}",
        errors.join("\n")
    );
}

#[test]
fn degraded_short_recordings_keep_each_required_message() {
    assert_retained(false);
}

#[test]
#[ignore = "serial optimized full long-recording noise-retention matrix"]
fn degraded_long_recordings_keep_each_required_message() {
    assert_retained(true);
}
