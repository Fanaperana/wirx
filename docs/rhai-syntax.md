# wirx language reference

A `.wx` file is a Rhai script. This page covers everything a design uses: values, parts and
symbols, pins, connections, nets and rails, groups and sheets, loops, layout hints, imports and
errors. For the commands and the overall workflow, see the [README](../README.md). For complete
designs, see [examples/](../examples).

Running a [Rhai](https://rhai.rs) script builds the design: parts, nets, groups and sheets. wirx
then checks it, lays it out and writes the `.esch`, netlist or SVG. The design decisions behind the
syntax: `->` connection chains; pins as `u.VIN`, `u[1]` or `u.pin(…)`; built-in rails plus
constructors; values as strings (numbers also accepted); parts defined with map literals; files
keep the `.wx` extension.

## At a glance

```rhai
title("USB-C to 3.3V");
revision("A");

let vbus = VBUS.driven();                    // power enters the board here (PWR_FLAG)

fn ldo(vin, vout) {                          // a function = a framed group named "ldo"
    let u = AMS1117_3V3();
    vin  -> u.VIN  -> C("10uF", "0805") -> GND;
    vout -> u.VOUT -> C("22uF", "0805") -> GND;
    u.GND -> GND;
}

fn power_led(rail) {
    rail -> R("1k") -> LED("green") -> GND;
}

sheet("Power", || {
    let j = USB_C_Power();
    j.VBUS -> Fuse("500mA", "1206") -> vbus;
    [j.GND, j.SHIELD] -> GND;
    j.CC1 -> R("5k1") -> GND;
    j.CC2 -> R("5k1") -> GND;

    ldo(vbus, P3V3);
    power_led(P3V3);
});
```

## Values

Values are strings, written the way engineers write them: `"10k"`, `"4k7"`, `"100nF"`, `"16MHz"`.
wirx parses and checks the units. Numbers are accepted when you compute a value, in base units
(ohms, farads, henries):

```rhai
pb1 -> R("330") -> LED("red") -> GND;

let r = (5.0 - 2.0) / 0.010;                 // 300 Ω
pb1 -> R(e24(r)) -> LED("red") -> GND;       // e24/e96 snap to standard values
```

## Parts

Standard parts are functions. The arguments are `(value, footprint)` and both are optional. A
footprint can be an alias (`"0603"`) or a full name. Anything else goes in a trailing map:

```rhai
let r1 = R("10k");
let c1 = C("100nF", "0402");
let c2 = C("10uF", "Capacitor_SMD:C_0805_2012Metric");
let r2 = R("0", "0603", #{ ref: "R10", mpn: "RC0603JR-070RL", dnp: true, LCSC: "C21189" });
let j  = Conn(6);                            // single-row header, 6 pins
let isp = Conn2(3);                          // dual-row 2×3, odd/even
```

Standard library: `R`, `C`, `C_Pol`, `L`, `Fuse`, `Crystal`, `D`, `D_Schottky`, `D_Zener`, `LED`,
`Q_NPN`, `Q_PNP`, `Q_NMOS`, `Q_PMOS`, `SW_Push`, `SW_SPST`, `AMS1117_3V3`, `NE555`,
`USB_C_Power`, `Conn(n)`, `Conn2(rows)`.

### Custom parts

A custom part is a function that returns `part(#{ … })`. Being a function, it can be called from
anywhere, including inside other functions and from imported files:

```rhai
fn ATmega328P() {
    part(#{
        name: "ATmega328P",
        prefix: "U",
        value: "ATmega328P-AU",
        footprint: "Package_QFP:TQFP-32_7x7mm_P0.8mm",
        datasheet: "https://…",
        mpn: "ATMEGA328P-AU",
        symbol: "box",                       // default; or "resistor", "capacitor", "npn", "led", …
        pins: [
            [29,        "~RESET", input],    // [number(s), name, type, side?]
            [7,         "XTAL1",  input],
            [8,         "XTAL2",  output, left],
            [12,        "PB0",    bidirectional],
            [[4, 6],    "VCC",    power_in],
            [[3, 5, 21], "GND",   power_in],
            ["A9",      "VBUS",   passive],  // non-numeric pin numbers as strings
        ],
    })
}

let u = ATmega328P();
```

- **Pin types** are built-in names: `input`, `output`, `bidirectional`, `tri_state`, `passive`,
  `power_in`, `power_out`, `open_collector`, `open_emitter`, `free`, `unspecified`, `no_connect`.
- **Sides:** `left`, `right`, `top`, `bottom`. Without a side, a `box` symbol puts inputs left,
  outputs right, supplies on top and grounds at the bottom.
- **Per-part options.** The constructor of a custom part takes no arguments; set one instance's
  reference, value, footprint, part number, fields or DNP with `.options(#{ … })`, the same keys as
  a standard part's options map. It works on standard parts too.

  ```rhai
  let u = ATmega328P().options(#{ ref: "U3", value: "ATmega328PB-AU", mpn: "ATMEGA328PB-AU" });
  let spare = ATmega328P().options(#{ dnp: true });
  ```

### Symbols

A custom part is drawn as a generated box by default. Two other ways give it a proper symbol.

**Lay the symbol out in wirx** with `symbol: #{ … }`. Each side lists its pins in order: top to
bottom on the left and right, left to right on the top and bottom. `gap` leaves a blank place
between groups. A row with several numbers is a stacked pin: drawn once, the other numbers hidden
behind it (eight IOVDD pins, one line). The part's pins come from the symbol, so there is no
separate `pins` list:

```rhai
fn LDO() {
    part(#{
        name: "TLV75533",
        footprint: "Package_TO_SOT_SMD:SOT-23-5",
        symbol: #{
            left:   [[1, "IN", power_in], gap, [3, "EN", input]],
            right:  [[5, "OUT", power_out], gap, [4, "NC", no_connect]],
            bottom: [[2, "GND", power_in]],
            // width: 1200, height: 800      optional minimum body size in mils
        },
    })
}
```

**Use an existing symbol** with `library:` (relative to the design file) and `symbol:` (the
symbol's name or full id). eleceda libraries (`.elib`), the symbols embedded in an eleceda
schematic (`.esch`) and KiCad libraries (`.kicad_sym`) all work, and the pins come from the
symbol:

```rhai
fn RP2040() {
    part(#{ name: "RP2040", library: "libs/MCU_RaspberryPi.kicad_sym", symbol: "RP2040" })
}
```

Library pin names keep short forms in scripts: a pin named `GPIO40/ADC0` answers to `u.GPIO40`
and `u.ADC0`, and `~{QSPI_SS}` answers to `u.QSPI_SS`. Multi-unit symbols (op-amp packs) are not
supported yet.

**Symbols resolve in other tools.** `wirx build design.wx` writes the design's symbols to
`design.elib` beside `design.esch` and lists it in the schematic's project libraries. eleceda
desktop loads it when it opens the file, so the symbols appear in its Library panel. For KiCad,
`wirx lib export design.wx -o design.kicad_sym` writes them as a KiCad library (`-o x.elib` writes
an eleceda one). Symbols written either way come back unchanged through `library:`.

## Pins

```rhai
u.VIN                  // by name
c1[1]                  // by number
j["A9"]                // a number that isn't a plain integer
u["~RESET"]            // a name that isn't a valid Rhai identifier
u.pin("VIN")           // explicit form, same as u.VIN
u.pin(1)               // explicit form, same as u[1]
u.pins(power_in)       // all pins of a type, as an array
u.pins("PB")           // all pins whose name starts with "PB", in order
```

A misspelt pin is an error at that line, with a suggestion: ``part `AMS1117_3V3` has no pin `VI`;
did you mean `u.VIN`?``

## Connections: `->`

`a -> b` connects `a` and `b` and evaluates to `b`, so chains continue left to right.

| On either side of `->` | Meaning |
| --- | --- |
| a pin (`u.VIN`, `c[1]`) | that pin |
| a net or rail (`sda`, `GND`, `P3V3`) | that net |
| an array (`[u.GND, j.SHIELD]`) | every element (fan in / fan out) |
| a two-pin part, inline (`R("1k")`) | passes through: the left side joins its *in* pin, the chain continues from its *out* pin |

In and out pins of inline parts:

| Part | in → out |
| --- | --- |
| R, C, L, Fuse, Crystal, switches | 1 → 2 |
| D, LED, Schottky, Zener | A → K (current flow) |
| C_Pol | + → − |

```rhai
vin -> u.VIN -> cin[1];                      // one net with three members
pb1 -> R("330") -> LED("red") -> GND;        // pb1 → R → LED → GND: three nets
GND -> [u.GND, cin[2], cout[2]];             // fan out
[j.GND, j.SHIELD] -> GND;                    // fan in
VBAT -> R("100k") -> vsense -> R("47k") -> GND;   // divider with a named tap
```

The order of a chain is also a layout hint: the parts are drawn along it in that order.

### Unused pins

```rhai
nc(u.PB0);
nc([u.PB2, u.PC3, u.ADC6]);
u.PB4 -> NC;
```

## Nets

```rhai
let sda = net("SDA");          // design-wide named net: the same name is the same net everywhere
let tap = net();               // anonymous net, named automatically
```

A pin can stand in for a net: `let fb = u.FB; fb -> R("10k") -> GND;`.

## Rails and power

Rails are drawn as power symbols at every pin, never as long wires.

- **Built in:** ground rails `GND`, `AGND`, `DGND`, `PGND`; supply rails `P1V8`, `P2V5`, `P3V3`,
  `P5V`, `P12V`, `VBUS`, `VBAT`, `VCC`, `VDD`; negative rails `N5V`, `N12V`.
- **Your own:** `supply("VIO")`, `ground("GND_ISO")`, `negative("-15V")`. The same name always
  means the same rail, so you can write `supply("VIO")` again inside any function.
- **`.driven()`** marks a rail as powered from off-board and adds a PWR_FLAG:
  `let vbus = VBUS.driven();`.

```rhai
let vio  = supply("VIO");
let vref = supply("+2V5");                   // any name, as a string
```

## Groups and sheets

- **Every function call that places two or more parts** becomes a framed group named after the
  function. A second call site of the same function is named "ldo 2", and so on. Calls from one
  call site in a loop share one group (eight `indicator(…)` calls in a loop draw as one block).
  wirx sees the call stack, so no wrapper is needed.
- **Nested calls** are drawn inside the caller's group.
- **To name a group yourself:** `group("3V3 regulator", || { … });`.
- **Sheets:** `sheet("Power", || { … });` makes a page. Statements outside any sheet go to a main
  sheet named after the file.

```rhai
fn ldo(vin, vout) { … }                      // framed group "ldo"

sheet("Power", || {
    ldo(vbus, P3V3);                         // group "ldo"
    ldo(P5V, supply("+1V8"));                // group "ldo 2"
    group("Status", || {
        P3V3 -> R("1k") -> LED("green") -> GND;
    });
});
```

**Built-in names.** Rails, pin types (`input`, …), sides (`left`, …) and `NC` are supplied only
when no variable, parameter or captured variable has that name, so your own names always win.
Shape names are strings (`symbol: "npn"`) so they never clash with variables like `led`.

**Scoping rule (from Rhai).** A `fn` cannot see variables created with `let` outside it. Pass
nets in as parameters, which are a module's ports anyway. Built-in rails, `supply("…")` by name
and `net("…")` by name work everywhere. Closures (`|| { … }`, as in `sheet` and `group`) can see
outer variables.

## Repetition and logic

This is plain Rhai:

```rhai
for i in 0..8 {
    u.pin(`PB${i}`) -> R("330") -> LED("red") -> GND;
}

for p in u.pins(power_in) {                  // one decoupling cap per supply pin
    p -> C("100nF", "0402") -> GND;
}

if with_usb { usb_input(vbus); }
```

## Layout hints (optional)

```rhai
near(c1, u.VDD);       // keep c1 at that pin
flow(down);            // main signal direction in the current group
```

**Decoupling banks.** Capacitors between one supply and ground are drawn as a bank: side by side,
one supply wire over them, one ground return under them, one power symbol for each rail, largest
value farthest from the pin. The bank stands at the pin it serves, joined to it by a wire. That
pin is, in order:

1. a `near(cap, u.VDD)` hint (never overridden);
2. the pin the capacitor was chained to: `u.VDD -> C("100nF") -> GND`, or
   `for p in u.pins(power_in) { p -> C("100nF") -> GND; }`;
3. the group's only power pin (`power_in` / `power_out`) on that rail.

Several power pins on the rail and no hint: the capacitors still form a bank, drawn on their own,
not joined to a guessed pin. Write `near` (or chain from the pin) when it matters which pin a
capacitor belongs to. Neighbouring pins on one rail (two VCC pins side by side) count as one
place: their short bus stays and the bank continues it. Capacitors on negative rails keep their own
symbols. A capacitor on a signal (reset, timing, filter, tank) is not a bank: it stays on its
signal. Polarized capacitors are drawn as wired, + on the side it is connected to.

Banks on one chip edge fan out: the outermost pins' banks are placed first, toward the nearer
corner, and the inner ones pass over them. On a crowded edge a bank that finds no clear spot is
drawn on its own instead; give the pin room in the symbol (`gap`) if it should stay at its pin.
`wirx import` writes `near(cap, chip.pin)` for each capacitor drawn wired to a chip pin, so banks
survive a round trip through eleceda.

Banks are a drawing convention. They do not mean the PCB placement or the decoupling has been
checked.

## Imports and title block

```rhai
import "parts/mcu" as mcu;                   // parts/mcu.wx
let u = mcu::ATmega328P();

title("ATmega328P board");
revision("B");
author("…");
company("…");
```

## Errors

- **Located errors.** Errors keep the current quality: Rhai syntax errors and wirx errors (unknown
  pin, unconnected pin, pin-type conflict, …) point at the line that caused them, with a hint.
  `check --json` stays machine-readable.
- **Errors from inside a loop or function** also name the call site that led there.

## Compared with today

| Today (`.wx` DSL) | Rhai |
| --- | --- |
| `cin = C(10uF, 0805)` | `let cin = C("10uF", "0805");` |
| `vin -- u.VIN -- cin.1` | `vin -> u.VIN -> cin[1];` |
| `GND -- u.GND, c.2` | `GND -> [u.GND, c[2]];` |
| `module Ldo(vin, vout) { … }` | `fn ldo(vin, vout) { … }` |
| `reg = Ldo(vin: VBUS, vout: +3V3)` | `ldo(VBUS, P3V3);` |
| `sheet "Power" { … }` | `sheet("Power", \|\| { … });` |
| `+3V3`, `rail VIO : supply` | `P3V3`, `supply("VIO")` |
| `power_flag VBUS` | `VBUS.driven()` |
| `nc u.5, u.6` | `nc([u[5], u[6]]);` |
| `part X { pins { … } }` | `fn X() { part(#{ pins: [ … ] }) }` |
| (not possible) | loops, conditions, computed values, inline parts |
