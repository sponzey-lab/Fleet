#!/usr/bin/env python3
"""Record and verify the deterministic seven-sample catalog benchmark."""

import argparse
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import sys
import time


def load_payload(path: Path, source: str) -> dict:
    try:
        with path.open(encoding="utf-8") as payload_file:
            return json.load(payload_file)
    except OSError as error:
        raise SystemExit(f"cannot read catalog performance {source}: {error}") from error
    except json.JSONDecodeError as error:
        raise SystemExit(f"cannot parse catalog performance {source}: {error}") from error


def validate(payload: dict, source: str) -> None:
    if payload.get("schema_version") != 1:
        raise SystemExit(f"{source} has unsupported schema_version")
    runner = payload.get("runner")
    runner_fields = ("label", "image_os", "image_version")
    if not isinstance(runner, dict) or not all(
        isinstance(runner.get(field), str) and runner[field] for field in runner_fields
    ):
        raise SystemExit(f"{source} has incomplete runner metadata")
    if "latest" in runner["label"]:
        raise SystemExit(f"{source} uses a non-pinned runner label")
    if not isinstance(payload.get("rustc_version"), str) or not payload["rustc_version"]:
        raise SystemExit(f"{source} has no rustc version")
    samples = payload.get("samples_ms")
    if (
        not isinstance(samples, list)
        or len(samples) != 7
        or any(not isinstance(value, (int, float)) or value < 0 for value in samples)
    ):
        raise SystemExit(f"{source} must contain exactly seven non-negative samples")
    median = payload.get("median_ms")
    if (
        not isinstance(median, (int, float))
        or abs(median - statistics.median(samples)) > 0.001
    ):
        raise SystemExit(f"{source} median does not match its samples")


def record(args: argparse.Namespace) -> None:
    if "latest" in args.runner_label:
        raise SystemExit(
            "catalog performance record requires an explicit non-latest runner label"
        )
    command = [
        "cargo",
        "test",
        "-q",
        "-p",
        "fleet-application",
        "one_thousand_document_sync_keeps_ready_and_checksum_reuse_paths_deterministic",
        "--lib",
    ]
    samples = []
    for _ in range(7):
        started = time.perf_counter_ns()
        subprocess.run(command, check=True)
        samples.append(round((time.perf_counter_ns() - started) / 1_000_000, 3))
    payload = {
        "schema_version": 1,
        "runner": {
            "label": args.runner_label,
            "image_os": os.environ.get("ImageOS", platform.system()),
            "image_version": os.environ.get("ImageVersion", "unrecorded"),
        },
        "rustc_version": subprocess.check_output(
            ["rustc", "--version"], text=True
        ).strip(),
        "samples_ms": samples,
        "median_ms": round(statistics.median(samples), 3),
    }
    report = Path(args.report)
    report.parent.mkdir(parents=True, exist_ok=True)
    report.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(json.dumps(payload, sort_keys=True))


def verify(args: argparse.Namespace) -> None:
    baseline = load_payload(Path(args.baseline), "baseline")
    report = load_payload(Path(args.report), "report")
    validate(baseline, "baseline")
    validate(report, "report")
    if baseline["runner"] != report["runner"]:
        raise SystemExit(
            "catalog performance runner metadata differs from the committed baseline"
        )
    if baseline["rustc_version"] != report["rustc_version"]:
        raise SystemExit(
            "catalog performance rustc version differs from the committed baseline"
        )
    baseline_median = baseline["median_ms"]
    threshold = baseline_median + max(baseline_median * 0.25, 50)
    if report["median_ms"] > threshold:
        raise SystemExit(
            "catalog performance regression: "
            f"median={report['median_ms']}ms threshold={threshold}ms "
            f"baseline={baseline_median}ms"
        )
    print(
        "catalog performance gate ok: "
        f"median={report['median_ms']}ms threshold={threshold}ms "
        f"baseline={baseline_median}ms"
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    record_parser = commands.add_parser("record")
    record_parser.add_argument("--runner-label", required=True)
    record_parser.add_argument("--report", required=True)
    verify_parser = commands.add_parser("verify")
    verify_parser.add_argument("--baseline", required=True)
    verify_parser.add_argument("--report", required=True)
    args = parser.parse_args()
    if args.command == "record":
        record(args)
    else:
        verify(args)


if __name__ == "__main__":
    main()
