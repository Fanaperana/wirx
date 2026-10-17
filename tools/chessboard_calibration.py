"""Host-side reference for measured chessboard sweeps, not flashed ESP32 firmware.

Capture format and acceptance procedure: examples/chessboard-calibration.md.
Only measured frequency coordinates are used. No extrapolation or nominal-frequency lookup.
"""
import argparse
import bisect
import json
import math
import statistics
from pathlib import Path

LABELS = {f"{color} {piece}" for color in ("White", "Black")
          for piece in ("Pawn", "Knight", "Bishop", "Rook", "King", "Queen")}
SQUARES = {f"{file}{rank}" for file in "ABCDEFGH" for rank in range(1, 9)}
GRID = [100000.0 * 3.0 ** (i / 200) for i in range(201)]


class InvalidMeasurement(ValueError):
    pass


def require(condition, message):
    if not condition:
        raise InvalidMeasurement(message)


def corrected_volts(adc, counts):
    """Use measured ADS1115 ground/reference plus an independent half-scale check."""
    z, h, r, v = (float(adc[k]) for k in
                  ("zero_counts", "half_counts", "reference_counts", "reference_volts"))
    require(all(math.isfinite(x) for x in (z, h, r, v)), "invalid ADC reference")
    require(1.24 <= v <= 1.26 and abs(z) <= 128 and r - z > 10000,
            "ADC reference or zero out of range; verify PGA +/-2.048 V")
    gain = v / (r - z)
    require(abs((h - z) * gain - v / 2) <= 0.005,
            "ADC half-reference check failed")
    values = [float(c) for c in counts]
    require(all(math.isfinite(c) and -128 <= c < 32000 for c in values),
            "ADC invalid or clipped")
    volts = [(c - z) * gain for c in values]
    require(all(-0.01 <= x <= 1.8 for x in volts), "signal outside calibrated range")
    return volts


def interpolate(frequencies, values, grid=GRID):
    require(len(frequencies) == len(values) and len(values) >= 40, "insufficient sweep")
    require(all(math.isfinite(f) and f > 0 for f in frequencies), "invalid frequency")
    require(all(a < b for a, b in zip(frequencies, frequencies[1:])),
            "frequencies must be measured, unique and increasing")
    require(frequencies[0] <= grid[0] and frequencies[-1] >= grid[-1],
            "VCO does not cover 100-300kHz; extrapolation refused")
    require(max(b / a for a, b in zip(frequencies, frequencies[1:])) <= 1.015,
            "frequency sampling too sparse (maximum step 1.5%)")
    out = []
    for f in grid:
        i = max(0, min(len(values) - 2, bisect.bisect_right(frequencies, f) - 1))
        w = (f - frequencies[i]) / (frequencies[i + 1] - frequencies[i])
        out.append(values[i] * (1 - w) + values[i + 1] * w)
    return out


def sweep_values(sweep):
    require(sweep["square"] in SQUARES, "unknown square")
    require(0 <= sweep.get("frequency_error_fraction", 1) <= 0.0025,
            "frequency counter error exceeds 0.25%")
    require(sweep.get("settled") is True, "unsettled measurement")
    return interpolate(sweep["frequency_hz"], corrected_volts(sweep["adc"], sweep["counts"]))


def feature(values, baseline, noise):
    diff = [(v - b) / n for v, b, n in zip(values, baseline, noise)]
    snr = math.sqrt(sum(x * x for x in diff) / len(diff))
    length = math.sqrt(sum(x * x for x in diff))
    return ([x / length for x in diff] if length else [0.0] * len(diff)), snr


def distance(a, b):
    return max(0.0, 1.0 - sum(x * y for x, y in zip(a, b)))


def fit(captures):
    require(captures.get("provenance") in ("measured", "synthetic"), "declare data provenance")
    model = {"schema": 1, "provenance": captures["provenance"], "grid_hz": GRID,
             "squares": {}, "training_sessions": [], "status": "UNVALIDATED"}
    empties = captures["empty"]
    tags = captures["tags"]
    require(bool(empties) and bool(tags), "empty calibration dataset")
    sessions = set()
    for square, empty in empties.items():
        require(square in SQUARES and len(empty) >= 3, "need three empty sweeps per square")
        require(all(s["square"] == square for s in empty), "baseline square mismatch")
        arrays = [sweep_values(s) for s in empty]
        baseline = [statistics.median(v) for v in zip(*arrays)]
        # 0.2mV is an explicit provisional noise floor, not an ADC accuracy claim.
        noise = [max(0.0002, statistics.stdev(v)) for v in zip(*arrays)]
        profiles = []
        for s in [s for s in tags if s["square"] == square]:
            require(s["label"] in LABELS and bool(s.get("session")), "invalid training label/session")
            vec, snr = feature(sweep_values(s), baseline, noise)
            require(snr >= 8, f"weak training signature: {square} {s['label']}")
            profiles.append({"label": s["label"], "vector": vec})
            sessions.add(s["session"])
        require({p["label"] for p in profiles} == LABELS,
                f"all twelve labels required on calibrated square {square}")
        separation = min(distance(a["vector"], b["vector"])
                         for a in profiles for b in profiles if a["label"] != b["label"])
        require(separation >= 0.02, f"overlapping training classes on {square}")
        model["squares"][square] = {"baseline": baseline, "noise": noise,
                                    "profiles": profiles, "radius": separation / 4}
    require(all(s["square"] in empties for s in tags), "tag has no empty calibration")
    model["training_sessions"] = sorted(sessions)
    return model


def classify(model, sweep):
    try:
        square = sweep["square"]
        require(square in model["squares"], "square has not been calibrated")
        entry = model["squares"][square]
        vec, snr = feature(sweep_values(sweep), entry["baseline"], entry["noise"])
        if snr < 8:
            # Deliberately not EMPTY: a weak/off-center/broken tag could produce the same result.
            return {"status": "NO_DETECTABLE_TAG", "snr": snr}
        best = {}
        for p in entry["profiles"]:
            best[p["label"]] = min(best.get(p["label"], 2), distance(vec, p["vector"]))
        ordered = sorted(best.items(), key=lambda p: p[1])
        label, score = ordered[0]
        margin = ordered[1][1] - score
        if score > entry["radius"] or margin < entry["radius"]:
            return {"status": "UNKNOWN", "snr": snr, "distance": score, "margin": margin}
        return {"status": "IDENTIFIED", "label": label, "snr": snr,
                "distance": score, "margin": margin}
    except (InvalidMeasurement, KeyError, TypeError, ValueError) as e:
        return {"status": "FAULT", "reason": str(e)}


def validate(model, captures):
    """Independent held-out records; coverage is a gate, never inferred from a simulation."""
    require(captures.get("provenance") == model["provenance"], "provenance mismatch")
    records = captures["tags"]
    require(bool(records), "empty validation set")
    seen, issues = set(), []
    correct = rejected = wrong = 0
    # Detailed mechanical coverage remains an engineering acceptance requirement in the guide.
    for record in records:
        require(record.get("session") and record["session"] not in model["training_sessions"],
                "validation session overlaps training")
        result = classify(model, record)
        if record["label"] in LABELS:
            seen.add((record["square"], record["label"]))
            if result.get("label") == record["label"]:
                correct += 1
            elif result["status"] == "IDENTIFIED":
                wrong += 1
            else:
                rejected += 1
        else:
            if result["status"] == "IDENTIFIED":
                wrong += 1
            else:
                correct += 1
    if seen != {(s, l) for s in SQUARES for l in LABELS}:
        issues.append("missing square/type/color coverage")
    if wrong:
        issues.append("incorrect accepted classifications")
    if rejected:
        issues.append("known pieces rejected in declared conditions")
    if model["provenance"] != "measured":
        issues.append("synthetic data cannot establish hardware performance")
    issues.append("mechanical, temperature, adjacent-piece and latency acceptance require bench sign-off")
    return {"status": "BENCH_SIGNOFF_REQUIRED", "correct": correct, "rejected": rejected,
            "wrong_accepted": wrong, "covered_square_labels": len(seen), "issues": issues}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("fit", "classify", "validate"))
    parser.add_argument("input", type=Path)
    parser.add_argument("output_or_capture", type=Path)
    args = parser.parse_args()
    data = json.loads(args.input.read_text())
    if args.command == "fit":
        result = fit(data)
        args.output_or_capture.write_text(json.dumps(result, indent=2) + "\n")
        print("Wrote UNVALIDATED model; use independent measurements and bench sign-off.")
    else:
        capture = json.loads(args.output_or_capture.read_text())
        result = classify(data, capture) if args.command == "classify" else validate(data, capture)
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
