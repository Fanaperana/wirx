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
