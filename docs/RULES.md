# Rust crossbreeding rules

These rules were read from the decompiled Rust dedicated server, Steam build 26431843, on
2026-11-06. Check them again after a game update changes the genetics code. Decompiled game code
is deliberately not stored in this project.

## Genes

- Every plant has six slots holding G, H, Y, W or X.
- W and X weigh 1.0. G, H and Y weigh 0.6. All crops share these values.

## When a plant crossbreeds

- A plant crossbreeds once, as it enters the Crossbreed stage. It then keeps those genes.
- It reads every neighbour in the same planter within 1.5 m that is the same plant type and alive.
- Its own weight is not added to the neighbours' totals.
- Clones keep their parent's genes. Nothing else changes genes, and there is no random mutation.

## Choosing each slot

For each slot, the game walks the neighbours in the planter's stored order, adding each gene's
weight to that gene's total. The leading gene only changes when a total becomes strictly higher.

- **Ties are not random.** The tied gene that reached the top total first wins, which means the
  gene whose last copy was planted earlier. Sites that show 50% are describing a choice you make.
- A winner replaces the centre's gene only if its total is strictly above the centre gene's own
  weight. On equal weight the centre keeps its gene.
- With two or more ties, one planting order has to satisfy all of them, and sometimes none can.

## Planting order

- The stored order is planting order while the server is running. Removing a plant keeps the rest
  in order.
- A server restart rebuilds the order from save-file order, which is not guaranteed to match.
  Plans that need a specific order should not span a restart.

## Timing

Read from the crop assets and `server.planttick` defaults. Modded servers can change the tick.

- Plants advance on a 60 second tick, each landing up to 10% early or late. Each tick adds 60
  seconds times the growth rate to the stage's age. A stage ends when its age is strictly above its
  length, and the remainder is thrown away. Time is therefore whole ticks.
- Growth rate is overall quality times (1 + 0.25 per G gene). Overall quality is the lowest of
  light, water, ground and temperature. Crossbreed and Ripe ignore genes and conditions.
- Every crop spends 1, 10 and 10 base minutes in Seed, Seedling and Sapling. Clones also start at
  Seed. That makes 24, 19, 15, 13, 13, 11 and 11 minutes to Crossbreed for 0 to 6 G at full quality.
- Crossbreed lasts 1 base minute, or 5 for berries, which is 2 or 6 ticks.
- Clones can be taken in Sapling, or from Mature onwards, but not during Crossbreed. Taking them
  removes the plant. One plant gives 2 clones, or 1 for orchid, plus 1 per two Y genes.
- Each quality is scaled by a per-crop optimum and capped at 1. Berries use 0.8, potato 0.7, and
  everything else 0.9, with water at 0.85.
- A planter gives ground quality 0.6, or 1.0 with fertiliser. An unfertilised planter therefore
  limits hemp to 67%, berries to 75% and potato to 86%.
- Planter water is best near 60% saturation. A full planter counts as 0.6 water quality.
- A neighbour that reaches Crossbreed before the centre does changes its own genes first, and the
  centre then reads the changed genes. A slow centre with fast neighbours needs them planted late.
  The guide plans for every tick landing at the unluckiest end of its jitter.
