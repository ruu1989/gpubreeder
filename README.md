# GPUBreeder

Finds crossbreeding plans for plants in Rust, using the game's exact rules and your GPU. Ties in
the game are decided by planting order, so plans give an exact order where it matters instead of a
percentage. See [docs/RULES.md](docs/RULES.md).

## Use

```bash
cargo run --release -- --genes "WGXYYW,WGGXYW,WGYXGW" --target GGGYYY
```

| Option | Default | Meaning |
|---|---|---|
| `-g, --genes` | | Your plants, separated by spaces or commas |
| `-f, --file` | | Read your plants from a file instead |
| `-t, --target` | | Stop at this exact genotype |
| `--generations` | 3 | Generations to search |
| `--carry` | 200 | Most plants carried into each next generation |
| `--plants` | 5 | Most neighbours around a centre, 1 to 8 |
| `--min-score` | 3.0 | Lowest score kept as a stepping stone |
| `--top` | 3 | Plans to print when there is no target |
| `--cpu` | off | Use the CPU instead of the GPU |

Work grows very quickly with `--plants` and `--carry`. Set `GPUBREEDER_TRACE=1` to see each batch.

## How it stays correct

1. `rules::game_cross` follows the game step by step, including 32-bit float weights.
2. The GPU and CPU searches use faster integer logic.
3. Every result must pass `game_cross` with a concrete planting order, or the run stops with an
   error. Each plan is then replayed from your starting plants before it is printed.

## Tests

```bash
cargo test --release -- --test-threads=1
```

The tests cover the following:

- Predictions against the reference rules across every planting order.
- CPU search against brute force.
- GPU results against CPU results.
- Results against the live rustbreeder.com v3.0.4 at default options.

GPU tests need a GPU.
