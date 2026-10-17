# Inductive chessboard prototype B / piece tags C

This is an experimental redesign of the supplied ESP32 / CD4046 / coil-matrix concept.
The schematic describes connections; it does not establish successful detection or piece
identification. Test one sensing cell and two tags before manufacturing a 64-square PCB.

## Files

- `chessboard.wx`: complete 64-square controller and sensing electronics.
- `chessboard.esch` / `.elib`: compiled eleceda schematic and symbol library.
- `chessboard-tags.wx`: 12 independent tag templates, one white and one black per type on one sheet; duplicate for a complete set.
- `chessboard-tags.esch` / `.elib`: separate piece-tag schematic and library.
- `chessboard.net` and `chessboard-tags.net`: KiCad PCB netlists for the two assemblies.
- `chessboard-calibration.md`: acquisition, calibration, mechanical envelope and acceptance requirements.
- `../tools/chessboard_calibration.py`: runnable host reference for measured sweep classification.

## Validation of this revision

The board compiles to 14 sheets, 970 components and 607 nets. The 12 tag templates compile to
one sheet, 36 components (including 12 DNP trim capacitors) and 24 nets. Both source files pass wirx electrical checks
without diagnostics. Eleceda loads both compiled outputs without repairs and reconstructs
the exact source connectivity. A separate netlist audit checks all 64 CD4051 channel pins,
ULN output numbering, gate-to-driver connections, IC counts and the dedicated GPIO map.

The board is **not warning-free in eleceda**: there are 66 four-way junction
drafting warnings, primarily in the sensing cells. Those are intentional same-net connections, but two offset T-junctions
would print more clearly. Sensor sheets score 76 in eleceda's automated readability metric;
the remaining board sheets score 97-100, and the tag sheets score 100. No electrical errors
were reported. These results validate the schematic translation, not the sensing physics.
Eight synthetic calibration-tool tests pass; they do not validate real coil performance.

The coils use the inductor schematic symbol and L reference designators. Their assigned
two-pin header footprints provide terminations for external air-core coils; these are not
generated PCB spiral footprints. **100 uH is a provisional tuning target**, to be measured including
the leads. Coil shape, size, winding resistance, Q, mutual coupling and board thickness remain
to be designed and characterized. Do not substitute a shielded power inductor for a coupling coil.

## Corrections from the supplied specification

| Original issue | Prototype A |
| --- | --- |
| ESP32 connected to 5 V in overview | Module supplied only by regulated 3.3 V |
| GPIO32/33 assigned to both outputs and ADC | Dedicated address outputs; GPIO36 is the sole amplitude ADC input |
| Three active-low 74HC138s ambiguously described as 64 row selects | Two active-high 74HC238s decode eight ranks and eight files |
| Active-low select connected directly to AND gates | Active-high row and column selects gate the VCO |
| 5 V HC logic and analog muxes driven by 3.3 V GPIOs | All logic and muxes powered at 3.3 V |
| Incorrect CD4051 and CD4046 pin labels | Custom symbols follow manufacturer pinouts |
| Eight analog channels but insufficient dedicated ADC pins | Eight rank muxes, four dual buffers, one final file mux, one ADC |
| Ambiguous amplifier feedback and 5 V ADC risk | Explicit unity-gain feedback; 3.3 V amplifier supply; per-cell attenuation |
| Eight dual op-amps despite only eight signal paths | Four LMV358 dual packages, all eight channels used |
| PWM on a non-DAC pin without a defined control path | GPIO25 DAC through a 1k / 100nF low-pass to VCO_IN |
| Floating oscillator inhibit and comparator inputs | Defined inhibit, unused comparator inputs grounded, unused outputs NC |
| BAT54S drawn as a single diode | Series pair modeled as pins 1 -> 3 -> 2 for ground restoration and peak detection |
| No explicit DC isolation of driven coil | 100nF AC excitation coupling and 1k source impedance |
| 100-ohm resistor without a power rating | 1210 footprint; select >=0.5 W component, verify thermal performance |
| 1-5 uH / 100-270 pF tags resonate at about 4-16 MHz | Tag revision B uses twelve 100 uH / 3.0-24 nF codes targeting about 103-291 kHz |

## Addressing and signal path

Rank 1..8 corresponds to ROW address 0..7; file A..H corresponds to COL address 0..7.
ROW_A0 and COL_A0 are the least significant address bits.

The row decoder is disabled unless SCAN_EN is high. The first eight AND gates generate
ROW_CLOCK = ROW_SELECT AND VCO_CLOCK. Sixty-four additional AND gates generate
CELL_GATE = ROW_CLOCK AND COL_SELECT. Consequently only the addressed cell is excited.
There are 18 quad AND packages (72 gates), eight ULN2803 arrays and 64 sink channels.
ULN2803 COM is the protection-diode common cathode and connects to 5 V; it is not a VCC pin.

Each selected ULN collector switches its 100-ohm pull-up. The signal is AC-coupled through
100nF and 1k into a parallel external coil / 6.8nF tank. The intended sensor-coil value of
100uH gives an unloaded ideal resonance of about 193kHz. Source impedance, coil resistance,
detector loading, ULN switching behavior and parasitics change the measured response.

The detector AC-couples the tank into a ground-restored diode pair, then filters the positive
envelope with 10k / 10nF. A diode clamps the envelope toward 5 V. A 100k / 47k divider
reduces an approximately 5.3 V envelope to approximately 1.7 V. This keeps the LMV358 input
away from its positive common-mode limit at a 3.3 V supply and leaves ADC headroom. Verify
actual overshoot and ADC attenuation/calibration on hardware; the clamp is not a precision limit.

Eight CD4051s choose the rank within each file. Four LMV358 packages buffer those eight
outputs. A ninth CD4051 chooses the file, followed by 1k / 10nF into ADS1115 AIN0.
GPIO36 (ADC1_CH0) remains connected as a diagnostic backup. ADS1115 AIN1/AIN2/AIN3
provide 1.25V / 0.625V / ground calibration checks from a REF3312 and divider.
GPIO34 measures VCO frequency through the ESP32 pulse counter; it is not an amplitude ADC.

## Firmware pin map

| Function | ESP32 GPIO | Module pad |
| --- | --- | --- |
| ROW_A0 | 32 | 8 |
| ROW_A1 | 33 | 9 |
| ROW_A2 | 14 | 13 |
| COL_A0 | 16 | 27 |
| COL_A1 | 17 | 28 |
| COL_A2 | 18 | 30 |
| SCAN_EN (high enables drive) | 26 | 11 |
| VCO DAC control | 25 / DAC1 | 10 |
| VCO_INH (high disables oscillator) | 27 | 12 |
| MUX_INH (high disconnects all muxes) | 23 | 37 |
| Amplitude ADC | 36 / ADC1_CH0 | 4 |
| VCO frequency counter | 34 | 6 |
| ADS1115 SDA / SCL | 21 / 22 | 33 / 36 |
| UART TX / RX | 1 / 3 | 35 / 34 |
| BOOT | 0 | 25 |
| Reset / enable | EN | 3 |

At reset, external resistors hold SCAN_EN low and both inhibit lines high. Programming
uses a **3.3 V logic UART** with manual BOOT and RESET buttons. The six-pin header's 3.3 V
pin is a reference/output; do not power it simultaneously from a programmer. Flash-interface
pads are left externally unconnected because the module already contains its flash.

Programming header pins 1..6: 3.3 V reference, ESP TX, ESP RX, EN, BOOT_N, GND.

## Piece tags and identification

The passive tags are separate electrical assemblies. The one-page schematic groups a white
and black example for each of the six types; duplicate them to obtain the counts below.
Tag revision B assigns a different capacitor to every type/color pair. Their inductor terminations must connect
to real coupling coils; the capacitor alone does not form a complete tag.

All tag coils have a nominal inductance of 100uH.

| Piece type | Count per color | White capacitor | White ideal resonance | Black capacitor | Black ideal resonance |
| --- | --- | --- | --- | --- | --- |
| Pawn | 8 | 3.0nF | 290.6kHz | 3.6nF | 265.3kHz |
| Knight | 2 | 4.3nF | 242.7kHz | 5.1nF | 222.9kHz |
| Bishop | 2 | 6.2nF | 202.1kHz | 7.5nF | 183.8kHz |
| Rook | 2 | 9.1nF | 166.8kHz | 11nF | 151.7kHz |
| King | 1 | 20nF | 112.5kHz | 24nF | 102.7kHz |
| Queen | 1 | 13nF | 139.6kHz | 16nF | 125.8kHz |

These values follow f = 1 / (2*pi*sqrt(L*C)); they are **not measured detection signatures**.
Coupled coils change the response, and the fixed sensor tank itself has a frequency-dependent
response. Record empty-square and occupied-square sweep curves and test neighboring squares,
off-center pieces, lift height, temperature, component tolerance and multiple pieces.
The twelve nominal frequencies encode type and color, not individual identity: same-color
pawns share a code. Match measured tag coils to 100uH +/-2% and use 1% C0G/NP0 capacitors.
Under these component-only assumptions, the calculated frequency intervals do not overlap and
remain within 100-300kHz. This does not include coupling shifts, coil self-capacitance, resonance
width or detector noise. Those effects may require wider code spacing, a wider sweep or a
different sensing circuit. Confirm separation of all twelve measured signatures before relying
on color recognition. Firmware must classify calibrated sweep curves; reading one amplitude
at a fixed frequency is insufficient. A host reference and its tests are supplied; board acquisition
firmware is not included or flashed. See chessboard-calibration.md for the implementation contract.

## Bring-up sequence

1. Supply regulated 5 V from a current-limited supply. Budget a 2 A supply for Wi-Fi bursts
   and margin, then measure actual demand. Check the 3.3 V regulator's thermal rise and
   output stability; select the AMS1117 vendor and tantalum capacitor ESR together.
2. Keep SCAN_EN low. Verify reset defaults, UART boot and all decoder truth tables.
3. Clear VCO_INH, sweep DAC1 and adjust the 100k trim. Record actual VCO frequency with a
   scope and GPIO34 counter. The 100pF / 10k+trim / 100k timing network is a starting point;
   change timing parts if it cannot cover 100-300kHz at 3.3 V. No frequency guarantee is implied.
4. Fit one measured sensor coil and check switching, pull-up dissipation, tank voltage,
   detector response and the approximately 0-1.8 V measurement range.
5. To change square: disable SCAN_EN, inhibit muxes, update rank/file bits, then enable muxes
   and SCAN_EN. Initially wait at least 1ms after a square/frequency change, then average
   calibrated ADC readings. Determine settling time experimentally before speeding up.
6. Characterize two different tags before all twelve type/color codes and all 64 squares. Piece
   identification is an acceptance test, not a property established by compilation.

## Layout and manufacturing limits

The .esch output is a schematic, not a routed PCB. Coil geometry, antenna keep-out, return
paths, analog isolation, regulator copper area, resistor wattage, capacitor dielectric and
voltage ratings, footprint pad checks, trace dimensions and production DRC remain to be done.
Do not apply the supplied 0.15mm via drill as a default without fabrication capability checks.
Use C0G/NP0 for resonance/timing capacitors. Confirm selected components and footprints in KiCad
before ordering. The retained ESP32-WROOM-32D is NRND; consider a currently supported module
for a future production revision rather than silently changing this prototype's controller.

## Pinout references

- [Espressif ESP32-WROOM-32D](https://documentation.espressif.com/esp32-wroom-32d_esp32-wroom-32u_datasheet_en.html)
- [Nexperia 74HC238](https://assets.nexperia.com/documents/data-sheet/74HC_HCT238.pdf)
- [TI SN74HC08](https://www.ti.com/lit/ds/symlink/sn74hc08.pdf)
- [TI CD4046B](https://www.ti.com/lit/ds/symlink/cd4046b.pdf)
- [TI CD4051B](https://www.ti.com/lit/ds/symlink/cd4051b.pdf)
- [TI LMV358](https://www.ti.com/lit/ds/symlink/lmv358.pdf)
- [ST ULN2803A](https://www.st.com/resource/en/datasheet/uln2803a.pdf)
- [Nexperia BAT54S](https://assets.nexperia.com/documents/data-sheet/BAT54S.pdf)
