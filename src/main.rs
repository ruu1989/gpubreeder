use anyhow::{Context, Result, bail};
use clap::Parser;
use gpubreeder::genes::*;
use gpubreeder::gpu::{ADAPTER_VARIABLE, GpuBackend, list_adapters};
use gpubreeder::plan::*;
use gpubreeder::search::*;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

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
    #[arg(short, long, conflicts_with = "mix")]
    target: Option<String>,
    /// Stop at the cheapest plant with these genes in any slots, e.g. 3G3Y
    #[arg(short, long)]
    mix: Option<String>,
    /// Most crossings to spend on one plant
    #[arg(long, default_value_t = 12)]
    max_crossings: u32,
    /// Most neighbours around a centre, 1 to 8
    #[arg(long, default_value_t = 5)]
    plants: usize,
    /// Stop searching after this many seconds
    #[arg(long, default_value_t = 60.0)]
    seconds: f64,
    /// Number of plans to print, unless --target is used
    #[arg(long, default_value_t = 3)]
    top: usize,
    /// Use the CPU instead of the GPU
    #[arg(long)]
    cpu: bool,
    /// Check your GPU's results against the CPU, then print a report to paste into a bug report
    #[arg(long, exclusive = true)]
    selftest: bool,
    /// Measure your GPU's top speed in neighbour groups per second, for about 10 seconds
    #[arg(long, exclusive = true)]
    benchmark: bool,
}

fn print_plan(plan: &Plan, scores: &Scores) {
    println!();
    println!(
        "=== {} | score {:.1} | generation {} | {} crossing(s) ===",
        plan.goal,
        plan.goal.score(scores) as f32 / 100.0,
        plan.generation,
        plan.steps.len()
    );
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
    The desktop app works out the exact window for each crossing.
  - Finish planting before the centre reaches Crossbreed.
  - Avoid server restarts until the centre has crossbred. A restart can change planting order,
    and steps marked REQUIRED depend on it.";

/// Runs every self-test case on the GPU and the CPU and compares them.
fn selftest() -> Result<()> {
    println!("GPUBreeder {} self-test, {} {}", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH);
    let cards = list_adapters();
    println!("Cards found: {}", if cards.is_empty() { "none".to_string() } else { cards.join("; ") });
    let mut gpu = GpuBackend::new()?;
    let i = &gpu.info;
    println!("Testing: {} | {:?} | {:?} | driver {} {}", i.name, i.backend, i.device_type, i.driver, i.driver_info);
    println!("Choose another card with {ADAPTER_VARIABLE}=<part of its name>.");
    let started = Instant::now();
    let cases = gpubreeder::selftest::cases();
    let mut failed = 0;
    for case in &cases {
        let case_start = Instant::now();
        let verdict = match gpubreeder::selftest::compare(&mut gpu, case) {
            Ok(None) => "ok".to_string(),
            Ok(Some(difference)) => format!("DIFFERENT: the GPU {difference}"),
            Err(e) => format!("ERROR: {e:#}"),
        };
        if verdict != "ok" {
            failed += 1;
        }
        println!("  {:<60} {:>6.2}s  {verdict}", case.name, case_start.elapsed().as_secs_f64());
    }
    println!("{} of {} cases match, in {:.1}s.", cases.len() - failed, cases.len(), started.elapsed().as_secs_f64());
    if failed > 0 {
        println!("FAIL: don't trust GPU plans on this card. --cpu still works. Please report this output.");
        std::process::exit(1);
    }
    println!("PASS: this card agrees with the CPU.");
    Ok(())
}

/// Prints the GPU's speed on the fixed benchmark workload.
fn benchmark() -> Result<()> {
    use gpubreeder::benchmark;
    let mut gpu = GpuBackend::new()?;
    let i = &gpu.info;
    println!("GPUBreeder {} benchmark on {} | {:?} | driver {} {}", env!("CARGO_PKG_VERSION"), i.name, i.backend, i.driver, i.driver_info);
    println!(
        "One crossing round over {} fixed clones with up to {} neighbours, repeated for up to 10 seconds.",
        benchmark::CLONES,
        benchmark::NEIGHBOURS
    );
    println!("Close games and other GPU work first for a fair number.");
    let mut n = 0;
    let report = benchmark::run(&mut gpu, Duration::from_secs(10), |speed| {
        n += 1;
        println!("  run {n:>2}: {} groups/s", groups(speed));
    })?;
    println!(
        "Top speed: {} neighbour groups per second (median {}, {} runs in {:.1}s).",
        groups(report.best()),
        groups(report.median()),
        report.runs.len(),
        report.seconds
    );
    Ok(())
}

/// Writes a large count with a unit, such as 2.41 billion.
fn groups(value: f64) -> String {
    match value {
        v if v >= 1e9 => format!("{:.2} billion", v / 1e9),
        v if v >= 1e6 => format!("{:.1} million", v / 1e6),
        v => format!("{:.0} thousand", v / 1e3),
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.selftest {
        return selftest();
    }
    if args.benchmark {
        return benchmark();
    }
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
    let goal = match (&args.target, &args.mix) {
        (Some(text), _) => Some(Goal::Exact(Genotype::parse(text)?)),
        (_, Some(text)) => Some(Goal::Mix(GeneMix::parse(text)?)),
        _ => None,
    };
    if let Some(goal) = goal {
        if let Some(owned) = inputs.iter().find(|&&g| goal.matches(g)) {
            println!("You already have {owned}, which meets {goal}.");
            return Ok(());
        }
    }
    let scores = Scores::default();
    let cfg = PlanConfig {
        max_crossings: args.max_crossings,
        max_plants: args.plants,
        goal,
        time_limit: Some(Duration::from_secs_f64(args.seconds)),
    };

    let mut backend: Box<dyn Backend> = if args.cpu { Box::new(CpuBackend) } else { Box::new(GpuBackend::new()?) };
    println!("Using {}", backend.name());
    let started = Instant::now();
    let outcome = run(&inputs, &cfg, backend.as_mut(), |s| {
        let note = if s.complete { "" } else { " (stopped by time limit)" };
        println!("{} crossing(s): {} new genotypes, {} known, {:.2}s{note}", s.crossings, s.new_genotypes, s.pool, s.seconds);
    })?;
    println!(
        "Finished in {:.2}s. Every plant at up to {} crossing(s) has been searched exhaustively.",
        started.elapsed().as_secs_f64(),
        outcome.proven_up_to()
    );

    let plans = match goal {
        Some(goal) => {
            let plans = outcome.meeting(goal, args.top);
            if plans.is_empty() {
                println!("{goal} was not reached. Try more --seconds or --plants.");
            }
            plans
        }
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
