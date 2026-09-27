//! Sustained load: runs per second, latency percentiles, CPU time per run,
//! allocations per run and peak memory, on one or more threads.
//!
//! ```text
//! cargo run --release --features json --example throughput -- <scenario> [seconds] [threads]
//! ```
//!
//! Scenarios: eligibility-1, eligibility-10, eligibility-100, eligibility-1000,
//! lifecycle, ask, compile, load.

#[path = "../benches/fixtures/mod.rs"]
mod fixtures;

use fixtures::{example, order, quote_data, return_data, return_journey, source};
use flow_runner::{Decision, Flow, Input, Limits, Value};
use serde_json::json;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Counts allocations while `COUNTING` is set.
struct Counting;
static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Relaxed) {
            ALLOCS.fetch_add(1, Relaxed);
            BYTES.fetch_add(layout.size() as u64, Relaxed);
        }
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if COUNTING.load(Relaxed) {
            ALLOCS.fetch_add(1, Relaxed);
            BYTES.fetch_add(new_size as u64, Relaxed);
        }
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// One unit of work, as a closure a thread can call repeatedly.
fn scenario(name: &str) -> Box<dyn Fn() + Send + Sync> {
    let limits = Limits::default();
    match name {
        s if s.starts_with("eligibility-") => {
            let lines: usize = s["eligibility-".len()..]
                .parse()
                .expect("eligibility-<lines>");
            let flow = example("return-eligibility");
            let data = order(lines);
            Box::new(move || {
                let out = flow
                    .run(Input::start("evaluate", data.clone()), &limits)
                    .unwrap();
                assert!(matches!(out, Decision::Value { .. }));
            })
        }
        "lifecycle" => {
            let flow = example("return-lifecycle");
            let data = return_data();
            Box::new(move || {
                let out = return_journey(&flow, &data, &limits);
                assert!(matches!(out, Decision::Value { .. }));
            })
        }
        "ask" => {
            let flow = example("carrier-quote");
            let data = quote_data();
            Box::new(move || {
                let Decision::Ask { token, .. } = flow
                    .run(Input::start("quote", data.clone()), &limits)
                    .unwrap()
                else {
                    panic!()
                };
                let answer = Value::from(json!({ "serviceable": true, "fee": 80, "etaDays": 2 }));
                let out = flow
                    .run(Input::resume(token, data.clone()).result(answer), &limits)
                    .unwrap();
                assert!(matches!(out, Decision::Value { .. }));
            })
        }
        "compile" => {
            let src = source("return-lifecycle");
            Box::new(move || {
                Flow::compile(&src).unwrap();
            })
        }
        "load" => {
            let bytes = example("return-lifecycle").to_bytes();
            Box::new(move || {
                Flow::from_bytes(&bytes).unwrap();
            })
        }
        other => panic!("unknown scenario '{other}'"),
    }
}

#[cfg(unix)]
fn rusage() -> libc::rusage {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    usage
}

/// User plus system CPU time of this process.
#[cfg(unix)]
fn cpu_seconds() -> f64 {
    let usage = rusage();
    let seconds = |time: libc::timeval| time.tv_sec as f64 + time.tv_usec as f64 / 1e6;
    seconds(usage.ru_utime) + seconds(usage.ru_stime)
}

#[cfg(unix)]
fn peak_rss_mib() -> f64 {
    let rss = rusage().ru_maxrss as f64;
    let bytes = if cfg!(target_os = "macos") {
        rss
    } else {
        rss * 1024.0
    };
    bytes / (1024.0 * 1024.0)
}

#[cfg(not(unix))]
fn cpu_seconds() -> f64 {
    f64::NAN
}

#[cfg(not(unix))]
fn peak_rss_mib() -> f64 {
    f64::NAN
}

/// Latencies in 1%-wide log buckets: fixed memory however long the run.
struct Histogram {
    counts: Vec<u64>,
    total: u64,
    max: u64,
}

impl Histogram {
    /// Buckets per unit of `ln(nanoseconds)`.
    const SCALE: f64 = 100.0;
    /// Enough buckets for latencies up to e^24 ns, about 26 seconds.
    const BUCKETS: usize = 2400;

    fn new() -> Self {
        Histogram {
            counts: vec![0; Self::BUCKETS],
            total: 0,
            max: 0,
        }
    }

    fn record(&mut self, nanoseconds: u64) {
        let bucket = ((nanoseconds.max(1) as f64).ln() * Self::SCALE) as usize;
        let last = self.counts.len() - 1;
        self.counts[bucket.min(last)] += 1;
        self.total += 1;
        self.max = self.max.max(nanoseconds);
    }

    fn merge(&mut self, other: &Histogram) {
        for (count, other_count) in self.counts.iter_mut().zip(&other.counts) {
            *count += other_count;
        }
        self.total += other.total;
        self.max = self.max.max(other.max);
    }

    /// The `p` quantile in microseconds.
    fn quantile(&self, p: f64) -> f64 {
        let target = ((self.total as f64 * p).ceil() as u64).max(1);
        let mut seen = 0;
        for (bucket, count) in self.counts.iter().enumerate() {
            seen += count;
            if seen >= target {
                return ((bucket as f64 + 0.5) / Self::SCALE).exp() / 1000.0;
            }
        }
        self.max as f64 / 1000.0
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let name = args.first().map_or("eligibility-10", String::as_str);
    let seconds: f64 = args
        .get(1)
        .map_or(3.0, |s| s.parse().expect("[seconds] is a number"));
    let threads: usize = args
        .get(2)
        .map_or(1, |s| s.parse().expect("[threads] is a whole number"));
    let work: Arc<dyn Fn() + Send + Sync> = Arc::from(scenario(name));

    // allocations per run, measured alone so counting doesn't slow the timed part
    for _ in 0..100 {
        work();
    }
    COUNTING.store(true, Relaxed);
    let probe = 1000;
    for _ in 0..probe {
        work();
    }
    COUNTING.store(false, Relaxed);
    let allocs = ALLOCS.load(Relaxed) as f64 / probe as f64;
    let bytes = BYTES.load(Relaxed) as f64 / probe as f64;

    let cpu_start = cpu_seconds();
    let wall_start = Instant::now();
    let deadline = Duration::from_secs_f64(seconds);
    let handles: Vec<_> = (0..threads)
        .map(|_| {
            let work = Arc::clone(&work);
            std::thread::spawn(move || {
                let mut histogram = Histogram::new();
                let start = Instant::now();
                while start.elapsed() < deadline {
                    let run_start = Instant::now();
                    work();
                    histogram.record(run_start.elapsed().as_nanos() as u64);
                }
                histogram
            })
        })
        .collect();
    let mut latency = Histogram::new();
    for handle in handles {
        latency.merge(&handle.join().unwrap());
    }
    let wall = wall_start.elapsed().as_secs_f64();
    let cpu = cpu_seconds() - cpu_start;
    let runs = latency.total as f64;
    let plural = if threads == 1 { "" } else { "s" };

    println!("scenario      {name}  ({threads} thread{plural})");
    println!(
        "runs          {runs:.0} in {wall:.2}s = {:.0} runs/s",
        runs / wall
    );
    println!(
        "latency µs    p50 {:.2}  p90 {:.2}  p99 {:.2}  p99.9 {:.2}  max {:.2}",
        latency.quantile(0.50),
        latency.quantile(0.90),
        latency.quantile(0.99),
        latency.quantile(0.999),
        latency.max as f64 / 1000.0
    );
    println!(
        "cpu           {:.2} µs per run, {:.0}% of {threads} core{plural}",
        cpu / runs * 1e6,
        cpu / wall * 100.0 / threads as f64
    );
    println!(
        "allocations   {allocs:.1} per run, {:.1} KiB per run",
        bytes / 1024.0
    );
    println!("peak memory   {:.1} MiB RSS", peak_rss_mib());
}
