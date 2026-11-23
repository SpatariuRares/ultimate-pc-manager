//! The GPU test: Direct3D 11 on the video card the user chose (DXGI lists
//! them). The compute part runs our own `cs_5_0` shader, eight independent
//! multiply-add chains per thread, and reports Compute in GFLOPS; the graphics part then
//! draws full-target triangles with a vertex + pixel shader pair whose pixel
//! shader runs a 64-step loop, and reports Graphics in Gpixels/sec. Both are
//! run in batches timed with GPU timestamp queries.

use std::time::Instant;

use super::engine::{Cancel, Measure, Probe, Unit};

/// Billion floating-point operations per second (GPU compute).
pub const GFLOPS: Measure = Measure { label: "GFLOPS", decimals: 0 };
/// Billion shaded pixels per second (GPU graphics).
pub const GPIXELS: Measure = Measure { label: "Gpixels/sec", decimals: 2 };
/// The compute figure.
pub const COMPUTE: &str = "Compute";
/// The graphics figure.
pub const GRAPHICS: &str = "Graphics";
/// The compute phase.
pub const TESTING_COMPUTE: &str = "Testing GPU compute";
/// The graphics phase.
pub const TESTING_GRAPHICS: &str = "Testing GPU graphics";

/// The GPU test's error messages.
pub const NO_GPU: &str = "A hardware Direct3D 11 GPU is unavailable on this system.";
pub const SHADERS_NOT_COMPILED: &str = "The Direct3D benchmark shaders could not be compiled.";
pub const PIPELINES_NOT_CREATED: &str = "The Direct3D benchmark pipelines could not be created.";
pub const RESOURCES_NOT_ALLOCATED: &str = "The Direct3D benchmark resources could not be allocated.";
pub const SYNC_UNAVAILABLE: &str = "Direct3D GPU synchronization is unavailable.";
pub const COMPUTE_FAILED: &str = "Direct3D compute work failed.";
/// Failed drawing: the compute message, said of graphics.
pub const GRAPHICS_FAILED: &str = "Direct3D graphics work failed.";

/// The compute shader's entry point.
pub const COMPUTE_ENTRY: &str = "MeasureFma";
/// Threads in one compute thread group.
pub const GROUP_THREADS: u32 = 128;
/// Thread groups in one dispatch.
pub const DISPATCH_GROUPS: u32 = 8192;
/// Threads in one dispatch: 1,048,576.
pub const DISPATCH_THREADS: u64 = DISPATCH_GROUPS as u64 * GROUP_THREADS as u64;
/// Independent multiply-add chains per thread (two float4 registers).
pub const CHAINS: u32 = 8;
/// Multiply-adds in each chain.
pub const STEPS: u32 = 160;

/// The compute shader (this app's own): each thread runs eight independent
/// multiply-add chains, as two float4 registers, for `STEPS` unrolled steps,
/// so the GPU can overlap them and the arithmetic units stay busy. The
/// multiplier comes from the thread id, so the compiler cannot fold the
/// chains; their sum is written out so it cannot drop them either.
pub fn compute_shader() -> String {
    format!(
        r#"RWStructuredBuffer<float> results : register(u0);

[numthreads({GROUP_THREADS}, 1, 1)]
void {COMPUTE_ENTRY}(uint3 thread : SV_DispatchThreadID) {{
  // Below 1, so every chain settles instead of growing without bound.
  float scale = 0.998f + float(thread.x % 251u) * 0.0000075f;
  float4 left = scale;
  float4 right = scale;
  [unroll] for (uint step = 0; step < {STEPS}; ++step) {{
    left = mad(left, scale, float4(0.0031f, 0.0047f, 0.0059f, 0.0071f));
    right = mad(right, scale, float4(0.0083f, 0.0097f, 0.0109f, 0.0127f));
  }}
  float4 total = left + right;
  results[thread.x] = (total.x + total.y) + (total.z + total.w);
}}
"#
    )
}

/// The vertex shader's entry point.
pub const VERTEX_ENTRY: &str = "CoverTarget";
/// The pixel shader's entry point.
pub const PIXEL_ENTRY: &str = "ShadePixel";

/// The graphics shaders (this app's own): the vertex shader makes one
/// triangle that covers the whole target from the vertex index alone (no
/// vertex buffer); the pixel shader runs 64 steps of colour arithmetic.
pub const GRAPHICS_SHADERS: &str = r#"struct Interpolants {
  float4 position : SV_Position;
  float2 uv : TEXCOORD0;
};

Interpolants CoverTarget(uint id : SV_VertexID) {
  // Vertices (0,0), (2,0), (0,2) in uv: a triangle twice the target's size.
  float2 uv = float2(float(id == 1u) * 2.0f, float(id == 2u) * 2.0f);
  Interpolants o;
  o.position = float4(uv.x * 2.0f - 1.0f, 1.0f - uv.y * 2.0f, 0.0f, 1.0f);
  o.uv = uv;
  return o;
}

float4 ShadePixel(Interpolants i) : SV_Target {
  float3 tint = float3(i.uv, 0.5f);
  [unroll] for (uint step = 0; step < 64; ++step) {
    tint = mad(tint, float3(0.9993f, 0.9995f, 0.9997f), tint.zxy * 0.0004f);
  }
  return float4(tint, 1.0f);
}
"#;

/// Floating-point operations counted for one shader thread: its chains'
/// multiply-adds, two FLOPs each (a multiply and an add). The setup and the
/// final sum (under 0.5% more) are not counted.
pub const FLOPS_PER_THREAD: f64 = CHAINS as f64 * STEPS as f64 * 2.0;

/// How long each part (compute, graphics) measures, in GPU time.
pub const PART_SECONDS: f64 = 2.0;
/// The GPU time each batch aims at (short enough to cancel quickly and to
/// stay far from the driver's timeout).
const BATCH_SECONDS: f64 = 0.05;
/// The most dispatches or draws in one batch.
const MAX_BATCH: u32 = 10_000;
/// A part stops measuring after this many times its GPU time in wall time.
const WALL_FACTOR: f64 = 5.0;

/// A video card DXGI lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adapter {
    /// "NVIDIA GeForce RTX 4080".
    pub name: String,
    /// Its locally unique id, which tells two equal cards apart.
    pub luid: i64,
}

/// The card to test: the one chosen (by its LUID) if it is still listed,
/// else the first listed (the high-performance one), else none.
pub fn chosen(adapters: &[Adapter], choice: Option<i64>) -> Option<&Adapter> {
    adapters.iter().find(|a| Some(a.luid) == choice).or(adapters.first())
}

/// The compute shader ready to run on one card.
pub trait Compute {
    /// Shader threads in one dispatch.
    fn threads(&self) -> u64;
    /// Runs `dispatches` dispatches and waits for them: the GPU seconds
    /// they took, or none when the timing is not valid (disjoint).
    fn run(&mut self, dispatches: u32) -> Result<Option<f64>, String>;
}

/// The graphics shaders ready to draw on one card.
pub trait Graphics {
    /// Pixels shaded by one draw.
    fn pixels(&self) -> u64;
    /// Makes `draws` draws and waits for them: the GPU seconds they took,
    /// or none when the timing is not valid (disjoint).
    fn draw(&mut self, draws: u32) -> Result<Option<f64>, String>;
}

/// A card ready for both parts of the test.
pub trait Gpu: Compute + Graphics {}

impl<T: Compute + Graphics> Gpu for T {}

/// One timed part of the test.
struct Part {
    phase: &'static str,
    figure: &'static str,
    unit: Measure,
    failed: &'static str,
    /// Where its progress runs within the test's (from, to).
    span: (f32, f32),
}

/// Runs batches with `run` until they took `seconds` of GPU time (or the
/// run is cancelled), then reports `work` units (per batch item) per GPU
/// second, in billions. The first batch warms up and is not counted.
fn measure(
    probe: &Probe,
    part: &Part,
    seconds: f64,
    work: f64,
    mut run: impl FnMut(u32) -> Result<Option<f64>, String>,
) -> Result<(), String> {
    probe.phase(part.phase);
    let start = Instant::now();
    let (mut gpu_seconds, mut done) = (0.0, 0.0);
    let mut batch = 1u32;
    let mut warm = false;
    while gpu_seconds < seconds && start.elapsed().as_secs_f64() < seconds * WALL_FACTOR {
        if probe.cancelled() {
            return Ok(());
        }
        let took = run(batch)?;
        if let Some(t) = took.filter(|t| *t > 0.0) {
            if warm {
                gpu_seconds += t;
                done += work * batch as f64;
            }
            warm = true;
        }
        batch = next_batch(batch, took);
        let (from, to) = part.span;
        probe.progress(from + (to - from) * (gpu_seconds / seconds).min(1.0) as f32);
    }
    if probe.cancelled() {
        return Ok(());
    }
    if gpu_seconds <= 0.0 {
        return Err(part.failed.into());
    }
    probe.figure(part.figure, done / gpu_seconds / 1e9, part.unit);
    Ok(())
}

/// "Testing GPU compute": the first half of the test, Compute in GFLOPS.
pub fn compute<G: Compute + ?Sized>(probe: &Probe, gpu: &mut G, seconds: f64) -> Result<(), String> {
    let part = Part { phase: TESTING_COMPUTE, figure: COMPUTE, unit: GFLOPS, failed: COMPUTE_FAILED, span: (0.0, 0.5) };
    let work = gpu.threads() as f64 * FLOPS_PER_THREAD;
    measure(probe, &part, seconds, work, |n| gpu.run(n))
}

/// "Testing GPU graphics": the second half of the test, Graphics in
/// Gpixels/sec.
pub fn graphics<G: Graphics + ?Sized>(probe: &Probe, gpu: &mut G, seconds: f64) -> Result<(), String> {
    let part = Part { phase: TESTING_GRAPHICS, figure: GRAPHICS, unit: GPIXELS, failed: GRAPHICS_FAILED, span: (0.5, 1.0) };
    let work = gpu.pixels() as f64;
    measure(probe, &part, seconds, work, |n| gpu.draw(n))
}

/// The dispatches or draws of the next batch, after `batch` took `took`
/// GPU seconds: aimed at a batch of [`BATCH_SECONDS`], growing at most 4
/// times.
fn next_batch(batch: u32, took: Option<f64>) -> u32 {
    let grown = match took {
        Some(t) if t > 0.0 => (batch as f64 * BATCH_SECONDS / t).min(batch as f64 * 4.0),
        _ => batch as f64 * 2.0,
    };
    (grown as u32).clamp(1, MAX_BATCH)
}

/// The engine unit of the GPU test on `adapter`, opened with `open` (which
/// gets the run's cancel flag): compute, then graphics.
pub fn unit_with(
    adapter: Option<Adapter>,
    open: impl FnOnce(&Adapter, Cancel) -> Result<Box<dyn Gpu>, String> + Send + 'static,
) -> Unit {
    unit_with_seconds(adapter, PART_SECONDS, open)
}

/// [`unit_with`], measuring each part for `seconds`.
fn unit_with_seconds(
    adapter: Option<Adapter>,
    seconds: f64,
    open: impl FnOnce(&Adapter, Cancel) -> Result<Box<dyn Gpu>, String> + Send + 'static,
) -> Unit {
    Unit::new(NAME, move |probe| {
        let adapter = adapter.ok_or(NO_GPU)?;
        let mut gpu = open(&adapter, probe.cancel_flag())?;
        compute(probe, gpu.as_mut(), seconds)?;
        if probe.cancelled() {
            return Ok(());
        }
        graphics(probe, gpu.as_mut(), seconds)
    })
}

/// The test's name, as in "%1 test failed: %2".
pub const NAME: &str = "GPU";

/// The engine unit of the GPU test on `adapter`, with Direct3D 11.
pub fn unit(adapter: Option<Adapter>) -> Unit {
    unit_with(adapter, open)
}

/// The cards with their dedicated video memory, the high-performance one
/// (the most memory: a discrete card before an integrated one) first; cards
/// with equal memory keep DXGI's order.
fn high_performance_first(mut cards: Vec<(Adapter, u64)>) -> Vec<Adapter> {
    cards.sort_by_key(|(_, memory)| std::cmp::Reverse(*memory));
    cards.into_iter().map(|(card, _)| card).collect()
}

/// The hardware video cards, the high-performance one first (none without
/// DXGI).
pub fn adapters() -> Vec<Adapter> {
    system::adapters()
}

/// Both parts' shaders on `adapter`, with Direct3D 11.
fn open(adapter: &Adapter, cancel: Cancel) -> Result<Box<dyn Gpu>, String> {
    system::open(adapter, cancel)
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux as system;
#[cfg(target_os = "macos")]
use macos as system;
#[cfg(windows)]
use windows as system;


#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::benchmarks::engine::{wait_idle, Engine, EngineState};
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::time::Duration;

    fn card(name: &str, luid: i64) -> Adapter {
        Adapter { name: name.into(), luid }
    }

    /// A card where every dispatch of a million threads takes `per` GPU
    /// seconds; it fails with `error` at the `fail_at`-th batch.
    /// Every draw of a million pixels takes `per_draw` GPU seconds; drawing
    /// fails with `draw_fail` and, once it starts, says so on `drawing`
    /// then waits for `resume`.
    struct Fake {
        per: f64,
        batches: u32,
        fail_at: Option<(u32, &'static str)>,
        started: Option<Sender<()>>,
        per_draw: f64,
        draws: u32,
        draw_fail: Option<&'static str>,
        drawing: Option<Sender<()>>,
        resume: Option<Receiver<()>>,
    }

    impl Fake {
        fn new(per: f64) -> Fake {
            Fake {
                per,
                batches: 0,
                fail_at: None,
                started: None,
                per_draw: 0.0001,
                draws: 0,
                draw_fail: None,
                drawing: None,
                resume: None,
            }
        }
    }

    impl Graphics for Fake {
        fn pixels(&self) -> u64 {
            1_000_000
        }
        fn draw(&mut self, draws: u32) -> Result<Option<f64>, String> {
            self.draws += 1;
            if let Some(s) = self.drawing.take() {
                s.send(()).unwrap();
            }
            if let Some(r) = self.resume.take() {
                r.recv().unwrap();
            }
            if let Some(error) = self.draw_fail {
                return Err(error.into());
            }
            Ok(Some(draws as f64 * self.per_draw))
        }
    }

    impl Compute for Fake {
        fn threads(&self) -> u64 {
            1_000_000
        }
        fn run(&mut self, dispatches: u32) -> Result<Option<f64>, String> {
            self.batches += 1;
            if let Some(s) = self.started.take() {
                s.send(()).unwrap();
            }
            if let Some((n, error)) = self.fail_at {
                if self.batches >= n {
                    return Err(error.into());
                }
            }
            if self.per > 1e-6 {
                // Batches of a slow fake take wall time too.
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(Some(dispatches as f64 * self.per))
        }
    }

    fn run_fake(fake: Fake, seconds: f64) -> EngineState {
        let mut engine = Engine::default();
        let mut fake = fake;
        engine.start(vec![Unit::new("GPU", move |p| compute(p, &mut fake, seconds))]);
        wait_idle(&engine)
    }

    #[test]
    fn the_chosen_card_is_tested_else_the_first_listed() {
        let cards = [card("AMD Radeon(TM) Graphics", 7), card("NVIDIA GeForce RTX 4080", 9)];
        assert_eq!(chosen(&cards, Some(9)).map(|a| a.name.as_str()), Some("NVIDIA GeForce RTX 4080"));
        assert_eq!(chosen(&cards, None).map(|a| a.name.as_str()), Some("AMD Radeon(TM) Graphics"));
        assert_eq!(chosen(&cards, Some(42)).map(|a| a.name.as_str()), Some("AMD Radeon(TM) Graphics"), "a card no longer listed");
        assert_eq!(chosen(&[], Some(9)), None);
    }

    #[test]
    fn the_card_with_the_most_video_memory_comes_first() {
        let gib = 1u64 << 30;
        let listed = vec![(card("AMD Radeon(TM) Graphics", 7), 512 << 20), (card("NVIDIA GeForce RTX 4080", 9), 16 * gib), (card("Second 4080", 11), 16 * gib)];
        let names: Vec<_> = high_performance_first(listed).into_iter().map(|a| a.name).collect();
        assert_eq!(names, ["NVIDIA GeForce RTX 4080", "Second 4080", "AMD Radeon(TM) Graphics"], "equal cards keep DXGI's order");
    }

    #[test]
    fn compute_is_reported_in_gflops_from_the_gpu_time() {
        // A million threads × 2560 flops per millisecond: 2560 GFLOPS.
        let s = run_fake(Fake::new(0.001), 0.2);
        assert_eq!(s.status().as_deref(), Some("Test complete"));
        let figures = s.results("GPU");
        assert_eq!(figures.len(), 1);
        assert_eq!((figures[0].name, figures[0].unit), ("Compute", GFLOPS));
        assert!((figures[0].value - 2560.0).abs() < 1e-6, "{}", figures[0].value);
        assert_eq!(figures[0].text(), "2560 GFLOPS");
    }

    #[test]
    fn a_cancelled_compute_run_reports_nothing() {
        let (started, wait_started) = channel();
        let fake = Fake { started: Some(started), ..Fake::new(1e-9) };
        let mut engine = Engine::default();
        let mut fake = fake;
        engine.start(vec![Unit::new("GPU", move |p| compute(p, &mut fake, 60.0))]);
        wait_started.recv().unwrap();
        engine.cancel();
        let s = wait_idle(&engine);
        assert_eq!(s.status().as_deref(), Some("Cancelled"));
        assert!(s.results("GPU").is_empty());
    }

    #[test]
    fn failed_compute_work_fails_the_test_with_its_message() {
        let fake = Fake { fail_at: Some((3, COMPUTE_FAILED)), ..Fake::new(0.001) };
        let s = run_fake(fake, 1.0);
        assert_eq!(s.status().as_deref(), Some("GPU test failed: Direct3D compute work failed."));
        assert!(s.results("GPU").is_empty());
    }

    #[test]
    fn a_card_that_cannot_be_opened_fails_the_test_with_its_message() {
        let mut engine = Engine::default();
        engine.start(vec![unit_with(Some(card("Card", 1)), |_, _| Err(SHADERS_NOT_COMPILED.into()))]);
        let s = wait_idle(&engine);
        assert_eq!(s.status().as_deref(), Some("GPU test failed: The Direct3D benchmark shaders could not be compiled."));

        let mut engine = Engine::default();
        engine.start(vec![unit_with(None, |_, _| unreachable!("no card to open"))]);
        let s = wait_idle(&engine);
        assert_eq!(s.status().as_deref(), Some("GPU test failed: A hardware Direct3D 11 GPU is unavailable on this system."));
    }

    #[test]
    fn the_gpu_unit_opens_the_given_card_and_measures_compute() {
        let (opened, wait_opened) = channel();
        let mut engine = Engine::default();
        engine.start(vec![unit_with_seconds(Some(card("NVIDIA GeForce RTX 4080", 9)), 0.1, move |a, _| {
            opened.send(a.clone()).unwrap();
            Ok(Box::new(Fake::new(0.001)))
        })]);
        let s = wait_idle(&engine);
        assert_eq!(wait_opened.recv().unwrap(), card("NVIDIA GeForce RTX 4080", 9));
        assert_eq!(s.status().as_deref(), Some("Test complete"));
        assert_eq!(s.results("GPU")[0].name, "Compute");
    }

    fn run_unit(fake: Fake, seconds: f64) -> Engine {
        let mut engine = Engine::default();
        engine.start(vec![unit_with_seconds(Some(card("Card", 1)), seconds, move |_, _| Ok(Box::new(fake)))]);
        engine
    }

    #[test]
    fn the_gpu_test_reports_compute_then_graphics_in_gpixels() {
        // A million pixels per 0.1 ms: 10 Gpixels/sec.
        let engine = run_unit(Fake::new(0.001), 0.1);
        let s = wait_idle(&engine);
        assert_eq!(s.status().as_deref(), Some("Test complete"));
        let figures = s.results("GPU");
        let names: Vec<_> = figures.iter().map(|f| (f.name, f.unit)).collect();
        assert_eq!(names, [("Compute", GFLOPS), ("Graphics", GPIXELS)]);
        assert!((figures[1].value - 10.0).abs() < 1e-6, "{}", figures[1].value);
        assert_eq!(figures[1].text(), "10.00 Gpixels/sec");
    }

    #[test]
    fn the_graphics_phase_follows_compute() {
        let (drawing, wait_drawing) = channel();
        let (resume, wait_resume) = channel();
        let fake = Fake { drawing: Some(drawing), resume: Some(wait_resume), ..Fake::new(0.001) };
        let engine = run_unit(fake, 0.1);
        wait_drawing.recv().unwrap();
        let s = engine.state();
        assert_eq!(s.phase, Some("Testing GPU graphics"));
        assert!(s.progress >= 0.5, "compute is the first half: {}", s.progress);
        assert_eq!(s.partial.iter().map(|f| f.name).collect::<Vec<_>>(), ["Compute"]);
        resume.send(()).unwrap();
        assert_eq!(wait_idle(&engine).status().as_deref(), Some("Test complete"));
    }

    #[test]
    fn a_run_cancelled_while_drawing_reports_nothing() {
        let (drawing, wait_drawing) = channel();
        let (resume, wait_resume) = channel();
        let fake = Fake { drawing: Some(drawing), resume: Some(wait_resume), per_draw: 1e-9, ..Fake::new(0.001) };
        let engine = run_unit(fake, 0.1);
        wait_drawing.recv().unwrap();
        engine.cancel();
        resume.send(()).unwrap();
        let s = wait_idle(&engine);
        assert_eq!(s.status().as_deref(), Some("Cancelled"));
        assert!(s.results("GPU").is_empty());
    }

    #[test]
    fn a_run_cancelled_while_computing_never_draws() {
        let (started, wait_started) = channel();
        let (drawn, wait_drawn) = channel();
        let fake = Fake { started: Some(started), drawing: Some(drawn), ..Fake::new(1e-9) };
        let engine = run_unit(fake, 60.0);
        wait_started.recv().unwrap();
        engine.cancel();
        let s = wait_idle(&engine);
        assert_eq!(s.status().as_deref(), Some("Cancelled"));
        assert!(wait_drawn.try_recv().is_err(), "the graphics phase ran");
    }

    #[test]
    fn failed_graphics_work_fails_the_test_with_its_message() {
        let fake = Fake { draw_fail: Some(GRAPHICS_FAILED), ..Fake::new(0.001) };
        let s = wait_idle(&run_unit(fake, 0.1));
        assert_eq!(s.status().as_deref(), Some("GPU test failed: Direct3D graphics work failed."));
        assert!(s.results("GPU").is_empty());
    }

    #[test]
    fn graphics_that_never_times_validly_fails_the_test() {
        let fake = Fake { per_draw: 0.0, ..Fake::new(0.001) };
        let s = wait_idle(&run_unit(fake, 0.1));
        assert_eq!(s.status().as_deref(), Some("GPU test failed: Direct3D graphics work failed."));
    }

    /// A card whose timing query never answers: like the real device, it
    /// waits for it while the run is not cancelled (for 30 s at most).
    struct Hung {
        cancel: Cancel,
        started: Sender<()>,
    }

    impl Hung {
        fn wait(&mut self) -> Result<Option<f64>, String> {
            let _ = self.started.send(());
            let t = Instant::now();
            while !self.cancel.cancelled() && t.elapsed() < Duration::from_secs(30) {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(COMPUTE_FAILED.into())
        }
    }

    impl Compute for Hung {
        fn threads(&self) -> u64 {
            1
        }
        fn run(&mut self, _: u32) -> Result<Option<f64>, String> {
            self.wait()
        }
    }

    impl Graphics for Hung {
        fn pixels(&self) -> u64 {
            1
        }
        fn draw(&mut self, _: u32) -> Result<Option<f64>, String> {
            self.wait()
        }
    }

    #[test]
    fn a_card_waiting_for_the_gpu_sees_the_cancel_at_once() {
        let (started, wait_started) = channel();
        let mut engine = Engine::default();
        engine.start(vec![unit_with_seconds(Some(card("Card", 1)), 60.0, move |_, cancel| Ok(Box::new(Hung { cancel, started })))]);
        wait_started.recv().unwrap();
        let t = Instant::now();
        engine.cancel();
        let s = wait_idle(&engine);
        assert!(t.elapsed() < Duration::from_secs(2), "took {:?}", t.elapsed());
        assert_eq!(s.status().as_deref(), Some("Cancelled"));
    }

    #[test]
    fn the_graphics_shaders_are_a_vertex_and_pixel_pair_with_a_64_step_loop() {
        assert!(GRAPHICS_SHADERS.contains(&format!("{VERTEX_ENTRY}(uint id : SV_VertexID)")));
        assert!(GRAPHICS_SHADERS.contains(&format!("float4 {PIXEL_ENTRY}(")));
        assert!(GRAPHICS_SHADERS.contains("for (uint step = 0; step < 64; ++step)"));
    }

    /// Lists this PC's cards and runs the GPU test (compute, then
    /// graphics) on the chosen one:
    /// `BENCH_GPU` (part of its name) picks it, else the high-performance card.
    /// `cargo test --release gpu_test_on_this_pc -- --ignored --nocapture`
    #[test]
    #[ignore = "needs a real Direct3D 11 GPU"]
    fn gpu_test_on_this_pc() {
        let cards = adapters();
        for a in &cards {
            println!("adapter: {} (LUID {:#x})", a.name, a.luid);
        }
        assert!(!cards.is_empty(), "no hardware Direct3D 11 card listed");
        let wanted = std::env::var("BENCH_GPU").ok();
        let choice = wanted.and_then(|w| cards.iter().find(|a| a.name.contains(&w)).map(|a| a.luid));
        let card = chosen(&cards, choice).cloned();
        println!("tested: {}", card.as_ref().unwrap().name);
        let mut engine = Engine::default();
        engine.start(vec![unit(card)]);
        let s = wait_idle(&engine);
        assert_eq!(s.status().as_deref(), Some("Test complete"), "{:?}", s.outcome);
        let figures = s.results("GPU");
        for f in figures {
            println!("{}: {}", f.name, f.text());
        }
        assert_eq!(figures.iter().map(|f| f.name).collect::<Vec<_>>(), ["Compute", "Graphics"]);
        assert!(figures.iter().all(|f| f.value > 0.0));
    }

    #[test]
    fn each_thread_counts_two_flops_per_multiply_add_of_its_eight_chains() {
        // 8 chains × 160 steps = 1280 multiply-adds, 2 FLOPs each.
        assert_eq!(FLOPS_PER_THREAD, 2560.0);
        // 8192 groups of 128 threads: 1,048,576 threads, 2,684,354,560 FLOPs.
        assert_eq!(DISPATCH_THREADS, 1_048_576);
        assert_eq!(DISPATCH_THREADS as f64 * FLOPS_PER_THREAD, 2_684_354_560.0);
    }

    #[test]
    fn the_compute_shader_is_built_from_the_counted_shape() {
        let source = compute_shader();
        assert!(source.contains(&format!("void {COMPUTE_ENTRY}(uint3 thread : SV_DispatchThreadID)")));
        assert!(source.contains("[numthreads(128, 1, 1)]"));
        assert!(source.contains("for (uint step = 0; step < 160; ++step)"));
        assert_eq!(source.matches("= mad(").count(), 2, "two float4 chains, four lanes each");
    }
}
