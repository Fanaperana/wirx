# wirx

**Schematic-as-code.** In a small [Rhai](https://rhai.rs) script you describe the parts, what
connects to what, in what order, and which parts belong together. The compiler then:

- checks the circuit (names, pin types, power, PCB readiness),
- lays out readable sheets: placement, grouping, wires, labels, power symbols,
- writes them as an [eleceda](../eleceda) schematic (`.esch`), a KiCad netlist or SVG previews.

AI models are good at describing circuits but bad at drawing them. Parts end up far from what they
serve, wires go to the wrong pins, and groups scatter across the page. With wirx the AI (or you)
only states intent. Placement and wiring are deterministic and checked: every output is verified
against the design before it is written.

```rhai
fn ldo(vin, vout) {                          // a function call = one framed group
    let u = AMS1117_3V3();
    vin  -> u.VIN  -> C("10uF", "0805") -> GND;   // two-pin parts sit inline
    vout -> u.VOUT -> C("22uF", "0805") -> GND;
    u.GND -> GND;
}

sheet("Power", || {
    let j = USB_C_Power();
    j.VBUS -> VBUS.driven();                 // power enters the board here (PWR_FLAG)
    [j.GND, j.SHIELD] -> GND.driven();
    j.CC1 -> R("5k1") -> GND;
    j.CC2 -> R("5k1") -> GND;

    ldo(VBUS, P3V3);
    P3V3 -> R("1k") -> LED("green") -> GND;
});
```

## Usage

```bash
cargo build --release
```

```bash
wirx check design.wx                     # run the script, check names and electrical rules
wirx check design.wx --json              # the same, machine-readable (for editors and AI)
wirx build design.wx -o design.esch      # check + lay out + write the eleceda schematic
wirx render design.wx -o out/            # one SVG per sheet, for a quick look
wirx netlist design.wx --format kicad    # KiCad .net for PCB layout (or --format json)
wirx lib list                            # standard parts, rails and symbol shapes
wirx lib show NE555                      # pins of one part
wirx lib export design.wx -o d.kicad_sym # the design's symbols for KiCad (or -o d.elib)
```

`build` refuses to write a file while there are errors (override with `--force`). Every
diagnostic has a stable code, a source location (also for errors inside functions and loops) and,
where possible, a hint that fixes it:

```text
error[unknown_pin]: part `AMS1117_3V3` has no pin `VI`
  --> design.wx:3:14
  |
3 |     vin -> u.VI;
  |              ^^
  = help: did you mean `.VIN`? pins: 3 VIN, 2 VOUT, 1 GND
```

## The language

A `.wx` file is a Rhai script; the full reference is [docs/rhai-syntax.md](docs/rhai-syntax.md).
The essentials:

| What | How |
| --- | --- |
| Standard part | `R("10k")`, `C("100nF", "0402")`, `R("0", "0603", #{ ref: "R10", mpn: "…", dnp: true })` |
| Computed value | `R(e24((5.0 - 2.0) / 0.010))` (numbers are base units) |
| Custom part | `fn ATmega328P() { part(#{ name: "ATmega328P", footprint: "…", pins: [[29, "~RESET", input], [[4, 6], "VCC", power_in]] }) }` |
| Pins | `u.VIN`, `c[1]`, `j["A9"]`, `u.pin("~RESET")`, `u.pins(power_in)`, `u.pins("PB")` |
| Connect | `a -> b` joins and continues from `b`; `[a, b] -> GND` fans in or out |
| Inline part | `pb1 -> R("330") -> LED("red") -> GND` (in → out: 1 → 2, A → K, + → −) |
| Unused pin | `u.PB0 -> NC;` or `nc([u.PB2, u.PC3]);` |
| Nets | `net("SDA")` (same name, same net everywhere), `net()` (anonymous) |
| Rails | `GND`, `P3V3`, `P5V`, `VBUS`, … built in; `supply("VIO")`, `ground("…")`, `negative("…")` |
| Off-board power | `VBUS.driven()` (adds a PWR_FLAG) |
| Groups | every function call that places parts is a framed group; `group("Name", \|\| { … })` |
| Sheets | `sheet("Power", \|\| { … })`; statements outside sheets go to a main sheet |
| Hints | `near(c1, u.VDD)`, `flow(down)` |
| Imports | `import "parts/mcu" as mcu;` then `mcu::ATmega328P()` |
| Title block | `title("…"); revision("B"); author("…"); company("…");` |

- **Logic.** Loops, conditions and arithmetic are plain Rhai:

  ```rhai
  for i in 0..8 { u.pin(`PB${i}`) -> R("330") -> LED("red") -> GND; }
  ```

- **Scope.** Functions can't see `let` variables from outside; pass nets in as parameters.
  Built-in rails, `net("…")` and `supply("…")` work everywhere.
- **Standard parts:** R, C, C_Pol, L, Fuse, Crystal, D, D_Schottky, D_Zener, LED, Q_NPN, Q_PNP,
  Q_NMOS, Q_PMOS, SW_Push, SW_SPST, AMS1117_3V3, NE555, USB_C_Power, plus `Conn(n)` (single-row
  header) and `Conn2(rows)` (dual-row, odd/even).
- **Footprint aliases** `0201` to `2512` work for resistors, capacitors, inductors, LEDs, fuses and
  diodes.

## Symbols that travel

- **Lay out a symbol in wirx.** `symbol: #{ left: […], right: […], top: […], bottom: […] }` puts
  pins where you want them, with `gap`s between groups and stacked pins drawn once.
- **Reuse a symbol** from an eleceda `.elib`, an `.esch` or a KiCad `.kicad_sym` file with
  `library: "…", symbol: "Name"`.
- **Automatic export.** `wirx build` writes the design's symbols to a `.elib` beside the `.esch`
  and registers it, so eleceda lists them in its Library panel when it opens the design.
- **KiCad export.** `wirx lib export design.wx -o design.kicad_sym` writes them for KiCad.

Details and examples: [docs/rhai-syntax.md](docs/rhai-syntax.md#symbols).
`examples/rp2350b.wx` draws the RP2350B this way.

## What the checks cover

| Code | Meaning |
| --- | --- |
| `syntax`, `unknown_*`, `duplicate_*` | Script and name errors, with "did you mean" hints |
| `string_as_net`, `not_inline`, `bad_*` | Connections that make no sense (a string as a net, an 8-pin part inline) |
| `pin_not_connected` | A pin that is neither connected nor marked `NC` |
| `nc_connected` | An `NC` pin that is connected after all |
| `pin_conflict` | KiCad's pin-type matrix: output vs output, power_out vs output, … |
| `power_not_driven` | Power inputs with no regulator output or `.driven()` |
| `input_not_driven` | Inputs with no driver |
| `rails_shorted` | Two rails joined |
| `missing_footprint`, `missing_value` | Not ready for PCB layout |

## How layout works

1. **Groups.** Each function call site that places parts is a group (nested calls stay in the
   outer group), and so are the parts placed directly on a sheet.
2. **Placement.** In each group the part with the most pins anchors the layout. Parts connected to
   its pins are placed in front of those pins: a series part facing the pin, a pull-down hanging
   below the line. Then their neighbours, and so on.
3. **Rail-only parts.** Decoupling caps settle next to the supply pin they serve. Adjacent pins on
   the same rail share one power symbol.
4. **Routing.** Wires use an orthogonal router on the 50 mil grid. It never lets two nets touch:
   wires cross only by passing straight through, never along or onto another net. Short nets are
   routed first; failures are ripped up and retried. A label is the last resort, and it is
   reported.
5. **Labels and sheets.** Nets leaving a group get one label. A sheet is drawn around its biggest
   group: the groups it shares labels with stand in a column beside the side of it those labels
   leave from (the crystal left of the chip when XIN is on its left), level with their labels, and
   every frame fits its circuit. Sheets without such partners are a grid of blocks. The smallest
   page that fits is used (A4, then A3, then A2, then more sheets).
6. **Wires between blocks.** A small part (a resistor, a button) that reaches a neighbouring block
   only through a label gets a wire to it instead, where a short one fits without crossing or
   touching anything.
7. **Text.** After routing, reference and value text moves to the nearest spot that covers no wire
   or part.
8. **Self-check.** wirx rebuilds connectivity from the drawing, the way eleceda does, and compares
   it with the design. A mismatch stops the build.

## Editing in eleceda: `wirx import`

`wirx import` turns an `.esch` file back into wirx source, so changes made in eleceda (by hand, or
by an AI through eleceda's MCP server) flow back into the design:

```bash
wirx build design.wx -o design.esch      # draw it
# … edit design.esch in eleceda …
wirx import design.esch -o design.wx     # take the edits back (without -o it prints the source)
```

- **Connectivity** is rebuilt the way eleceda does it: wires join at their ends and at any point on
  them, labels join by name (local labels per sheet, global labels everywhere), power symbols by
  value. Hierarchical labels join their sheet pins.
- **Parts.** A wirx symbol (`wirx:R`) whose pins still match the standard part becomes that part
  again (`R(…)`, `Conn(6)`). Other symbols become custom parts, `fn Name() { part(#{ … }) }`, with
  the pins from the file (number, name, type, side) and the shape they match, else `"box"`.
- **Instances** keep their value, footprint (as an alias such as `"0603"` where one fits), extra
  fields and `dnp` in the options map. References are kept: `ref:` is written only where automatic
  numbering would differ.
- **Nets** become `->` chains, named after their labels (`net("SDA")`). Power-symbol nets become
  rails: built-ins like `GND` or `P3V3`, else `supply("…")` / `ground("…")`. No-connect flags
  become `nc([…])` and PWR_FLAGs `.driven()`.
- **Groups.** Each sheet becomes a `sheet("…", || …)`. A block frame titled like a function
  (`ldo`, `ldo 2`) becomes a function called once per block; blocks that hold the same parts wired
  the same way share one function, with the nets that differ as parameters. Other titles become
  `group("…", || …)`.

Before writing, `import` compiles its own output and compares the netlist with the schematic's; it
refuses to write on a mismatch (override with `--force`). What wirx cannot express yet is reported
as a warning: buses, text notes, layout hints, and per-part value, footprint, fields or references
of custom parts (their constructor takes no options yet). Re-importing a file that wirx built gives
the same netlist, and importing again after a rebuild gives the same source (`tests/import.rs`
checks this for the examples).

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
- LSP and formatter
- Footprint pad checks
- KiCad `.kicad_sch` output
