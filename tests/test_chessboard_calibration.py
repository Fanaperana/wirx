"""Synthetic algorithm tests only. These do not simulate or certify physical coil coupling."""
import importlib.util
import math
import sys
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location("chessboard_calibration",
    Path(__file__).resolve().parents[1] / "tools" / "chessboard_calibration.py")
cal = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = cal
spec.loader.exec_module(cal)
LABELS = sorted(cal.LABELS)
CENTERS = [104000 * (286000 / 104000) ** (i / 11) for i in range(12)]


def capture(label=None, amplitude=0.07, shift=0.0, gain=1.0, zero=20, session="training", jitter=False):
    frequencies = [f for f in cal.GRID]
    frequencies[0] = 99500
    frequencies[-1] = 301000
    if jitter:
        frequencies = [f * (1 + 0.0003 * math.sin(i)) for i, f in enumerate(frequencies)]
    index = LABELS.index(label) if label else None
    def voltage(f):
        baseline = 0.3 + 0.015 * math.log(f / 100000)
        if index is None:
            return baseline
        # Artificial shape for classification tests; not a physical detector response model.
        center = CENTERS[index] * (1 + shift)
        return baseline + amplitude * math.exp(-0.5 * ((f - center) / (center * 0.012)) ** 2)
    return {"square": "A1", "label": label or "Empty", "session": session,
            "frequency_hz": frequencies, "frequency_error_fraction": 0.001,
            "settled": True, "adc": {"zero_counts": zero, "half_counts": zero + gain * 10000,
                                       "reference_counts": zero + gain * 20000, "reference_volts": 1.25},
            "counts": [zero + gain * voltage(f) / 0.0000625 for f in frequencies]}


def dataset():
    return {"provenance": "synthetic", "empty": {"A1": [capture() for _ in range(3)]},
            "tags": [capture(label, shift=s) for label in LABELS for s in (-0.005, 0.005)]}


class CalibrationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.model = cal.fit(dataset())

    def test_all_labels_survive_amplitude_and_adc_gain_changes(self):
        for label in LABELS:
            for scale in (0.6, 1.0, 1.7):
                s = capture(label, amplitude=0.07 * scale, gain=1.02, zero=-30,
                            session="held-out", jitter=True, shift=0.003)
                result = cal.classify(self.model, s)
                self.assertEqual(result.get("label"), label, result)

    def test_weak_signal_is_not_silently_called_empty_or_a_piece(self):
        result = cal.classify(self.model, capture(LABELS[0], amplitude=0.00001))
        self.assertEqual(result["status"], "NO_DETECTABLE_TAG")
        self.assertNotIn("label", result)

    def test_mixed_response_is_rejected(self):
        s, other, empty = capture(LABELS[3]), capture(LABELS[4]), capture()
        s["counts"] = [a + b - e for a, b, e in zip(s["counts"], other["counts"], empty["counts"])]
        self.assertEqual(cal.classify(self.model, s)["status"], "UNKNOWN")

    def test_invalid_measurements_are_faults(self):
        for change in ("clipped", "reference", "frequency", "range", "settling", "square", "half"):
            s = capture(LABELS[0])
            if change == "clipped": s["counts"][0] = 32767
            if change == "reference": s["adc"]["reference_counts"] = 100
            if change == "frequency": s["frequency_error_fraction"] = 0.01
            if change == "range": s["frequency_hz"][0] = 100001
            if change == "settling": s["settled"] = False
            if change == "square": s["square"] = "H8"
            if change == "half": s["adc"]["half_counts"] += 1000
            self.assertEqual(cal.classify(self.model, s)["status"], "FAULT", change)

    def test_identical_training_classes_cannot_be_approved(self):
        data = dataset()
        for s in data["tags"]:
            s["counts"] = capture(LABELS[0])["counts"]
        with self.assertRaises(cal.InvalidMeasurement):
            cal.fit(data)

    def test_no_training_validation_leakage(self):
        with self.assertRaises(cal.InvalidMeasurement):
            cal.validate(self.model, {"provenance": "synthetic", "tags": [capture(LABELS[0])]})

    def test_simulation_and_partial_coverage_never_claim_hardware_ready(self):
        report = cal.validate(self.model, {"provenance": "synthetic",
            "tags": [capture(l, session="independent") for l in LABELS]})
        self.assertEqual(report["status"], "BENCH_SIGNOFF_REQUIRED")
        self.assertEqual(report["wrong_accepted"], 0)
        self.assertIn("missing square/type/color coverage", report["issues"])

    def test_nominal_component_intervals_are_separate_but_not_a_position_guarantee(self):
        caps = [3.0, 3.6, 4.3, 5.1, 6.2, 7.5, 9.1, 11, 13, 16, 20, 24]
        bands = sorted((1 / (2 * math.pi * math.sqrt(102e-6 * c * 1.01e-9)),
                        1 / (2 * math.pi * math.sqrt(98e-6 * c * 0.99e-9))) for c in caps)
        for low, high in bands:
            self.assertGreater(low, 100000)
            self.assertLess(high, 300000)
        for a, b in zip(bands, bands[1:]):
            self.assertLess(a[1], b[0])


if __name__ == "__main__":
    unittest.main()
