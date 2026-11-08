# Changelog

Each release's notes on GitHub come from its section here, so write them for players.

## 0.1.0-rc.1

The first test build. Please report anything that doesn't match what the game does.

### What it does

- **Finds the shortest route** to a target such as 5G1Y. It searches cheapest-first on the
  graphics card, and when it reaches the target it can say whether the route is the minimum.
- **Gets the planting order right.** The game settles tied genes by planting order, not by chance.
  Plans say when order matters and give the exact order.
- **Scans your clones** from the Rust window at any resolution or aspect ratio, ultrawide included.
  You can also type or paste a list.
- **Guides you through planting.** Each wait names what to look for on the plant, such as "the
  centre shows Seedling 35%, and no later than Sapling 70%". The timer is only an estimate to
  prompt a look. You can step back, skip, or click any step.
- **Breeds several crops at once.** Each crop gets its own tab with its own clones, plans and
  guide. Guides run side by side, and each has its own alarm.
- **Use fertiliser** and **Max water** or **Optimise water** set the timings. Light and heat are
  taken as perfect.
- `gpubreeder-cli --selftest` checks your graphics card against the CPU, and `--benchmark`
  measures its speed.

### Known limits

- The download isn't signed yet, so Windows SmartScreen will warn the first time it runs. Choose
  **More info**, then **Run anyway**.
- GPU temperature, power and clocks show for NVIDIA cards only. Searching works on any card with
  Vulkan or DirectX 12.
- Only an RTX 4090 has been tested so far. If `--selftest` fails on your card, please open an issue
  with its output.
- The guide's stage percentages assume the stage progress the game shows. If yours reads
  differently, please say so.
