# Layout metrics

Measured with `wirx stats <example>`, release build. Each stage can be switched off to measure
it on its own (`--no-candidates`, and later `--no-reroute` / `--no-cleanup`).

- **crossings:** wires of different nets crossing; stubs to labels and power symbols are included.
- **bends:** wire corners.
- **length:** routed wire length in mils.
- **collisions:** visible text covering wires, labels, other parts or other text, plus part
  bodies that overlap.
- **cand:** the selected placement candidate, out of the number tried.
- **ms:** layout time for the group.

Fixtures:
- **Regression fixtures:** the USB block (`ldo`/`Power`, `mcu`/`Power`), the LDO (`ldo`/`ldo`)
  and the blinker (`blinker`/`astable`).
- **Improvement fixture:** the MCU block (`mcu`/`mcu`), which had the one remaining crossing.

The acceptance rule is that an optimisation never replaces a better result with a worse one. The
test `optimisations_never_make_a_group_worse` checks it for every group of every example.

## Stage 1: candidate placement

Each group is placed with 8 deterministic candidates (gap 300 or 400 mil, branch order bottom-up
or top-down, off-line shift direction), and each candidate is routed independently. Candidate 0
is the previous placement. A candidate with more collisions or unrouted pins than candidate 0 is
rejected. The others are ranked by (crossings, bends, length, displacement), and only a strictly
better one wins.

| group | before: crossings / bends / length | after: crossings / bends / length | cand | moved | ms before → after |
| --- | --- | --- | --- | --- | --- |
| ldo/Power (USB) | 0 / 9 / 2250 | 0 / 9 / 2250 | 0/8 | 0 | 1.5 → 8.2 |
| ldo/ldo | 0 / 0 / 0 | 0 / 0 / 0 | 0/8 | 0 | 1.9 → 2.4 |
| blinker/astable | 0 / 14 / 5400 | 0 / 14 / 5400 | 0/8 | 0 | 6.3 → 38.5 |
| mcu/Power (USB) | 0 / 6 / 1750 | 0 / 6 / 1750 | 0/8 | 0 | 2.9 → 10.1 |
| **mcu/mcu** | **1** / 8 / 3050 | **0** / 11 / 3000 | 4/8 | 650 | 1.1 → 10.4 |

- **Collisions and unrouted pins:** 0 everywhere, before and after.
- **eleceda readability:** the MCU sheet goes from 99 to 100; every other sheet stays at 100.
- **Total layout time:** ldo 3.7 → 11.4 ms, blinker 6.6 → 39.7 ms, mcu 5.0 → 24.1 ms.
- **MCU block:** the crossing disappears with the top-down branch order (candidate 4). It costs 3
  more bends and saves 50 mil of wire; crossings rank first.

## Stage 2: crossing-driven rerouting

After the first routing, the nets involved in crossings between different nets are ripped up and
rerouted with every other net in place. Attempts cycle through three deterministic orders
(original, reversed, rotated). Cells around a recurring crossing get more expensive each time
(+60 per recurrence, 3×3 cells around it). This congestion history is local to each placement
candidate. The budget is 12 iterations, 200 net reroutes, or 4 attempts in a row without improvement. (The first version used a 20 ms wall-clock budget, which made the output depend on machine speed; the budget now counts work.) Only a
strictly better result on (unrouted, crossings, bends, length) replaces the best one.

Rerouting alone (default placement, `--no-candidates`), stage 1 off vs on:

| group | before: crossings / bends / length | after: crossings / bends / length | ms before → after |
| --- | --- | --- | --- |
| ldo/Power (USB) | 0 / 9 / 2250 | 0 / 9 / 2250 | 1.6 → 1.5 |
| ldo/ldo | 0 / 0 / 0 | 0 / 0 / 0 | 2.8 → 1.8 |
| blinker/astable | 0 / 14 / 5400 | 0 / 14 / 5400 | 6.5 → 6.5 |
| mcu/Power (USB) | 0 / 6 / 1750 | 0 / 6 / 1750 | 3.4 → 3.2 |
| **mcu/mcu** | **1** / 8 / 3050 | **0** / 9 / 3650 | 1.2 → 6.4 |

Both stages on (the default):

| group | stage 1 only | stages 1 + 2 | cand | moved | ms |
| --- | --- | --- | --- | --- | --- |
| ldo/Power (USB) | 0 / 9 / 2250 | 0 / 9 / 2250 | 0/8 | 0 | 37.8 |
| ldo/ldo | 0 / 0 / 0 | 0 / 0 / 0 | 0/8 | 0 | 2.3 |
| blinker/astable | 0 / 14 / 5400 | 0 / 14 / 5400 | 0/8 | 0 | 44.0 |
| mcu/Power (USB) | 0 / 6 / 1750 | 0 / 6 / 1750 | 0/8 | 0 | 39.5 |
| **mcu/mcu** | 0 / 11 / 3000 (cand 4) | 0 / **9** / 3650 (cand 0) | 0/8 | **0** | 55.7 |

- **Selected candidate:** with rerouting, the default placement routes without crossings and with
  fewer bends than candidate 4. The search now keeps it, with no parts moved.
- **Collisions and unrouted pins:** 0 everywhere. eleceda readability is 100 on every sheet.
- **Total layout time:** ldo 40.8 ms, blinker 45.0 ms, mcu 99.2 ms. Candidates that start with
  crossings spend part of the reroute budget.

## Stage 3: cleanup

1. **Jog removal and track separation.** Each net is rerouted on its own with every other net in
   place. This pass charges an extra 60 per bend and 25 per cell run right next to another net's
   parallel wire. A new route is kept only when the whole group gets strictly better.
2. **Safe collinear merging.** Collinear pieces of the same net are joined where they meet at a
   plain degree-two point. Pin tips, junctions, branches, label anchors, stub ends and power symbol
   points are never removed. The merged wires are kept only if the group's pin connectivity,
   rebuilt from geometry, is identical to before. (On these fixtures the router already draws
   corner-only paths, so merging changes nothing yet.)
3. **Junctions and verification.** Junctions are recomputed afterwards, and the full sheet still
   goes through the connectivity self-check.

The first cleanup run made a four-way point in the blinker: eleceda warned
`four_way_junction`, readability 97, and the metrics did not see it. Four-way points are now
measured and are a hard rejection for candidates. Every optimisation also uses one shared quality
order, `GroupStats::rank`: unrouted, crossings, four-way points, bends, hugging pairs, length,
then displacement. Stage 1's order (crossings, bends, length, displacement) is kept inside it.

Before = stages 1 + 2 (`--no-cleanup`), after = all stages:

| group | before: crossings / bends / length / hugs | after: crossings / bends / length / hugs | cand | ms before → after |
| --- | --- | --- | --- | --- |
| ldo/Power (USB) | 0 / 9 / 2250 / 0 | 0 / 9 / 2250 / 0 | 0/8 | 36.8 → 59.8 |
| ldo/ldo | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 | 0/8 | 3.1 → 2.7 |
| blinker/astable | 0 / 14 / 5400 / **1** | 0 / 14 / 5600 / **0** | 0/8 | 44.0 → 75.4 |
| mcu/Power (USB) | 0 / 6 / 1750 / 1 | 0 / 6 / 1750 / 1 | 0/8 | 39.4 → 70.4 |
| **mcu/mcu** | 0 / 9 / 3650 / 1 | 0 / **8** / **3250** / 2 | 0/8 | 56.3 → 76.8 |

Four-way points, collisions and unrouted pins are 0 everywhere. eleceda readability is 100 on
every sheet.

## Summary: all stages vs the original layout

`--no-candidates --no-reroute --no-cleanup` vs the default:

| group | original: crossings / bends / length | now: crossings / bends / length | total ms (whole design) |
| --- | --- | --- | --- |
| ldo/Power (USB) | 0 / 9 / 2250 | 0 / 9 / 2250 | ldo: 3.6 → 63.3 |
| ldo/ldo | 0 / 0 / 0 | 0 / 0 / 0 | |
| blinker/astable | 0 / 14 / 5400 (1 hug) | 0 / 14 / 5600 (0 hugs) | blinker: 7.3 → 76.8 |
| mcu/Power (USB) | 0 / 6 / 1750 | 0 / 6 / 1750 | mcu: 5.2 → 153.4 |
| **mcu/mcu** | **1** / 8 / 3050 | **0** / 8 / 3250 | |

The test `optimisations_never_make_a_group_worse` checks the acceptance rule for every group of
every example: the optimised layout is never worse than the plain default on `GroupStats::rank`
(displacement aside), and never has more collisions, unrouted pins or four-way points.
`layout_fixtures` keeps the four fixtures at zero crossings, collisions, unrouted pins and
four-way points.

## Stage 4: crowded chip edges and sheet packing (RP2350B board)

The RP2350B board (`examples/rp2350b.wx`) is the first design whose MCU sheet holds one large chip
and several small partner blocks (crystal, flash, buttons, SWD). Before this stage its MCU sheet
scored 70 in eleceda: the core-regulator parts above the chip's crowded top edge could not be
routed, and the small blocks sat in one row of equal-height frames, far from the pins they serve.

1. **Crowded top edge.** The PWR_FLAG of a signal net (`VREG_AVDD`) now goes on a small part's
   pin instead of the chip's, where its symbol blocked the neighbouring pins 100 mil away. On that
   pin it stands 250 mil to the side of the stub (its name clears the part), and the router
   reaches a pin whose own flag stub fills the space in front of it through the stub's corner.
2. **Decoupling rows wrap** at the width of the circuit above them, so 15 caps under the chip no
   longer make the block (and the distance to its neighbours) twice as wide.
3. **Sheets drawn around their biggest group** (`pack::hub`). Groups sharing labels with it stand
   in a column beside the side those labels leave from; groups wired only to a column member join
   that column; the rest go in a row underneath. Every frame fits its circuit. The order of each
   column is searched (all orders up to 6 blocks): blocks settle level with their labels (pool
   adjacent violators, weighted 10× for blocks whose labels sit on small parts), and the order
   with the least label-to-partner distance wins, distance beyond 1.5 in on a small part's label
   costing most. Sheets with no such partner keep the grid.
4. **Clusters face their partner.** Inside a group, clusters go left to right so that the cluster
   for the lowest pin of the chip side they connect to is nearest that side (on the buttons
   block: RESET for `RUN` outside, BOOTSEL for `QSPI_SS` nearer the chip), so their wires to the
   chip need not cross.
5. **Wires between blocks** (`join.rs`). After packing, a label on a small part becomes a wire to
   the same net's label in another block when one of at most 3 in and 3 corners fits without
   crossing or touching anything, and without entering a third block. When the net has no other
   label left, both labels go; otherwise one stays (on a crowded chip edge the chip's label goes
   and the small part's label turns to lie along the new wire).

Group stats (`wirx stats examples/rp2350b.wx`), before → after; the other groups are unchanged:

| group | cand | crossings / bends / length | collisions | unrouted | ms |
| --- | --- | --- | --- | --- | --- |
| rp2350b/mcu | 0/8 → 4/8 | 0 / 0 / 0 → 0 / 7 / 2100 | 1 → **0** | 3 → **0** | 945 → 102 |
| rp2350b total | | 4 / 28 / 11000 → 4 / 35 / 13100 | 5 → 4 | 3 → **0** | 1250 → 430 |

The extra bends and length are the regulator parts now wired instead of labelled. Join wires are
drawn after the group stats are taken, so they are not in this table.

eleceda (`tools/verify-esch.mts`), RP2350B sheets:

| sheet | before | after |
| --- | --- | --- |
| Power | 92 (4 crossings, 2 text overlaps) | 92 (unchanged) |
| **MCU** | **70**: label-only R6, C3, L1, SW2; far R7 5.6 in, R9 5.0 in; 2 × labels could be a wire; 1 text overlap; 3 `unrouted` warnings | **100**: nothing flagged, no warnings |
| IO | 96 (2 text overlaps) | 96 (unchanged) |

How the MCU sheet got there, one change at a time:

| step | MCU readability | what was left |
| --- | --- | --- |
| before | 70 | see above |
| 1. flag off the chip edge, routing through flag stubs | 87 | SW2 label-only, R7 and R9 far |
| 2–3. wrapped cap row, hub packing | 91 | SW2 label-only, R9 far, flag text |
| 5. wires between blocks, column order search | 94 | R9 2.3 in from U3, flag text |
| 4. clusters face their partner | 98 | flag text |
| 1. flag 250 mil beside its stub | **100** | |

- **ldo, blinker, mcu:** readability 100 on every sheet, no errors or warnings, and the group
  stats above are unchanged. Their sheets do change: the blinker's indicator is wired to the 555's
  output and the mcu example's status LED to PB1 instead of by label, and the mcu example's MCU
  sheet is drawn around the ATmega block.
- `optimisations_never_make_a_group_worse` and `layout_fixtures` pass; `rp2350b_board_builds`
  now also requires no layout warnings (no unrouted pins).

## Stage 5: Power and IO sheets of the RP2350B board

The Power sheet lost points to the USB input block (connector → USBLC6 ESD → 27 Ω series
resistors) and to two text overlaps; the IO sheet to two header supply symbols. Baseline: the
tree before this stage, with the crossing reroute budget counted in work (200 net reroutes per
candidate) instead of 20 ms, so the numbers do not depend on machine speed. That alone already
took `usb_input` from 4 to 2 crossings; every number below was measured with it.

1. **ESD pin order** (`examples/rp2350b.wx`). The USBLC6 symbol listed IO1 (DM) above IO2 (DP),
   the opposite of the connector's DP-above-DM, so the two lines could not reach it without
   crossing; the router avoided the crossing by taking DM all the way round the connector. The
   symbol now lists IO2 first. Pin numbers and the netlist are unchanged (still the same netlist
   as eleceda's reference board).
2. **Hanging parts in crossing order** (`uncross_hangs`). Two parts hanging down off pins on
   one side of a part (the CC1/CC2 pull-downs) are swapped across when the part on the upper
   pin stands nearer, which makes its wire cross the other's; the part on the upper pin goes
   further out. Hanging up, mirrored. The swap is kept only when both fit.
3. **Side-pin rail symbols clear their neighbours** (`side_rail_len`). A supply symbol on a
   side pin stands up across the rows above it (a ground symbol down across the rows below). Its
   stub now grows until the symbol clears those rows' labels and rail symbols, instead of a fixed
   250 mil that put `+3V3` on the header's GPIO30/GPIO31 labels and `+5V` on pin 33's stub.
4. **Text order fallback** (`text::place`). When placing part text first leaves a power symbol
   name with no clear spot (`+5V` under D1's value), the power names are placed first as well,
   and the result with fewer collisions is kept.

Group stats (`wirx stats examples/rp2350b.wx`), before → after; the other groups are unchanged:

| group | cand | crossings / bends / length | hugs | collisions | ms |
| --- | --- | --- | --- | --- | --- |
| rp2350b/usb_input | 6/8 → 2/8 | 2 / 22 / 10800 → **0** / 21 / 7850 | 1 → 2 | 0 → 0 | 650 → 460 |
| rp2350b/Power | | | | 2 → **0** | |
| rp2350b/headers | | | | 2 → **0** | |
| rp2350b total | | 2 / 35 / 15550 → **0** / 34 / 12600 | | 4 → **0** | 815 → 640 |

eleceda (`tools/verify-esch.mts`), RP2350B sheets:

| sheet | before | after |
| --- | --- | --- |
| **Power** | **94**: 2 crossings in usb_input, 2 text overlaps (D1 Value, `+5V` name) | **100**: nothing flagged |
| MCU | 100 | 100 |
| **IO** | **96**: 2 × `+3V3` name covers the drawing | **100**: nothing flagged |

(Power was 92 with 4 crossings with the 20 ms budget on this machine.)

One change at a time on the Power sheet: ESD pin order 94 → 95 (the twist goes, CC1/CC2 still
cross once), hanging-part swap → 96, text order → 100. The side-rail stubs alone take IO to 100.

- **ldo, blinker, mcu:** readability 100 on every sheet, no errors or warnings; group stats
  unchanged. The mcu example's Connectors sheet changes: J3's `+5V` symbol stands clear of the
  A1 label (pin 5's stub 250 → 300 mil), so the block is 50 mil wider.
- Tried and dropped: drawing the USBLC6 pass-through (IO in on the left, out on the right, as in
  KiCad's symbol). eleceda does not join same-name pins, so both pins of a channel must be wired
  together round the body, which drew a loop around the ESD on each line.
- `optimisations_never_make_a_group_worse`, `layout_fixtures` and `rp2350b_board_builds` pass.

## Stage 6: decoupling banks

Until now, every decoupling capacitor was a rail-only part with its own supply and ground symbol,
standing in a row under its circuit. Connectivity was right, but nothing on the page showed which
pin a capacitor served. Now (`layout/group/bank.rs`) the capacitors between one supply and ground
form a bank: one supply wire over them, one ground return under them, one symbol for each rail at
the far end. The bank is drawn at the pin it serves (a `near` hint, then a chain from the pin,
then the group's only power pin on that rail), and the pin's own power symbol gives way to the
wire into the bank. Banks with no such pin are drawn on their own, split into banks no wider than
their circuit. A chip that sits between rails (a regulator in the row under its circuit) brings
its banks along as one unit, so they fit beside its pins.

Chessboard fixture (`examples/chessboard.wx`), before → after:

| group | before | after |
| --- | --- | --- |
| ESP32 controller | C6 10uF, C7 100nF under the circuit, 2 × `+3V3`, 2 × GND | bank at U2.VCC: `VCC ─ C7 ─ C6 ─ +3V3`, one GND return |
| 3V3 regulator | C3, C4, C5 in a row, 6 symbols | C3 (+5V) at VIN; C5 100nF, C4 22uF (+ up) at VOUT; 4 symbols |
| 5V input | C1, C2 each with +5V and GND | one +5V bank, one GND return (no chip pin on +5V: drawn on its own) |
| Eight sink drivers | 2 caps, 4 symbols | one +5V bank on its own (ULN COM is a passive pin, not a supply input) |
| decoders, gates, mux, buffers, ADC | one 100nF beside the chip, own symbols | the 100nF at the chip's VCC/VDD pin |
| Reference and calibration | C320, C321 in a row | C320 at IN (+3V3), C321 at OUT (+1V25_CAL) |
| ESP_EN (C8), CAL_HALF (C322), VCO, sensing cells | on their signals | unchanged |

- **Netlists:** identical for all six designs (`wirx netlist --format json`, byte for byte).
- **eleceda** (`tools/verify-esch.mts`): loads without repairs, connectivity matches wirx on every
  design. Readability is unchanged on every sheet: chessboard 97/98/76×8/100/100/97/100,
  rp2350b, ldo, blinker, mcu and tags 100. Warnings are unchanged: 66 on the chessboard, all
  from before (65 four-way points in the sensing cells, 1 open-drain ALERT), none elsewhere.
- **wirx stats** (signal routing only; bank wires are decorations): unchanged on every group
  except five. The four column buffers (LMV358) go from 4850 to 5650 mil of wire and from 0 to
  2 hugging pairs: the bank's wire at VCC blocks the feedback route over the chip, so both
  feedback loops run underneath. The swept oscillator goes from 12400 to 12800 mil. There are no
  new crossings, collisions or four-way points anywhere.
- **RP2350B MCU sheet:** the 15 capacitors become a +1V1 bank of 4 and two +3V3 banks (5 and 6),
  standing on their own because the chip has several supply pins on each rail. A single 11-cap
  bank was wider than the chip and pushed the buttons block too far for its wires (readability
  93); splitting at the circuit's width keeps the sheet at 100.
- **mcu:** VCC and AVCC are neighbouring pins sharing one `+5V` symbol, so C2 and C3 form one bank
  on their own. **blinker:** `near(cd, u.VCC)` gives a one-capacitor bank at VCC.

Tests: `decoupling_caps_share_one_bank_at_their_pin`, `separate_rails_make_separate_banks`,
`explicit_associations_pick_the_pin`, `polarized_caps_follow_their_wiring` and
`chessboard_decoupling_banks`.

### Stage 6, second pass: every example's decaps at their pins

- **Examples written explicitly.** chessboard.wx and rp2350b.wx chain each decoupling capacitor
  from the pin it serves (`u.VCC -> C("100nF") -> GND`). mcu.wx puts AREF between VCC and AVCC
  so each gets its own capacitor. The RP2350B symbol spaces its top-edge supply pins (`gap`) so
  every bank finds room. Netlists are unchanged (same nets and parts; only the order pins are
  listed in a net follows the new source order).
- **Runs of pins.** Neighbouring pins on one rail (the ATmega's two VCC pins) take a bank: their
  short bus stays and the bank's wire continues it.
- **Fan-out.** Banks on one edge are attached outermost pin first, toward the nearer corner, in a
  first pass that tries only that way; the rest then try both. Straight connections between pins
  already in line count as obstacles, so a bank never cuts across a wire that is routed later.
- **Text beside upright parts.** Capacitor reference and value text used to go above the top
  pin and below the bottom one, beyond a bank's wires or a power symbol. Now an upright two-pin
  part has it beside its body.
- **Import.** `wirx import` writes `near(cap, chip.pin)` for a capacitor drawn wired to one
  chip's pin, so the drawing survives a round trip.

eleceda readability, before stage 6 → now: chessboard Scan and VCO 98 → 100, Buffers and ADC
97 → 100, every other sheet unchanged (chessboard warnings 66 → 65; none elsewhere). Every
supply pin of the RP2350B, ATmega328P, ESP32, AMS1117, REF3312, ADS1115, CD4046, CD4051, LMV358,
74HC08, 74HC238 and W25Q128 now has its capacitors drawn at it. No two texts overlap on any sheet.

