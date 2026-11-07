//! GPU readings for the side panel, from NVIDIA's NVML. It is read-only and whole-GPU: it needs
//! no driver of its own and asks nothing about other processes.

use crate::Shared;
use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
use serde::Serialize;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

const INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone, Default, Serialize)]
struct Reading {
    name: String,
    utilization: Option<u32>,
    memory_used_gb: Option<f64>,
    memory_total_gb: Option<f64>,
    temperature: Option<u32>,
    power_watts: Option<f64>,
    power_limit_watts: Option<f64>,
    core_mhz: Option<u32>,
    memory_mhz: Option<u32>,
    fan_percent: Option<u32>,
    groups_per_second: f64,
    searching: bool,
}

pub fn start(app: AppHandle, shared: Arc<Shared>) {
    std::thread::spawn(move || {
        let nvml = Nvml::init().ok();
        let device = nvml.as_ref().and_then(|n| n.device_by_index(0).ok());
        let name = device.as_ref().and_then(|d| d.name().ok()).unwrap_or_else(|| "NVIDIA telemetry unavailable".into());
        let mut last_count = 0u64;
        let mut last_time = Instant::now();
        loop {
            std::thread::sleep(INTERVAL);
            let mut reading = Reading { name: name.clone(), ..Default::default() };
            if let Some(d) = &device {
                reading.utilization = d.utilization_rates().ok().map(|u| u.gpu);
                if let Ok(memory) = d.memory_info() {
                    reading.memory_used_gb = Some(memory.used as f64 / 1e9);
                    reading.memory_total_gb = Some(memory.total as f64 / 1e9);
                }
                reading.temperature = d.temperature(TemperatureSensor::Gpu).ok();
                reading.power_watts = d.power_usage().ok().map(|mw| mw as f64 / 1000.0);
                reading.power_limit_watts = d.enforced_power_limit().ok().map(|mw| mw as f64 / 1000.0);
                reading.core_mhz = d.clock_info(Clock::Graphics).ok();
                reading.memory_mhz = d.clock_info(Clock::Memory).ok();
                reading.fan_percent = d.fan_speed(0).ok();
            }
            let count = shared.control.lock().ok().and_then(|c| c.as_ref().map(|c| c.groups.load(Ordering::Relaxed))).unwrap_or(0);
            let elapsed = last_time.elapsed().as_secs_f64();
            reading.groups_per_second = count.saturating_sub(last_count) as f64 / elapsed;
            last_count = count;
            last_time = Instant::now();
            reading.searching = shared.searching.load(Ordering::Relaxed);
            let _ = app.emit("telemetry", reading);
        }
    });
}
