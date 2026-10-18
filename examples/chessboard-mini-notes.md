# Mini chessboard prototype (revision A)

A two-square bench prototype of the inductive chessboard. Its job is to prove, before building a
64-square board, that one coil per square can tell four different pieces apart.

| File | What it is |
| --- | --- |
| `chessboard-mini.wx` | The board: RP2350A, LDC1614, two coil connectors (two more spare), USB-C |
| `chessboard-mini-tags.wx` | Four passive piece tags (Pawn, Knight, Rook, Queen), each its own small circuit |
| `.esch` / `.elib` / `.net` | Compiled schematic for eleceda, its symbols, and KiCad netlists for PCB layout |

This is a schematic for a prototype. The coil values are placeholders until measured, and the
piece identification has to be shown on the bench. Nothing here has been built or tested.

## How it works

- **The squares.** Each square is a flat coil in parallel with a capacitor (an LC tank) across
  one LDC1614 channel (`INxA` / `INxB`). The LDC1614 keeps the tank oscillating and measures its
  frequency to 28 bits against a 40 MHz reference oscillator. With a 10 µH coil and 680 pF, a
  square runs at about **1.93 MHz**.
- **The pieces.** Each piece hides a closed loop of its own coil and a capacitor, tuned to a
  different frequency. Placed over a square it couples into the sensor coil and pulls the
  square's frequency. Tags tuned **below** the square's frequency pull it one way, tags tuned
  **above** pull it the other way, and the closer the tag's tuning, the larger the pull.
- **The firmware.** The RP2350A reads both channels over I2C, subtracts each square's empty
  reading, and matches the shift against four calibrated signatures.

| Piece | Tag capacitor | Tag resonance (10 µH) | Relative to the square (1.93 MHz) |
| --- | --- | --- | --- |
| Pawn | 2.4 nF | 1.03 MHz | 47 % below |
| Knight | 1.3 nF | 1.40 MHz | 27 % below |
| Rook | 330 pF | 2.77 MHz | 43 % above |
| Queen | 180 pF | 3.75 MHz | 94 % above |

No tag is tuned within ±25 % of the square. A tag tuned close to the sensor frequency soaks up
the tank's energy, and the LDC1614 then flags a low-amplitude error instead of giving a reading.

## Choosing the coils (do this first)

The design uses ready-made coils, not coils etched on the board. Each square's coil plugs into a
2-pin JST-PH connector (`J`), and each tag's coil is soldered to its tag's two pads.

- **Square coils:** flat spiral coils, about 20–30 mm across and 8–20 µH. Flat
  wireless-charging receiver coils (for example from Würth's WE-WPCC range) and the coils on TI's
  LDC coil evaluation boards fit. Confirm the exact part numbers and inductance from the
  datasheet before ordering; none are fixed in this design.
- **Tag coils:** smaller flat coils, 15–25 mm, so they fit a piece base. They can be the same
  family as the square coils.
- **Measure** each coil's inductance near 2 MHz (an LCR meter, or the LDC1614 itself with a
  known capacitor), then compute every capacitor:

  C = 1 / ((2π f)² L)

  For a square, aim f at about 2 MHz. Each tag keeps its spacing from the table above. Each
  tank and each tag has an empty 100 pF trim footprint for fine adjustment.
- **Capacitors:** use C0G/NP0, 1 %, everywhere in the tanks and tags. Other dielectrics drift
  with temperature and smear the signatures.

## Board summary

| Block | Parts | Notes |
| --- | --- | --- |
| USB-C power and data | J1 TYPE-C-31-M-12, USBLC6-2SC6 ESD, 5k1 CC pull-downs, 27 Ω series | Same as the RP2350B example |
| 3.3 V supply | SS34 Schottky, AP2112K-3.3, 10 µF in and out, power LED | 600 mA is plenty |
| MCU | RP2350A (QFN-60), 12 MHz crystal, W25Q128JV flash, BOOTSEL / RESET, SWD header, status LED on GPIO25 | Core supply from the chip's own regulator through a 3.3 µH inductor; every supply pin decoupled |
| Sensor | LDC1614 (WQFN-16), 1 µF + 100 nF on VDD, 40 MHz oscillator on CLKIN | I2C address 0x2A (ADDR low) |
| Squares A, B | coil connector, 680 pF C0G tank, empty trim | Populate |
| Squares C, D | the same footprints | Leave empty (spare channels for a 2 × 2 patch) |

The RP2350A pin numbers were checked against two independent sources. The LDC1614 pins come from
KiCad's `Sensor_Proximity` library.

| Signal | RP2350A | LDC1614 |
| --- | --- | --- |
| I2C SDA | GPIO4 (pin 7) | SDA (2), 4.7 kΩ pull-up |
| I2C SCL | GPIO5 (pin 8) | SCL (1), 4.7 kΩ pull-up |
| Interrupt | GPIO6 (pin 9) | INTB (5), open drain, 10 kΩ pull-up |
| Shutdown | GPIO7 (pin 10) | SD (6), 10 kΩ pull-down (runs by default) |

## Firmware starting point

Starting values only. Confirm every register against the LDC1614 datasheet, or TI's LDC
calculator, for the coils you actually fit.

1. Release SD (drive GPIO7 low), then wait about 1 ms.
2. For channels 0 and 1, set the clock dividers so the reference is the full 40 MHz. Use the
   input divider that suits a 1–9 MHz differential sensor.
3. Set RCOUNT for the conversion time you want: longer conversions give finer resolution. Set
   SETTLECOUNT from the tank's Q, as the datasheet describes.
4. Set the drive current so the tank amplitude sits within the datasheet's range with no piece
   present. Check that it stays in range with each piece on top.
5. In MUX_CONFIG, enable autoscan of channels 0 and 1, with the deglitch filter just above the
   sensor frequency (3.3 MHz).
6. Read DATA_CH0 and DATA_CH1 when INTB signals a conversion. Treat error flags (amplitude
   high or low, watchdog) as readings in their own right: a nearly matched tag shows up as one.

**Calibration:** record each empty square, then each piece on each square (and at the heights
the real pieces will sit). Classify a reading by the nearest of the five signatures (empty plus
four pieces), and log any reading that lands between them.

## Bench checks before scaling up

- **Empty squares:** their frequency is stable to well under the smallest piece shift over
  several minutes, and as the board warms up.
- **Separation:** the four pieces stay separable when moved off-centre and lifted by the base
  thickness.
- **Neighbours:** a piece on square A does not register on square B (try 25, 30 and 40 mm
  spacing).
- **Ruling out the wrong coils:** shielded power inductors will not couple. The coils must be
  open (unshielded) flat spirals.
