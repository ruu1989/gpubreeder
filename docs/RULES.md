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

- Growth rate is overall quality times (1 + 0.25 per G gene).
- Blueberry, orchid and potato take 1, 10 and 10 base minutes for Seed, Seedling and Sapling.
  Other crops have not been read.
- A neighbour that reaches Crossbreed before the centre does changes its own genes first. Planting
  neighbours once the centre is about 50% through Sapling prevents that under equal conditions.
