//! Desktop front end for the GPUBreeder library.
//!
//! This app never touches the game. It opens no handle to any game process, injects nothing,
//! draws no overlay and reads no game memory. Plants arrive as pasted text, and GPU telemetry
//! comes from NVIDIA's own read-only NVML library.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod telemetry;

use anyhow::{Context, Result, anyhow};
use gpubreeder::genes::*;
use gpubreeder::gpu::{GpuBackend, GpuFailure};
use gpubreeder::guide::{self, Guide, Place, Places};
use gpubreeder::plan::*;
use gpubreeder::search::*;
use gpubreeder::timing::*;
use serde::{Deserialize, Serialize};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Shortcut, ShortcutState};

/// Plans listed after a search. Arrangements of the same mix often tie.
const PLANS_SHOWN: usize = 12;

/// The last search's plans, the starting plants they used, and where each plant is stored.
type Results = (Vec<Plan>, Vec<Genotype>, Places);

#[derive(Default)]
struct Shared {
    backend: Mutex<Option<GpuBackend>>,
    results: Mutex<Option<Results>>,
    control: Mutex<Option<Arc<SearchControl>>>,
    searching: AtomicBool,
    alarm: AtomicU64,
}

impl Shared {
    /// A panic mid-search leaves this lock poisoned, and the backend inside is then replaced.
    fn backend(&self) -> MutexGuard<'_, Option<GpuBackend>> {
        self.backend.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Lets go of a dead GPU device so the next search starts a new one. It is leaked on purpose,
    /// because cleaning up a device can wait on work that will never finish.
    fn discard_backend(&self) {
        std::mem::forget(self.backend().take());
    }
}

/// Errors cross to the page as plain text.
fn text(error: anyhow::Error) -> String {
    format!("{error:#}")
}

#[derive(Serialize)]
struct PastedPlant {
    genes: String,
    g: u32,
    to_crossbreed_minutes: u32,
    places: Vec<Place>,
}

#[derive(Serialize)]
struct PastedReply {
    plants: Vec<PastedPlant>,
    ignored: Vec<String>,
    /// Where the last plant should be, which shows at a glance if numbers have drifted.
    last: Option<Place>,
}

#[tauri::command]
fn crops() -> Vec<Crop> {
    CROPS.to_vec()
}

#[tauri::command]
fn read_plants(pasted: String, crop: String, fertilized: bool, quality: f32) -> Result<PastedReply, String> {
    let crop = crop_by_key(&crop).map_err(text)?;
    let parsed = parse_pasted(&pasted);
    let places = guide::places_of(&parsed.scanned);
    let last = parsed.scanned.iter().rposition(Option::is_some).map(|at| Place::of(at as u32 + 1));
    let mut plants = Vec::with_capacity(parsed.plants.len());
    for plant in parsed.plants {
        let g = plant.genes().iter().filter(|&&gene| gene == G).count() as u32;
        let to_crossbreed_minutes = Timeline::new(crop, g, conditions(fertilized, quality)).map_err(text)?.to_crossbreed();
        plants.push(PastedPlant {
            genes: plant.to_string(),
            g,
            to_crossbreed_minutes,
            places: places.get(&plant).cloned().unwrap_or_default(),
        });
    }
    Ok(PastedReply { plants, ignored: parsed.ignored, last })
}

#[derive(Deserialize)]
struct SearchRequest {
    plants: String,
    /// A mix like 3G3Y. Empty means the highest score reachable in the time allowed.
    mix: String,
    neighbours: usize,
    seconds: f64,
    crop: String,
    fertilized: bool,
    quality: f32,
}

#[derive(Clone, Serialize)]
struct StepView {
    result: String,
    centre: Option<String>,
    order: Vec<String>,
    order_required: bool,
}

#[derive(Clone, Serialize)]
struct PlanView {
    index: usize,
    goal: String,
    crossings: usize,
    generation: u32,
    steps: Vec<StepView>,
    guide: Result<Guide, String>,
}

#[derive(Clone, Serialize)]
struct RoundView {
    crossings: u32,
    new_genotypes: usize,
    known: usize,
    seconds: f64,
    complete: bool,
}

#[derive(Clone, Serialize)]
struct SearchReply {
    plans: Vec<PlanView>,
    proven_up_to: u32,
    seconds: f64,
    reached_goal: bool,
    already_have: Option<String>,
    device: String,
    stopped: bool,
}

fn conditions(fertilized: bool, quality: f32) -> Conditions {
    Conditions { fertilized, other_quality: quality.clamp(0.0, 1.0) }
}

fn plan_view(index: usize, plan: &Plan, inputs: &[Genotype], places: &Places, crop: &Crop, conditions: Conditions) -> PlanView {
    let steps = plan.steps.iter().map(|s| StepView {
        result: s.result.to_string(),
        centre: s.centre.map(|c| c.to_string()),
        order: s.order.iter().map(Genotype::to_string).collect(),
        order_required: order_matters(s),
    });
    PlanView {
        index,
        goal: plan.goal.to_string(),
        crossings: plan.steps.len(),
        generation: plan.generation,
        steps: steps.collect(),
        guide: guide::build(plan, inputs, places, crop, conditions).map_err(text),
    }
}

fn run_search(app: &AppHandle, shared: &Shared, request: SearchRequest) -> Result<SearchReply> {
    let pasted = parse_pasted(&request.plants);
    let places = guide::places_of(&pasted.scanned);
    let inputs = pasted.plants;
    if inputs.is_empty() {
        return Err(anyhow!("Paste at least one plant first."));
    }
    let goal = match request.mix.trim() {
        "" => None,
        mix => Some(Goal::Mix(GeneMix::parse(mix)?)),
    };
    let crop = crop_by_key(&request.crop)?;
    let growing = conditions(request.fertilized, request.quality);
    let already_have = goal.and_then(|goal| inputs.iter().find(|&&g| goal.matches(g))).map(Genotype::to_string);
    if already_have.is_some() {
        return Ok(SearchReply {
            plans: Vec::new(),
            proven_up_to: 0,
            seconds: 0.0,
            reached_goal: true,
            already_have,
            device: String::new(),
            stopped: false,
        });
    }

    let mut slot = shared.backend();
    if slot.is_none() {
        *slot = Some(GpuBackend::new().context("starting the GPU")?);
    }
    let backend = slot.as_mut().context("the GPU is not ready")?;
    let control = backend.control();
    control.cancel.store(false, Ordering::Relaxed);
    control.groups.store(0, Ordering::Relaxed);
    *shared.control.lock().map_err(|_| anyhow!("search state was poisoned"))? = Some(control.clone());

    let device = backend.name();
    let cfg = PlanConfig {
        max_crossings: 12,
        max_plants: request.neighbours.clamp(1, 8),
        goal,
        time_limit: Some(Duration::from_secs_f64(request.seconds)),
    };
    let started = Instant::now();
    let _ = app.emit("search-started", request.seconds);
    let outcome = run(&inputs, &cfg, backend, |s| {
        let round =
            RoundView { crossings: s.crossings, new_genotypes: s.new_genotypes, known: s.pool, seconds: s.seconds, complete: s.complete };
        let _ = app.emit("search-round", round);
    })?;

    let plans = match goal {
        Some(goal) => outcome.meeting(goal, PLANS_SHOWN),
        None => outcome.best(&Scores::default(), PLANS_SHOWN),
    };
    for plan in &plans {
        plan.check(&outcome.inputs)?;
    }
    let views = plans.iter().enumerate().map(|(i, p)| plan_view(i, p, &outcome.inputs, &places, crop, growing)).collect();
    let reply = SearchReply {
        reached_goal: !plans.is_empty(),
        plans: views,
        proven_up_to: outcome.proven_up_to(),
        seconds: started.elapsed().as_secs_f64(),
        already_have: None,
        device,
        stopped: control.cancel.load(Ordering::Relaxed),
    };
    *shared.results.lock().map_err(|_| anyhow!("search state was poisoned"))? = Some((plans, outcome.inputs, places));
    Ok(reply)
}

#[derive(Clone, Serialize)]
struct BenchmarkReply {
    device: String,
    best: f64,
    median: f64,
    runs: usize,
    seconds: f64,
}

fn run_benchmark(shared: &Shared) -> Result<BenchmarkReply> {
    let mut slot = shared.backend();
    if slot.is_none() {
        *slot = Some(GpuBackend::new().context("starting the GPU")?);
    }
    let backend = slot.as_mut().context("the GPU is not ready")?;
    let control = backend.control();
    control.cancel.store(false, Ordering::Relaxed);
    control.groups.store(0, Ordering::Relaxed);
    *shared.control.lock().map_err(|_| anyhow!("search state was poisoned"))? = Some(control);
    let report = gpubreeder::benchmark::run(backend, Duration::from_secs(10), |_| {})?;
    Ok(BenchmarkReply {
        device: backend.name(),
        best: report.best(),
        median: report.median(),
        runs: report.runs.len(),
        seconds: report.seconds,
    })
}

/// Measures the card's top speed on a fixed workload. It uses the search's GPU, so it can't run
/// beside a search.
#[tauri::command]
fn benchmark(app: AppHandle) -> Result<(), String> {
    let shared = app.state::<Arc<Shared>>().inner().clone();
    if shared.searching.swap(true, Ordering::SeqCst) {
        return Err("Wait for the search to finish first.".into());
    }
    std::thread::spawn(move || {
        let reply = catch_unwind(AssertUnwindSafe(|| run_benchmark(&shared)))
            .unwrap_or_else(|_| Err(GpuFailure::new("the GPU code panicked").into()));
        if reply.as_ref().err().is_some_and(|error| error.is::<GpuFailure>()) {
            shared.discard_backend();
        }
        shared.searching.store(false, Ordering::SeqCst);
        match reply {
            Ok(reply) => app.emit("benchmark-done", reply),
            Err(error) => app.emit("benchmark-failed", text(error)),
        }
    });
    Ok(())
}

#[tauri::command]
fn start_search(app: AppHandle, request: SearchRequest) -> Result<(), String> {
    let shared = app.state::<Arc<Shared>>().inner().clone();
    if shared.searching.swap(true, Ordering::SeqCst) {
        return Err("A search is already running.".into());
    }
    std::thread::spawn(move || {
        // wgpu panics on some device errors, and a thread that dies silently would leave the page
        // saying "searching" forever.
        let reply = catch_unwind(AssertUnwindSafe(|| run_search(&app, &shared, request)))
            .unwrap_or_else(|_| Err(GpuFailure::new("the GPU code panicked").into()));
        if reply.as_ref().err().is_some_and(|error| error.is::<GpuFailure>()) {
            shared.discard_backend();
        }
        shared.searching.store(false, Ordering::SeqCst);
        match reply {
            Ok(reply) => app.emit("search-done", reply),
            Err(error) => app.emit("search-failed", text(error)),
        }
    });
    Ok(())
}

#[tauri::command]
fn stop_search(shared: State<Arc<Shared>>) {
    if let Ok(control) = shared.control.lock() {
        if let Some(control) = control.as_ref() {
            control.cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// Rebuilds the plan list after the crop or growing conditions change.
#[tauri::command]
fn replan(shared: State<Arc<Shared>>, crop: String, fertilized: bool, quality: f32) -> Result<Vec<PlanView>, String> {
    let crop = crop_by_key(&crop).map_err(text)?;
    let results = shared.results.lock().map_err(|_| "search state was poisoned")?;
    let Some((plans, inputs, places)) = results.as_ref() else {
        return Ok(Vec::new());
    };
    let growing = conditions(fertilized, quality);
    Ok(plans.iter().enumerate().map(|(i, p)| plan_view(i, p, inputs, places, crop, growing)).collect())
}

#[derive(Serialize)]
struct GrowRow {
    g: u32,
    to_sapling: u32,
    to_crossbreed: u32,
    to_mature: u32,
    to_ripe: u32,
}

#[derive(Serialize)]
struct GrowTable {
    crop: Crop,
    quality: f32,
    rows: Vec<GrowRow>,
}

#[tauri::command]
fn grow_table(fertilized: bool, quality: f32) -> Result<Vec<GrowTable>, String> {
    let growing = conditions(fertilized, quality);
    let table = |crop: &Crop| -> Result<GrowTable> {
        let rows = (0..=SLOTS as u32).map(|g| {
            let t = Timeline::new(crop, g, growing)?;
            Ok(GrowRow { g, to_sapling: t.to_sapling(), to_crossbreed: t.to_crossbreed(), to_mature: t.to_mature(), to_ripe: t.to_ripe() })
        });
        Ok(GrowTable { crop: *crop, quality: growing.quality(crop), rows: rows.collect::<Result<_>>()? })
    };
    CROPS.iter().map(table).collect::<Result<_>>().map_err(text)
}

/// Timers live here so they still fire while the window is behind the game.
#[tauri::command]
fn arm_alarm(app: AppHandle, seconds: f64) {
    let shared = app.state::<Arc<Shared>>().inner().clone();
    let ticket = shared.alarm.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs_f64(seconds.max(0.0)));
        if shared.alarm.load(Ordering::SeqCst) == ticket {
            let _ = app.emit("alarm", ());
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.request_user_attention(Some(tauri::UserAttentionType::Informational));
            }
        }
    });
}

#[tauri::command]
fn disarm_alarm(shared: State<Arc<Shared>>) {
    shared.alarm.fetch_add(1, Ordering::SeqCst);
}

#[tauri::command]
fn set_always_on_top(app: AppHandle, on: bool) -> Result<(), String> {
    let window = app.get_webview_window("main").ok_or("no window")?;
    window.set_always_on_top(on).map_err(|e| e.to_string())
}

/// F9 is claimed only while a guide is running. This is the standard Windows hotkey call,
/// not a keyboard hook, so nothing watches the game's input.
#[tauri::command]
fn set_hotkey(app: AppHandle, on: bool) -> Result<(), String> {
    let shortcuts = app.global_shortcut();
    let key = Shortcut::new(None, Code::F9);
    let held = shortcuts.is_registered(key);
    match (on, held) {
        (true, false) => shortcuts.register(key).map_err(|e| e.to_string()),
        (false, true) => shortcuts.unregister(key).map_err(|e| e.to_string()),
        _ => Ok(()),
    }
}

#[tauri::command]
fn open_rust_breeder(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener().open_url("https://rustbreeder.com/", None::<&str>).map_err(|e| e.to_string())
}

fn main() {
    let shared = Arc::new(Shared::default());
    let hotkeys = tauri_plugin_global_shortcut::Builder::new()
        .with_handler(|app, _, event| {
            if event.state() == ShortcutState::Pressed {
                let _ = app.emit("hotkey-next", ());
            }
        })
        .build();
    tauri::Builder::default()
        .plugin(hotkeys)
        .plugin(tauri_plugin_opener::init())
        .manage(shared.clone())
        .invoke_handler(tauri::generate_handler![
            crops,
            read_plants,
            start_search,
            stop_search,
            replan,
            grow_table,
            arm_alarm,
            disarm_alarm,
            set_always_on_top,
            set_hotkey,
            open_rust_breeder,
            benchmark
        ])
        .setup(move |app| {
            telemetry::start(app.handle().clone(), shared.clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("the app failed to start");
}
