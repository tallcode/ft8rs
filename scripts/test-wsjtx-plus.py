#!/usr/bin/env python3
"""Build once, then replay original/reference/optimized serially with private CallDB.

Writes raw bit snapshots, paired timing summaries and opt-in hotspot profiles.
No fixtures, annotations, live databases or accepted expectations are updated.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import resource
import shutil
import statistics
import struct
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
CORE = ROOT / "crates/ft8rs-core"
PLUS = CORE / "src/decode/lib_wsjtx_plus"
TEST = "decode::lib_wsjtx_plus::validation::paired_corpus_replay"
PACKED_SYNC_ABS_TOLERANCE = 1e-9
COMPATIBILITY_TRUTH = {
    ("230208_140300_plus_1db", 3, "YO3GOD OY1DZ RR73"),
    ("230208_140300_plus_1db", 8, "I3EVK VE7ON RR73"),
    ("230208_140300_plus_1db", 11, "CQ OH8CMS KP24"),
    ("230208_140300_plus_1db", 13, "PD4I RL6LS RR73"),
    ("230208_140300_plus_1db", 13, "RJ3DC OE9GWV RR73"),
    ("230208_140300_plus_3db", 11, "CQ OH8CMS KP24"),
}


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def cpu_description():
    if platform.system() == "Darwin":
        result = subprocess.run(["sysctl", "-n", "machdep.cpu.brand_string"], text=True, capture_output=True)
        if result.returncode == 0:
            return result.stdout.strip()
        return {"architecture": platform.machine(), "model_unavailable": result.stderr.strip()}
    return platform.processor()


def protect_originals():
    origin = json.loads((PLUS / "origin.json").read_text())
    protected_paths = set()
    for directory in ["decode/lib_wsjtx", "decode/lib_jtdx", "decode/hybrid", "decode/dx", "util", "stream"]:
        protected_paths.update(str(p.relative_to(ROOT)) for p in (CORE / "src" / directory).rglob("*.rs"))
    protected_paths.add("crates/ft8rs-core/src/decode/gf2.rs")
    assert protected_paths == set(origin["protected"]), "protected source inventory changed"
    profile_wiring = {f"crates/ft8rs-core/src/stream/{name}.rs" for name in ("profile", "session", "slot")}
    for relative, expected in origin["protected"].items():
        if relative in profile_wiring:
            continue
        assert digest(ROOT / relative) == expected, f"protected source changed: {relative}"
    return origin["base_commit"]


def read_rows(path):
    return [json.loads(line) for line in path.read_text().splitlines()]


def percentile(values, fraction):
    values = sorted(values)
    return values[max(0, math.ceil(len(values) * fraction) - 1)]


def summarize(runs):
    totals = [sum(slot["total_ms"] for row in run for slot in row["slots"]) for run in runs]
    slots = [slot for run in runs for row in run for slot in row["slots"]]
    times = [slot["total_ms"] for slot in slots]
    callbacks = [t for slot in slots for t in slot["callback_boundary_ms"]]
    return {
        "run_total_ms": totals,
        "median_run_total_ms": statistics.median(totals),
        "run_total_cv": statistics.stdev(totals) / statistics.mean(totals) if len(totals) > 1 else None,
        "mean_slot_ms": statistics.mean(times),
        "p95_slot_ms": percentile(times, .95),
        "max_slot_ms": max(times),
        "cold_first_case_slot_ms": [run[0]["slots"][0]["total_ms"] for run in runs],
        "max_final_stage_ms": max(s["stage_ms"][2] for s in slots),
        "max_callback_boundary_ms": max(callbacks) if callbacks else None,
        "callback_boundary_model": "stage41/47/50 ready at -3192/-1464/-600ms with sequential backlog; not a live end-to-end measurement",
    }


def per_case(runs):
    result = {}
    for index, row in enumerate(runs[0]):
        samples = [run[index] for run in runs]
        totals = [sum(s["total_ms"] for s in r["slots"]) for r in samples]
        result[row["case"]] = {
            "median_total_ms": statistics.median(totals),
            "run_total_ms": totals,
            "max_slot_ms": max(s["total_ms"] for r in samples for s in r["slots"]),
            "rows": sum(len(s["rows"]) for s in row["slots"]),
        }
    return result


def same_rows(reference, candidate, sync_atol):
    if reference == candidate:
        return True
    if not sync_atol or len(reference) != len(candidate):
        return False
    for expected, actual in zip(reference, candidate):
        if expected == actual:
            continue
        if dict(actual, sync=expected["sync"]) != expected:
            return False
        expected_sync, actual_sync = [
            struct.unpack(">d", row["sync"].to_bytes(8, "big"))[0]
            for row in (expected, actual)
        ]
        if not math.isclose(expected_sync, actual_sync, abs_tol=sync_atol, rel_tol=0):
            return False
    return True


def compare(reference, candidate, *, sync_atol=0):
    assert len(reference) == len(candidate)
    observations = 0
    for a, b in zip(reference, candidate):
        assert (a["case"], a["config"]) == (b["case"], b["config"])
        assert len(a["slots"]) == len(b["slots"])
        for slot, (x, y) in enumerate(zip(a["slots"], b["slots"])):
            assert same_rows(x["rows"], y["rows"], sync_atol), f"results differ: {a['case']} slot={slot}"
            assert same_rows(x["callbacks"], y["callbacks"], sync_atol), f"callbacks differ: {a['case']} slot={slot}"
            observations += len(x["rows"])
    return observations


def compare_compatibility(reference, candidate):
    assert len(reference) == len(candidate)
    expected = {row for row in COMPATIBILITY_TRUTH if row[0] in {r["case"] for r in reference}}
    added = []
    for baseline, recovery in zip(reference, candidate):
        assert (baseline["case"], baseline["config"]) == (recovery["case"], recovery["config"])
        assert len(baseline["slots"]) == len(recovery["slots"])
        for slot, (before, after) in enumerate(zip(baseline["slots"], recovery["slots"])):
            count = len(before["rows"])
            assert after["rows"][:count] == before["rows"], (baseline["case"], slot, "baseline output")
            assert after["callbacks"][:count] == before["callbacks"], (baseline["case"], slot, "baseline callback")
            added.extend((baseline["case"], slot, row["msg"]) for row in after["rows"][count:])
    assert sorted(added) == sorted(expected), {
        "missing": sorted(expected - set(added)),
        "unexpected": sorted(set(added) - expected),
        "actual": added,
    }
    return added


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, help="fresh result directory")
    parser.add_argument("--profile", choices=["fast", "release"], default="release")
    parser.add_argument("--repeats", type=int, default=5)
    parser.add_argument("--case", default="", help="exact corpus id, empty selects full corpus")
    parser.add_argument("--hotspots", action="store_true", help="extra untimed profiling replay")
    parser.add_argument("--compatibility", action="store_true", help="compare plus baseline with opt-in recovery and verify exact additions")
    args = parser.parse_args()
    assert args.repeats > 0
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    base = protect_originals()
    env = dict(os.environ, RUSTC_WRAPPER="")
    command = ["cargo", "test", "--offline", "--locked", "--profile", args.profile, "-p", "ft8rs", "--lib", "--no-run", "--message-format=json"]
    with (output / "build.jsonl").open("w") as stdout, (output / "build.log").open("w") as stderr:
        subprocess.run(command, cwd=ROOT, env=env, stdout=stdout, stderr=stderr, check=True)
    artifacts = [r for r in read_rows(output / "build.jsonl") if r.get("reason") == "compiler-artifact" and r.get("executable") and r["target"]["name"] == "ft8rs" and r["profile"]["test"]]
    assert len(artifacts) == 1
    original_exe = Path(artifacts[0]["executable"])
    conditions = {
        "base_commit": base,
        "profile": args.profile,
        "command": command,
        "rustc": subprocess.check_output(["rustc", "-vV"], text=True),
        "platform": platform.platform(),
        "machine": platform.machine(),
        "cpu": cpu_description(),
        "rustflags": env.get("RUSTFLAGS", ""),
        "executable_sha256": digest(original_exe),
        "runner_sha256": digest(Path(__file__)),
        "lockfile_sha256": digest(ROOT / "Cargo.lock"),
        "origin_sha256": digest(PLUS / "origin.json"),
        "calldb_sha256": digest(CORE / "ALLCALL7.TXT.2022"),
        "source_sha256": {str(p.relative_to(ROOT)): digest(p) for p in sorted(PLUS.rglob("*.rs"))},
        "repeats": args.repeats,
        "case_filter": args.case,
        "compatibility": args.compatibility,
        "timing_scope": "all three staged calls plus slot state allocation; serial, no acquisition waits or reporting I/O",
    }
    (output / "conditions.json").write_text(json.dumps(conditions, indent=2) + "\n")
    modes = ["optimized", "compatibility"] if args.compatibility else ["original", "reference", "optimized"]
    runs = {mode: [] for mode in modes}
    cpu = {mode: [] for mode in runs}
    with tempfile.TemporaryDirectory(prefix="ft8rs-plus-") as temp:
        private = Path(temp)
        exe = private / "plus-tests"
        shutil.copy2(original_exe, exe)
        shutil.copy2(CORE / "ALLCALL7.TXT.2022", private / "ALLCALL7.TXT")
        for repeat in range(args.repeats):
            rotation = repeat % len(modes)
            for mode in modes[rotation:] + modes[:rotation]:
                path = output / f"round-{repeat}-{mode}.jsonl"
                run_env = dict(env, FT8RS_PLUS_MODE=mode, FT8RS_PLUS_REPEATS="1", FT8RS_PLUS_CASE=args.case, FT8RS_PLUS_OUTPUT=str(path), FT8RS_PLUS_PROFILE="0")
                before = resource.getrusage(resource.RUSAGE_CHILDREN)
                with (output / f"round-{repeat}-{mode}.log").open("w") as log:
                    subprocess.run([str(exe), TEST, "--exact", "--ignored", "--test-threads=1", "--nocapture"], cwd=CORE, env=run_env, stdout=log, stderr=subprocess.STDOUT, check=True)
                after = resource.getrusage(resource.RUSAGE_CHILDREN)
                cpu[mode].append({"user_s": after.ru_utime-before.ru_utime, "system_s": after.ru_stime-before.ru_stime})
                runs[mode].append(read_rows(path))
                print(f"round={repeat} mode={mode} total={summarize(runs[mode])['run_total_ms'][-1]/1000:.3f}s", flush=True)
        if args.hotspots:
            run_env = dict(env, FT8RS_PLUS_MODE="optimized", FT8RS_PLUS_REPEATS="1", FT8RS_PLUS_CASE=args.case, FT8RS_PLUS_OUTPUT=str(output / "hotspots.jsonl"), FT8RS_PLUS_PROFILE="1")
            with (output / "hotspots.log").open("w") as log:
                subprocess.run([str(exe), TEST, "--exact", "--ignored", "--test-threads=1", "--nocapture"], cwd=CORE, env=run_env, stdout=log, stderr=subprocess.STDOUT, check=True)
    reference = runs["optimized" if args.compatibility else "original"][0]
    additions = []
    for mode, mode_runs in runs.items():
        for run in mode_runs:
            if mode == "compatibility":
                additions = compare_compatibility(reference, run)
            elif mode == "optimized" and not args.compatibility:
                compare(mode_runs[0], run)
                compare(reference, run, sync_atol=PACKED_SYNC_ABS_TOLERANCE)
            else:
                compare(reference, run)
    if args.hotspots:
        compare(runs["optimized"][0], read_rows(output / "hotspots.jsonl"))
    conditions["input_sha256"] = {r["input"]: digest(Path(r["input"])) for r in reference}
    (output / "conditions.json").write_text(json.dumps(conditions, indent=2) + "\n")
    summary = {mode: summarize(mode_runs) for mode, mode_runs in runs.items()}
    summary["process_cpu_including_io"] = cpu
    summary["per_case"] = {mode: per_case(mode_runs) for mode, mode_runs in runs.items()}
    summary["equivalence"] = {"cases": len(reference), "slots": sum(len(r["slots"]) for r in reference), "observations": compare(reference, runs["optimized"][0], sync_atol=0 if args.compatibility else PACKED_SYNC_ABS_TOLERANCE), "lost": 0, "added": len(additions), "metadata_or_order_changes": 0}
    if args.compatibility:
        summary["verified_additions"] = additions
    else:
        summary["equivalence"]["sync_absolute_tolerance"] = PACKED_SYNC_ABS_TOLERANCE
        summary["equivalence"]["sync_rounding_rows"] = sum(
            before["sync"] != after["sync"]
            for old_case, new_case in zip(reference, runs["optimized"][0])
            for old_slot, new_slot in zip(old_case["slots"], new_case["slots"])
            for before, after in zip(old_slot["rows"], new_slot["rows"])
        )
        summary["optimized_vs_reference_reduction"] = 1 - summary["optimized"]["median_run_total_ms"] / summary["reference"]["median_run_total_ms"]
        summary["optimized_vs_original_reduction"] = 1 - summary["optimized"]["median_run_total_ms"] / summary["original"]["median_run_total_ms"]
    protect_originals()
    (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
