# Calibration and physical acceptance — board B / tags C

This design now includes a precision amplitude ADC and electrical self-check points.
It does **not** have measured proof of twelve-class recognition across position and temperature.
The software below is a runnable host reference, not ESP32 firmware or a hardware simulator.
Do not equate its synthetic unit-test results with a working chessboard.

## Mechanical design envelope

Use 55mm squares and a 440 x 440mm playing area, excluding the frame. FIDE specifies
50-60mm squares; 55mm is our choice within that range, not a single mandated dimension.
[FIDE board specification, section 3.3](https://handbook.fide.com/chapter/ChessEquipmentWithoutElectronicComponenets032026)

Sensor-to-tag distance includes the playing surface, adhesive, coil burial and the piece's
base/felt. No distance or placement guarantee can be assigned until those are specified.
The original 30mm sensor coil is not automatically adequate for a 55mm square. Test larger
air-core/PCB coils and tag orientation on a single-square fixture before selecting geometry.
Do not add a ground plane, metal weights or mounting hardware beneath coils without repeating
the coupling tests. No verified PCB coil layout is included yet.

Record the following requirements before mechanical release:

| Parameter | Current status |
| --- | --- |
| Square pitch | 55mm design target |
| Playing area | 440 x 440mm |
| Surface/adhesive thickness | Not specified; measure |
| Tag burial/felt thickness | Not specified; measure |
| Maximum sensor-to-tag gap | Sum of the above; not established |
| Allowed center offset and tilt | Not specified; characterize the usable square area |
| Temperature range | Not specified; qualify intended indoor range |
| Maximum move-report latency | Not specified; measure with the real scan implementation |

## Hardware provisions

- ADS1115 at I2C address 0x48, SDA=GPIO21, SCL=GPIO22, 3.3V supply. Use the +/-2.048V
  PGA range and single-shot conversions at 860SPS. Its internal reference is used for conversion.
- AIN0 measures ADC_SENSE after the existing multiplexer and 1k/10nF filter.
  GPIO36 remains a diagnostic backup, not the preferred precision path.
- AIN1 measures the external REF3312 nominal 1.25V reference, AIN2 its nominal 0.625V
  divider, and AIN3 ground. These are independent check inputs, not an external ADC VREF pin.
- Measure the actual reference and divider voltages with a trusted meter during bring-up;
  record them. The host reference assumes the divider is half the measured reference, with a
  5mV check allowance. This allowance is a provisional engineering policy, not a vendor guarantee.
- A four-pin calibration header exposes SENSE / 1.25V / 0.625V / GND. To inject a reference
  into AIN0, first set SCAN_EN=0 and MUX_INH=1. Remove the bench jumper before scanning.
  This checks the final filter and ADC input; it does not calibrate all 64 detectors.
- GPIO34 counts actual VCO edges using the ESP32 hardware pulse counter. Do not use an ISR
  on every edge at 300kHz. Firmware must associate every amplitude sample with measured frequency.
- Twelve optional 100pF parallel tag capacitor footprints are DNP by default. Fit selected
  C0G parts only after measuring a particular tag. The nominal frequency table assumes DNP.
  Recalibrate after fitting a trim capacitor; adding capacitance lowers frequency.

Pinouts: [ADS1115](https://www.ti.com/lit/ds/symlink/ads1115.pdf),
[REF3312](https://www.ti.com/lit/gpn/REF3312).

## Acquisition requirements for the ESP32 implementation

1. Check all ADC reference inputs at boot and periodically. Reject clipped, drifting or
   inconsistent readings; do not try to classify through an ADC fault.
2. Establish the VCO's measured DAC-code/frequency map over the full 100-300kHz range.
   Retune timing parts if coverage is missing. Frequency error budget for the reference
   classifier is 0.25%; use an adequate count interval and measure timebase error.
3. Disable excitation and inhibit muxes before changing rank/file address. Update all six
   address bits, enable the muxes, then enable excitation. Never switch directly between
   active squares with intermediate addresses being driven.
4. After changing frequency or square, wait for the analog path and ADS1115 conversion to
   settle. Start with 1ms analog delay plus one complete conversion; discard the first reading
   after channel/PGA changes. Measure step response and increase delay until repeatable.
5. Collect dense sweeps with measured frequency spacing <=1.5%, average/median repeated
   readings, and record settling/frequency-error metadata. An eight-bit DAC can have repeated
   or skipped frequency steps: deduplicate real frequency measurements and refuse sparse gaps.
6. Capture three or more independent empty-board sweeps on **each** square. Repeat only when
   the board is known empty; never learn a baseline from an occupied or uncertain square.
7. Collect each of the 32 actual tags at center, edges and corners of the allowed placement
   area, each tag orientation, minimum/maximum gap, and intended temperature extremes.
   Use multiple sweeps and independent sessions. Repeat for all 64 squares or demonstrate
   a separately validated equivalence procedure; a single-square result is not board coverage.
8. Repeat held-out tests with occupied neighbors and a fully populated board. Passive neighboring
   tags can couple even though only one reader cell is excited. Firmware cannot correct an
   unmeasurably weak signal or two physically indistinguishable signatures.
9. Track temporal stability and request a rescan when a piece is lifted or straddles squares.
   A decision requires consistent accepted results on at least three independent sweeps.
   Do not let chess legality override a contradictory electrical measurement.

ADS1115 is at most 860SPS: a 201-point full sweep of 64 squares already requires approximately
15 seconds of conversion time before switching, averaging and settling overhead. This is a
diagnostic/calibration mode, **not a claim of tournament-speed live scanning**. Live firmware
needs a measured fast occupancy scan followed by selective sweeps of changed/uncertain squares,
or a faster acquisition architecture if the required latency cannot be met.

## Runnable calibration and recognition reference

`tools/chessboard_calibration.py` uses measured frequency grids, per-square empty baselines,
noise-whitened differences, amplitude-normalized shape matching and a class-separation margin.
Multiple position/tolerance profiles can be stored for each type/color pair on each square.
This compensates amplitude changes only when the remaining shape is still measurable and
distinct; it does not assume that position changes amplitude alone.

Its outputs are:

- `IDENTIFIED`: within the learned radius and separated from the runner-up.
- `UNKNOWN`: strong but ambiguous/untrained response; rescan or report uncertainty.
- `NO_DETECTABLE_TAG`: insufficient evidence. This deliberately does not assert EMPTY.
- `FAULT`: calibration, clipping, frequency coverage, settling or uncalibrated-square failure.

Training refuses overlapping classes. The acceptance radius is one-quarter of the minimum
between-class cosine distance. The minimum separation of 0.02, signal SNR of 8 and 0.2mV
noise floor are **provisional policies** to be qualified against hardware, not proof of accuracy.

Input JSON for fitting has `provenance: "measured"`, `empty: {"A1": [sweep, sweep, sweep], ...}`
and `tags: [sweep, ...]`. Synthetic datasets must say `provenance: "synthetic"`.
Every calibrated square needs all twelve type/color labels. Each sweep has this structure:

```json
{
  "square": "A1",
  "label": "White Pawn",
  "session": "fixture-session-01",
  "frequency_hz": [99000, 99500, 100000],
  "counts": [5000, 5001, 5002],
  "frequency_error_fraction": 0.001,
  "settled": true,
  "adc": {
    "zero_counts": 20,
    "half_counts": 10020,
    "reference_counts": 20020,
    "reference_volts": 1.25
  }
}
```

The three-point array above illustrates the schema only: a real capture must cover
100-300kHz densely with at least 40 samples. Record position, gap, tag serial, temperature,
neighbor occupancy and timing alongside the sweeps; this reference does not automate those
physical coverage checks. ADC counts are signed conversion results, averaged if needed,
with every reference reading taken at the same PGA setting as the amplitude reading.

```text
python tools/chessboard_calibration.py fit training.json model.json
python tools/chessboard_calibration.py classify model.json sample.json
python tools/chessboard_calibration.py validate model.json held-out.json
python -m unittest discover -s tests -p test_chessboard_calibration.py -v
```

Validation refuses session reuse, counts accepted errors and rejected known pieces, reports
missing square/class coverage and never certifies hardware from synthetic data. Even complete
measured square/class coverage returns `BENCH_SIGNOFF_REQUIRED`: position, gap, temperature,
neighbors and latency must independently meet the declared acceptance envelope.

## Release gates

Before claiming reliable detection: define the mechanical envelope; obtain the coil geometries
and Q measurements; measure frequency coverage; prove ADC stability and detector sensitivity;
collect independent 32-tag/64-square position and environment data; verify no wrong accepted
labels and acceptable rejection/latency rates. If classes overlap, change the RF geometry,
code spacing, frequency range or measurement method rather than merely loosening thresholds.
The current files are a calibration-capable prototype for those tests, not a proven production design.
