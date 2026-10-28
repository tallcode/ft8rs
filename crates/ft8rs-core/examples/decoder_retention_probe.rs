//! Explicit calibration/report tool. It writes candidate observations, never
//! accepted expectations. Reuses exactly the integration tests' decoder runner.
#[allow(dead_code)]
#[path = "../tests/decoder/adapters.rs"]
mod adapters;
#[path = "../tests/decoder/data.rs"]
mod data;
#[path = "../tests/decoder/extended.rs"]
mod extended;
#[allow(dead_code)]
#[path = "../tests/decoder/random.rs"]
mod random;
#[allow(dead_code)]
#[path = "../tests/decoder/support.rs"]
mod support;

fn main() {
    use std::io::Write;
    let output = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .expect("usage: decoder_retention_probe <fresh-candidate-report-directory>");
    assert!(!output.exists(), "choose a fresh report directory");
    std::fs::create_dir_all(&output).unwrap();
    let mut registry = std::fs::File::create(output.join("adapters.tsv")).unwrap();
    writeln!(
        registry,
        "# name\ttarget_only\tnative_baseline\tpreserves_native\tsupports_a8"
    )
    .unwrap();
    for adapter in adapters::ADAPTERS {
        writeln!(
            registry,
            "{}\t{}\t{}\t{}\t{}",
            adapter.name,
            adapter.target_only,
            adapter.native_baseline,
            adapter.preserves_native,
            adapter.supports_a8
        )
        .unwrap();
    }
    let mut rows = std::fs::File::create(output.join("observations.tsv")).unwrap();
    let mut curves = std::fs::File::create(output.join("curves.tsv")).unwrap();
    writeln!(rows, "# adapter\tfile\tslot\tmessage\tfreq\tdt\tclass").unwrap();
    writeln!(
        curves,
        "# adapter\tfile\tsource\tdb\tmatched\ttotal\tunlabelled\tfirst_nonempty_slot_callback_ms\tsummed_slot_work_ms"
    )
    .unwrap();
    let baseline = output.join("baseline");
    std::fs::create_dir(&baseline).unwrap();
    for long in [false, true] {
        let mut results = extended::baseline_measurements(long);
        results.extend(extended::measurements(long));
        for result in results {
            if result.db == "0" {
                let mut control = std::fs::File::create(
                    baseline.join(format!("{}-{}.tsv", result.adapter, result.source)),
                )
                .unwrap();
                writeln!(control, "# config={:?}", result.config).unwrap();
                let mut ordered: Vec<_> = result.observations.iter().collect();
                ordered.sort_by(|a, b| {
                    a.slot
                        .cmp(&b.slot)
                        .then_with(|| a.row.msg.cmp(&b.row.msg))
                        .then_with(|| a.row.freq.total_cmp(&b.row.freq))
                });
                for actual in ordered {
                    writeln!(
                        control,
                        "{}\t{}\t{:.9}\t{:.9}\t{:.9}",
                        actual.slot,
                        support::normal(&actual.row.msg),
                        actual.row.freq,
                        actual.row.dt,
                        actual.row.snr
                    )
                    .unwrap();
                }
            }
            let matched = result
                .observations
                .iter()
                .filter(|r| r.class == "target")
                .count();
            let unlabelled = result
                .observations
                .iter()
                .filter(|r| r.class == "unlabelled")
                .count();
            for actual in &result.observations {
                writeln!(
                    rows,
                    "{}\t{}\t{}\t{}\t{:.9}\t{:.9}\t{}",
                    result.adapter,
                    result.variant,
                    actual.slot,
                    support::normal(&actual.row.msg),
                    actual.row.freq,
                    actual.row.dt,
                    actual.class
                )
                .unwrap();
            }
            writeln!(
                curves,
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:?}\t{:.3}",
                result.adapter,
                result.variant,
                result.source,
                result.db,
                matched,
                result.target_total,
                unlabelled,
                result.first_ms,
                result.finished_ms
            )
            .unwrap();
            eprintln!(
                "{} {}: {matched}/{} reference targets; {unlabelled} unlabelled",
                result.adapter, result.variant, result.target_total
            );
        }
    }
}
