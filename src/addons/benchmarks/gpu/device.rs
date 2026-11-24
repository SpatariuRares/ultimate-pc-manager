//! The wgpu side of the GPU test, the same on every system: the backend
//! each system uses (`system::BACKENDS`) lists the cards, and a device on
//! one runs the compute shader and draws with the graphics shaders, timed
//! with timestamp queries or, without them, with the CPU clock.

use std::future::Future;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use super::*;

/// The side of the square render target: 4,194,304 pixels per draw.
const TARGET_SIDE: u32 = 2048;
/// How long a batch may take before the work counts as failed.
const BATCH_TIMEOUT: Duration = Duration::from_secs(10);
/// The render target's format.
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Runs a future to its end on this thread (wgpu's requests answer at once
/// on the native backends).
fn block_on<F: Future>(future: F) -> F::Output {
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
            return value;
        }
        std::thread::park();
    }
}

fn instance() -> wgpu::Instance {
    wgpu::Instance::new(&wgpu::InstanceDescriptor { backends: system::BACKENDS, ..Default::default() })
}

fn kind(t: wgpu::DeviceType) -> Kind {
    match t {
        wgpu::DeviceType::DiscreteGpu => Kind::Discrete,
        wgpu::DeviceType::IntegratedGpu => Kind::Integrated,
        wgpu::DeviceType::VirtualGpu => Kind::Virtual,
        wgpu::DeviceType::Cpu => Kind::Software,
        wgpu::DeviceType::Other => Kind::Other,
    }
}

fn describe(adapters: &[wgpu::Adapter]) -> Vec<Found> {
    adapters
        .iter()
        .map(|a| {
            let i = a.get_info();
            Found { name: i.name.trim().to_string(), vendor: i.vendor, device: i.device, kind: kind(i.device_type) }
        })
        .collect()
}

/// The adapters the system's backend reports, unfiltered.
pub fn found() -> Vec<Found> {
    describe(&instance().enumerate_adapters(system::BACKENDS))
}

/// How a batch's time is read back.
enum Timer {
    /// The CPU clock around a batch that is waited for.
    Cpu,
    /// Two timestamps written by the pass, resolved into `resolve` and
    /// copied to `read`.
    Gpu { set: wgpu::QuerySet, resolve: wgpu::Buffer, read: wgpu::Buffer, tick_nanos: f32 },
}

/// Both parts' shaders ready on one card.
pub struct Device {
    device: wgpu::Device,
    queue: wgpu::Queue,
    compute: wgpu::ComputePipeline,
    bind: wgpu::BindGroup,
    render: wgpu::RenderPipeline,
    target: wgpu::TextureView,
    timer: Timer,
    /// The run's cancel flag, seen while waiting for the GPU.
    cancel: Cancel,
}

impl Device {
    /// Opens a device on `adapter`, builds the pipelines and allocates the
    /// buffer and render target; `cpu_clock` forces CPU timing even when
    /// the device has timestamps.
    pub fn open(adapter: &Adapter, cancel: Cancel, cpu_clock: bool) -> Result<Device, String> {
        let adapters = instance().enumerate_adapters(system::BACKENDS);
        let found = describe(&adapters);
        let index = listed(&found).into_iter().find(|(a, _)| a.id == adapter.id).map(|(_, i)| i).ok_or(NO_GPU)?;
        let card = &adapters[index];
        let has_timestamps = card.features().contains(wgpu::Features::TIMESTAMP_QUERY);
        let (device, queue) = block_on(card.request_device(&wgpu::DeviceDescriptor {
            label: Some("benchmark"),
            required_features: if has_timestamps { wgpu::Features::TIMESTAMP_QUERY } else { wgpu::Features::empty() },
            ..Default::default()
        }))
        .map_err(|_| NO_GPU)?;
        // A shader that does not build is reported as an error scope.
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let compute_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("compute"),
            source: wgpu::ShaderSource::Wgsl(compute_shader().into()),
        });
        let graphics_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("graphics"),
            source: wgpu::ShaderSource::Wgsl(graphics_shaders().into()),
        });
        if block_on(device.pop_error_scope()).is_some() {
            return Err(SHADERS_NOT_COMPILED.into());
        }

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let compute = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("compute"),
            layout: None,
            module: &compute_module,
            entry_point: Some(COMPUTE_ENTRY),
            compilation_options: Default::default(),
            cache: None,
        });
        let render = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("graphics"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &graphics_module,
                entry_point: Some(VERTEX_ENTRY),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &graphics_module,
                entry_point: Some(PIXEL_ENTRY),
                compilation_options: Default::default(),
                targets: &[Some(TARGET_FORMAT.into())],
            }),
            multiview: None,
            cache: None,
        });
        if block_on(device.pop_error_scope()).is_some() {
            return Err(PIPELINES_NOT_CREATED.into());
        }

        device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("results"),
            size: DISPATCH_THREADS * 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("target"),
            size: wgpu::Extent3d { width: TARGET_SIDE, height: TARGET_SIDE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TARGET_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        if block_on(device.pop_error_scope()).is_some() {
            return Err(RESOURCES_NOT_ALLOCATED.into());
        }
        let target = texture.create_view(&Default::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("results"),
            layout: &compute.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
        });

        let tick = queue.get_timestamp_period();
        let timer = match timing_for(has_timestamps && !cpu_clock, tick) {
            Timing::CpuClock => Timer::Cpu,
            Timing::Timestamps(tick_nanos) => Timer::Gpu {
                set: device.create_query_set(&wgpu::QuerySetDescriptor { label: Some("timestamps"), ty: wgpu::QueryType::Timestamp, count: 2 }),
                resolve: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("resolve"),
                    size: 16,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                }),
                read: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("read"),
                    size: 16,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                tick_nanos,
            },
        };
        Ok(Device { device, queue, compute, bind, render, target, timer, cancel })
    }

    /// Polls the device until `done` is set; `failed` when it takes too
    /// long or the run is cancelled (the engine then reports the cancel,
    /// not the error).
    fn wait(&self, done: &AtomicBool, failed: &str) -> Result<(), String> {
        let start = Instant::now();
        while !done.load(Ordering::Acquire) {
            if self.device.poll(wgpu::PollType::Poll).is_err() || start.elapsed() > BATCH_TIMEOUT || self.cancel.cancelled() {
                return Err(failed.into());
            }
            if !done.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_micros(100));
            }
        }
        Ok(())
    }

    /// Records `work` (which gets the timestamp writes to put in its pass)
    /// and waits for it: the seconds it took, or none when the timing is
    /// not valid.
    fn timed<W>(&self, failed: &str, work: W) -> Result<Option<f64>, String>
    where
        W: FnOnce(&mut wgpu::CommandEncoder, Option<&wgpu::QuerySet>),
    {
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let set = match &self.timer {
            Timer::Gpu { set, .. } => Some(set),
            Timer::Cpu => None,
        };
        work(&mut encoder, set);
        if let Timer::Gpu { set, resolve, read, .. } = &self.timer {
            encoder.resolve_query_set(set, 0..2, resolve, 0);
            encoder.copy_buffer_to_buffer(resolve, 0, read, 0, 16);
        }
        let done = Arc::new(AtomicBool::new(false));
        let start = Instant::now();
        self.queue.submit([encoder.finish()]);
        let flag = done.clone();
        self.queue.on_submitted_work_done(move || flag.store(true, Ordering::Release));
        self.wait(&done, failed)?;
        let wall = start.elapsed().as_secs_f64();
        let Timer::Gpu { read, tick_nanos, .. } = &self.timer else { return Ok(Some(wall)) };

        let mapped = Arc::new(AtomicBool::new(false));
        let flag = mapped.clone();
        read.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            if result.is_ok() {
                flag.store(true, Ordering::Release);
            }
        });
        self.wait(&mapped, failed)?;
        let ticks: Vec<u64> = {
            let data = read.slice(..).get_mapped_range();
            data.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect()
        };
        read.unmap();
        Ok(timestamp_seconds(ticks[0], ticks[1], *tick_nanos))
    }
}

impl Compute for Device {
    fn threads(&self) -> u64 {
        DISPATCH_THREADS
    }

    fn run(&mut self, dispatches: u32) -> Result<Option<f64>, String> {
        self.timed(COMPUTE_FAILED, |encoder, set| {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("compute"),
                timestamp_writes: set.map(|query_set| wgpu::ComputePassTimestampWrites {
                    query_set,
                    beginning_of_pass_write_index: Some(0),
                    end_of_pass_write_index: Some(1),
                }),
            });
            pass.set_pipeline(&self.compute);
            pass.set_bind_group(0, &self.bind, &[]);
            for _ in 0..dispatches {
                pass.dispatch_workgroups(DISPATCH_GROUPS, 1, 1);
            }
        })
    }
}

impl Graphics for Device {
    fn pixels(&self) -> u64 {
        TARGET_SIDE as u64 * TARGET_SIDE as u64
    }

    fn draw(&mut self, draws: u32) -> Result<Option<f64>, String> {
        self.timed(GRAPHICS_FAILED, |encoder, set| {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("graphics"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: set.map(|query_set| wgpu::RenderPassTimestampWrites {
                    query_set,
                    beginning_of_pass_write_index: Some(0),
                    end_of_pass_write_index: Some(1),
                }),
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.render);
            for _ in 0..draws {
                pass.draw(0..3, 0..1);
            }
        })
    }
}
