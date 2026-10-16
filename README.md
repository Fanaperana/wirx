# wirx

**Schematic-as-code.** In a small language you describe the parts, what connects to what, in what
order, and which parts belong together. The compiler then:

- checks the circuit (names, pin types, power, PCB readiness),
- lays out readable sheets: placement, grouping, wires, labels, power symbols,
- writes them as an [eleceda](../eleceda) schematic (`.esch`), a KiCad netlist or SVG previews.

AI models are good at describing circuits but bad at drawing them. Parts end up far from what they
serve, wires go to the wrong pins, and groups scatter across the page. With wirx the AI (or you)
only states intent. Placement and wiring are deterministic and checked: every output is verified
against the design before it is written.

```wx
module Ldo(vin, vout) {
  u    = AMS1117_3V3
  cin  = C(10uF, 0805)
  cout = C(22uF, 0805)

  vin  -- u.VIN  -- cin.1        // one net; chain order is a placement hint
  vout -- u.VOUT -- cout.1
  GND  -- u.GND, cin.2, cout.2   // fan out with commas
}

sheet "Power" {
  j = USB_C_Power
  j.VBUS -- VBUS
  j.GND, j.SHIELD -- GND
  nc j.CC1, j.CC2
  power_flag VBUS, GND           // power enters the board here

  reg = Ldo(vin: VBUS, vout: +3V3)
}
```

## Usage

```bash
cargo build --release
```

```bash
wirx check design.wx                     # syntax, names, electrical rules
wirx check design.wx --json              # the same, machine-readable (for editors and AI)
wirx build design.wx -o design.esch      # check + lay out + write the eleceda schematic
wirx render design.wx -o out/            # one SVG per sheet, for a quick look
wirx netlist design.wx --format kicad    # KiCad .net for PCB layout (or --format json)
wirx lib list                            # standard parts, rails and symbol shapes
wirx lib show NE555                      # pins of one part
```

`build` refuses to write a file while there are errors (override with `--force`). Every
diagnostic has a stable code, a source location and, where possible, a hint that is a line of
wirx that fixes it:

```text
error[unknown_pin]: part `AMS1117_3V3` (u) has no pin `VI`
  --> design.wx:4:13
  |
4 |   VBUS -- u.VI -- c.1
  |             ^^
  = help: did you mean `u.VIN`? pins: 3 VIN, 2 VOUT, 1 GND
```

## The language

A file holds `part`, `module`, `sheet` and `rail` declarations, `use` imports and title-block
lines. Statements need no separators (`;` is allowed). Comments are `//` and `/* */`.

### Parts

```wx
part ATmega328P {
  prefix "U"                      // reference prefix (U1, U2…)
  value "ATmega328P-AU"
  footprint "Package_QFP:TQFP-32_7x7mm_P0.8mm"
  datasheet "https://…"
  mpn "ATMEGA328P-AU"
  field "LCSC" "C14877"           // any extra field
  symbol box                      // default; or a shape: resistor, capacitor, npn, led, …
  pins {
    29 ~RESET : input             // number name : type
    7  XTAL1  : input
    8  XTAL2  : output @left      // @side overrides automatic placement
    4, 6 VCC  : power_in          // several pins with one name
    3, 5, 21 GND : power_in
  }
}
```

- **Pin types:** `input`, `output`, `bidirectional`, `tri_state`, `passive` (the default),
  `power_in`, `power_out`, `open_collector`, `open_emitter`, `free`, `unspecified`, `no_connect`.
- **`box` symbols** are generated from the pin list. Inputs go on the left, outputs on the right,
  supplies on top and grounds at the bottom. Other pins split between left and right in
  declaration order.
- **Shapes:** `resistor`, `capacitor`, `cap_polarized`, `inductor`, `fuse`, `crystal`, `diode`,
  `led`, `zener`, `schottky`, `npn`/`pnp` (pins B, C, E), `nmos`/`pmos` (G, D, S), `button`,
  `switch`. The part's pins map to the shape's pins in declaration order.

### Instances and connections

```wx
r1 = R(4k7, 0603)                 // value, footprint alias
r2 = R(value: 10k, footprint: "Resistor_SMD:R_0402_1005Metric", ref: "R10", mpn: "…")
reg = Ldo(vin: VBUS, vout: +3V3)  // module instance; ports by name or position

a -- r1.1                         // pin by number…
u.VOUT -- r1.2 -- LED_A           // …or by name; a chain is one net
GND -- u.GND, c1.2, c2.2          // comma lists fan out
reg.vout -- TP1                   // a module instance's port
nc u.PB0, u.PB2                   // intentionally unconnected (or `u.PB0 -- nc`)
power_flag VBUS                   // net driven from off-board (adds a PWR_FLAG)
```

- **Footprint aliases** `0201` to `2512` work for resistors, capacitors, inductors, LEDs, fuses and
  diodes.
- **References** are assigned in declaration order per prefix. `ref:` pins one down.

### Nets and rails

- **Sheet level.** A bare name like `VBUS` or `SDA` is a design-wide net.
- **Inside a module.** Bare names are local to that instance, so two copies don't short. Ports are
  the way in and out.
- **Rails** are drawn as power symbols at every pin, never as long wires. Names starting with `+`
  or `-` (`+3V3`, `-12V`) are rails automatically. The standard library also declares `GND`,
  `GNDA`, `AGND`, `DGND`, `PGND`, `VSS` (ground) and `VCC`, `VDD`, `VBUS`, `VBAT`, `VDDA`, `AVCC`
  (supply).
- **Declaring a rail:** `rail VIO : supply` (kinds: `ground`, `supply`, `negative`).

### Modules, sheets and grouping

```wx
module Name(port1, port2) { … }   // reusable block
sheet "Power" { … }               // a page; statements outside any sheet go to a main sheet
```

- **A module placed on a sheet** is drawn as one framed group, laid out together.
- **A module nested inside a module** is drawn as part of its parent's group.
- **Groups connect across the sheet** by net labels. Groups that share nets are placed next to each
  other, and labels become global when the net spans sheets.

### Layout hints (optional)

```wx
layout {
  c1 near u.VDD                   // keep the decoupling cap at that pin
  flow right
}
```

### Files and the standard library

- **Imports.** `use "parts/sensors.wx"` imports parts and modules from another file. A library
  file is just parts and modules.
- **Standard library.** Always available: `use std::{R, C}` only documents what you use. It holds
  R, C, C_Pol, L, Fuse, Crystal, D, D_Schottky, D_Zener, LED, Q_NPN, Q_PNP, Q_NMOS, Q_PMOS,
  SW_Push, SW_SPST, AMS1117_3V3, NE555 and USB_C_Power.
- **Generated families:** `Conn_01xNN` (single-row header) and `Conn_02xNN` (dual-row, odd/even).

### Title block

```wx
title "My board"
revision "B"
author "…"
company "…"
```

## What the checks cover

| Code | Meaning |
| --- | --- |
| `unknown_*`, `duplicate_*`, `syntax` | Names and structure, with "did you mean" hints |
| `pin_not_connected` | A pin that is neither connected nor marked `nc` |
| `nc_connected` | An `nc` pin that is connected after all |
| `pin_conflict` | KiCad's pin-type matrix: output vs output, power_out vs output, … |
| `power_not_driven` | Power inputs with no regulator output or `power_flag` |
| `input_not_driven` | Inputs with no driver |
| `rails_shorted` | Two rails joined |
| `missing_footprint`, `missing_value` | Not ready for PCB layout |
| `unused_port` | A module port the body never uses |

## How layout works

1. **Groups.** Each sheet-level module instance is a group, and so are the parts placed directly
   on a sheet.
2. **Placement.** In each group the part with the most pins anchors the layout. Parts connected to
   its pins are placed in front of those pins: a series part facing the pin, a pull-down hanging
   below the line. Then their neighbours, and so on.
3. **Rail-only parts.** Decoupling caps settle next to the supply pin they serve. Adjacent pins on
   the same rail share one power symbol.
4. **Routing.** Wires use an orthogonal router on the 50 mil grid. It never lets two nets touch:
   wires cross only by passing straight through, never along or onto another net. Short nets are
   routed first; failures are ripped up and retried. A label is the last resort, and it is
   reported.
5. **Labels.** Nets leaving a group get one label. Groups are placed near the groups they share
   labels with, on the smallest page that fits (A4, then A3, then A2, then more sheets).
6. **Text.** After routing, reference and value text moves to the nearest spot that covers no wire
   or part.
7. **Self-check.** wirx rebuilds connectivity from the drawing, the way eleceda does, and compares
   it with the design. A mismatch stops the build.

## Verifying output with eleceda

`tools/verify-esch.mts` runs eleceda's own loader, validator and readability score on a generated
file. It also compares connectivity with wirx's netlist:

```bash
wirx netlist examples/mcu.wx --format json -o mcu.net.json
cd ../eleceda && pnpm exec tsx ../wirx/tools/verify-esch.mts ../wirx/examples/mcu.esch ../wirx/mcu.net.json
```

The examples (`examples/ldo.wx`, `blinker.wx`, `mcu.wx`) meet the bar eleceda sets for its own
reference designs: they load without repairs, have no errors or warnings, and score 95+ for
readability.

## Not yet

- Hierarchical sheet symbols
- Buses
- `.esch` → `.wx` (two-way editing)
- LSP and formatter
- Footprint pad checks
- KiCad `.kicad_sch` output
