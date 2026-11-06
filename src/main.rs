use anyhow::{bail, Context, Result};
use clap::Parser;
use gpubreeder::genes::*;
use gpubreeder::gpu::GpuBackend;
use gpubreeder::plan::*;
use gpubreeder::search::*;
use std::collections::BTreeMap;
use std::time::Instant;

/// Finds crossbreeding plans for Rust plants, using the game's exact rules.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Your plants, separated by spaces or commas, e.g. "WGXYYW,WGGXYW"
    #[arg(short, long)]
    genes: Option<String>,
    /// Read your plants from a file instead
    #[arg(short, long)]
    file: Option<String>,
    /// Stop at this exact genotype, e.g. GGGYYY
    #[arg(short, long)]
    target: Option<String>,
    /// Generations to search
    #[arg(long, default_value_t = 3)]
    generations: usize,
    /// Most plants carried into each next generation
    #[arg(long, default_value_t = 200)]
    carry: usize,
    /// Most neighbours around a centre, 1 to 8
    #[arg(long, default_value_t = 5)]
    plants: usize,
    /// Lowest score worth keeping as a stepping stone
    #[arg(long, default_value_t = 3.0)]
    min_score: f32,
    /// Number of plans to print when there is no target
    #[arg(long, default_value_t = 3)]
    top: usize,
    /// Use the CPU instead of the GPU
    #[arg(long)]
    cpu: bool,
}

fn print_plan(plan: &Plan, scores: &Scores) {
    println!();
    println!("=== {} | score {:.1} | generation {} | {} crossing(s) ===", plan.goal, plan.goal.score(scores) as f32 / 100.0, plan.generation, plan.steps.len());
    let inputs: Vec<String> = plan.inputs_used.iter().map(Genotype::to_string).collect();
    println!("Starting plants used: {}", inputs.join(" "));
    for (number, step) in plan.steps.iter().enumerate() {
        println!();
        println!("Step {}: make {}", number + 1, step.result);
        match step.centre {
            Some(c) => println!("  Centre: {c}"),
            None => println!("  Centre: any plant of this crop"),
        }
        if order_matters(step) {
            println!("  Neighbours, planting order is REQUIRED:");
            for (i, plant) in step.order.iter().enumerate() {
                println!("    {}. {plant}", i + 1);
            }
        } else {
            let mut copies: BTreeMap<Genotype, usize> = BTreeMap::new();
            for plant in &step.order {
                *copies.entry(*plant).or_default() += 1;
            }
            let list: Vec<String> = copies.iter().map(|(g, n)| if *n > 1 { format!("{g} x{n}") } else { g.to_string() }).collect();
            println!("  Neighbours, any order: {}", list.join("  "));
        }
    }
}

const CHECKLIST: &str = "
Before planting each step:
  - Empty the planter. Every living plant of this crop within 1.5 m counts as a neighbour.
  - Plant the centre first, then neighbours when it is about 50% through Sapling.
  - Finish planting before the centre reaches Crossbreed.
  - Avoid server restarts until the centre has crossbred. A restart can change planting order,
    and steps marked REQUIRED depend on it.";

fn main() -> Result<()> {
    let args = Args::parse();
    let text = match (&args.genes, &args.file) {
        (Some(text), None) => text.clone(),
        (None, Some(path)) => std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?,
        _ => bail!("give exactly one of --genes or --file"),
    };
    let inputs = parse_list(&text)?;
    if inputs.is_empty() {
        bail!("no plants given");
    }
    if !(1..=8).contains(&args.plants) {
        bail!("--plants must be 1 to 8");
    }
    let target = args.target.as_deref().map(Genotype::parse).transpose()?;
    let scores = Scores::default();
    let cfg = PlanConfig {
        generations: args.generations.max(1),
        carry: args.carry,
        layer: LayerConfig { max_plants: args.plants, min_score: (args.min_score * 100.0).round() as i32, scores },
        target,
    };

    let mut backend: Box<dyn Backend> = if args.cpu { Box::new(CpuBackend) } else { Box::new(GpuBackend::new()?) };
    println!("Using {}", backend.name());
    let started = Instant::now();
    let outcome = run(&inputs, &cfg, backend.as_mut(), |s| {
        println!(
            "Generation {}: {} plants, {:.2e} groups, {} new genotypes, {:.2}s ({:.2e} groups/s)",
            s.generation, s.pool, s.groups as f64, s.new_genotypes, s.seconds, s.groups as f64 / s.seconds.max(1e-9)
        );
    })?;
    println!("Finished in {:.2}s, {} genotypes reached", started.elapsed().as_secs_f64(), outcome.recipes.len());

    let plans = match target {
        Some(t) if t.0 < GENOTYPES as u16 && outcome.inputs.contains(&t) => {
            println!("{t} is already one of your plants.");
            Vec::new()
        }
        Some(t) => match outcome.plan(t) {
            Some(plan) => vec![plan],
            None => {
                println!("{t} was not reached. Try more --generations, --carry or --plants.");
                Vec::new()
            }
        },
        None => outcome.best(&scores, args.top),
    };
    for plan in &plans {
        plan.check(&outcome.inputs)?;
        print_plan(plan, &scores);
    }
    if !plans.is_empty() {
        println!("{CHECKLIST}");
    }
    Ok(())
}
