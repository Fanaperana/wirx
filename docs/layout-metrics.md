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
candidate. The budget is 12 iterations, 20 ms, or 4 attempts in a row without improvement. Only a
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
