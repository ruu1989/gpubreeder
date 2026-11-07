// Per-slot and per-gene index loops keep the CPU code shaped like the GPU kernel.
#![allow(clippy::needless_range_loop)]

pub mod benchmark;
pub mod genes;
pub mod gpu;
pub mod guide;
pub mod plan;
pub mod rules;
pub mod search;
pub mod selftest;
pub mod timing;
