//! DXGI lists the cards; a Direct3D 11 device on one runs the compute
//! shader and draws with the graphics shaders, timed with timestamp
//! queries.

use std::time::{Duration, Instant};

use ::windows::core::{Interface, HRESULT, PCSTR};
use ::windows::Win32::Foundation::{HMODULE, LUID, S_FALSE, S_OK};
use ::windows::Win32::Graphics::Direct3D::Fxc::{D3DCompile, D3DCOMPILE_OPTIMIZATION_LEVEL3};
use ::windows::Win32::Graphics::Direct3D::{
    ID3DBlob, D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
};
use ::windows::Win32::Graphics::Direct3D11::*;
use ::windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC};
use ::windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};

use super::*;

/// The side of the square render target: 4,194,304 pixels per draw.
const TARGET_SIDE: u32 = 2048;
/// How long a query may take before the work counts as failed.
const QUERY_TIMEOUT: Duration = Duration::from_secs(10);

fn luid(l: LUID) -> i64 {
    ((l.HighPart as i64) << 32) | l.LowPart as i64
}

/// The hardware cards with their DXGI adapters and dedicated video
/// memory, in DXGI order.
fn listed() -> Vec<(Adapter, IDXGIAdapter1, u64)> {
    // SAFETY: plain DXGI calls; every out value is owned by the returned COM wrappers.
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else { return Vec::new() };
    let mut out = Vec::new();
    for i in 0.. {
        let Ok(a) = (unsafe { factory.EnumAdapters1(i) }) else { break };
        let Ok(d) = (unsafe { a.GetDesc1() }) else { continue };
        if d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let len = d.Description.iter().position(|&c| c == 0).unwrap_or(d.Description.len());
        let name = String::from_utf16_lossy(&d.Description[..len]).trim().to_string();
        out.push((Adapter { name, luid: luid(d.AdapterLuid) }, a, d.DedicatedVideoMemory as u64));
    }
    out
}

pub fn adapters() -> Vec<Adapter> {
    high_performance_first(listed().into_iter().map(|(a, _, memory)| (a, memory)).collect())
}

/// Compiles `entry` of `source` for `target` (a NUL-terminated profile).
///
/// # Safety
/// Plain Direct3D compiler call; the blob owns the bytecode.
unsafe fn compile(source: &str, entry: &str, target: &'static [u8]) -> Result<ID3DBlob, String> {
    let entry = format!("{entry}\0");
    let mut blob: Option<ID3DBlob> = None;
    D3DCompile(
        source.as_ptr().cast(),
        source.len(),
        PCSTR(b"benchmark\0".as_ptr()),
        None,
        None,
        PCSTR(entry.as_ptr()),
        PCSTR(target.as_ptr()),
        D3DCOMPILE_OPTIMIZATION_LEVEL3,
        0,
        &mut blob,
        None,
    )
    .map_err(|_| SHADERS_NOT_COMPILED)?;
    blob.ok_or_else(|| SHADERS_NOT_COMPILED.to_string())
}

/// The bytes of a compiled shader.
///
/// # Safety
/// The slice lives as long as `blob`.
unsafe fn bytecode(blob: &ID3DBlob) -> &[u8] {
    std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize())
}

/// Both parts' shaders ready on one card.
pub struct Device {
    context: ID3D11DeviceContext,
    compute: ID3D11ComputeShader,
    output: ID3D11UnorderedAccessView,
    vertex: ID3D11VertexShader,
    pixel: ID3D11PixelShader,
    target: ID3D11RenderTargetView,
    disjoint: ID3D11Query,
    begin: ID3D11Query,
    end: ID3D11Query,
    /// The run's cancel flag, seen while waiting for the GPU.
    cancel: Cancel,
}

impl Device {
    /// Creates a Direct3D 11 device on `adapter`, compiles the shaders
    /// and allocates their buffer, render target and queries; waits for
    /// the GPU end at once when `cancel` is set.
    pub fn open(adapter: &Adapter, cancel: Cancel) -> Result<Device, String> {
        let dxgi = listed().into_iter().find(|(a, ..)| a.luid == adapter.luid).map(|(_, d, _)| d).ok_or(NO_GPU)?;
        // SAFETY: Direct3D calls with valid descriptors; out values are owned COM wrappers.
        unsafe {
            let (mut device, mut context) = (None, None);
            D3D11CreateDevice(
                &dxgi,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_FLAG(0),
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .map_err(|_| NO_GPU)?;
            let (device, context): (ID3D11Device, ID3D11DeviceContext) = (device.ok_or(NO_GPU)?, context.ok_or(NO_GPU)?);

            let cs = compile(&compute_shader(), COMPUTE_ENTRY,b"cs_5_0\0")?;
            let vs = compile(GRAPHICS_SHADERS, VERTEX_ENTRY, b"vs_5_0\0")?;
            let ps = compile(GRAPHICS_SHADERS, PIXEL_ENTRY, b"ps_5_0\0")?;

            let mut compute = None;
            device.CreateComputeShader(bytecode(&cs), None, Some(&mut compute)).map_err(|_| PIPELINES_NOT_CREATED)?;
            let compute = compute.ok_or(PIPELINES_NOT_CREATED)?;
            let mut vertex = None;
            device.CreateVertexShader(bytecode(&vs), None, Some(&mut vertex)).map_err(|_| PIPELINES_NOT_CREATED)?;
            let vertex = vertex.ok_or(PIPELINES_NOT_CREATED)?;
            let mut pixel = None;
            device.CreatePixelShader(bytecode(&ps), None, Some(&mut pixel)).map_err(|_| PIPELINES_NOT_CREATED)?;
            let pixel = pixel.ok_or(PIPELINES_NOT_CREATED)?;

            let desc = D3D11_BUFFER_DESC {
                ByteWidth: DISPATCH_THREADS as u32 * 4,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_UNORDERED_ACCESS.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: D3D11_RESOURCE_MISC_BUFFER_STRUCTURED.0 as u32,
                StructureByteStride: 4,
            };
            let mut buffer = None;
            device.CreateBuffer(&desc, None, Some(&mut buffer)).map_err(|_| RESOURCES_NOT_ALLOCATED)?;
            let buffer = buffer.ok_or(RESOURCES_NOT_ALLOCATED)?;
            let mut output = None;
            device.CreateUnorderedAccessView(&buffer, None, Some(&mut output)).map_err(|_| RESOURCES_NOT_ALLOCATED)?;
            let output = output.ok_or(RESOURCES_NOT_ALLOCATED)?;

            let desc = D3D11_TEXTURE2D_DESC {
                Width: TARGET_SIDE,
                Height: TARGET_SIDE,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let mut texture = None;
            device.CreateTexture2D(&desc, None, Some(&mut texture)).map_err(|_| RESOURCES_NOT_ALLOCATED)?;
            let texture = texture.ok_or(RESOURCES_NOT_ALLOCATED)?;
            let mut target = None;
            device.CreateRenderTargetView(&texture, None, Some(&mut target)).map_err(|_| RESOURCES_NOT_ALLOCATED)?;
            let target = target.ok_or(RESOURCES_NOT_ALLOCATED)?;

            let query = |kind| -> Result<ID3D11Query, String> {
                let mut q = None;
                device.CreateQuery(&D3D11_QUERY_DESC { Query: kind, MiscFlags: 0 }, Some(&mut q)).map_err(|_| SYNC_UNAVAILABLE)?;
                q.ok_or_else(|| SYNC_UNAVAILABLE.to_string())
            };
            let disjoint = query(D3D11_QUERY_TIMESTAMP_DISJOINT)?;
            let begin = query(D3D11_QUERY_TIMESTAMP)?;
            let end = query(D3D11_QUERY_TIMESTAMP)?;
            Ok(Device { context, compute, output, vertex, pixel, target, disjoint, begin, end, cancel })
        }
    }

    /// Waits for `query`'s data; `failed` when it never comes or the run
    /// is cancelled (the engine then reports the cancel, not the error).
    fn data<T: Default>(&self, query: &ID3D11Query, failed: &str) -> Result<T, String> {
        let mut value = T::default();
        let t = Instant::now();
        loop {
            // SAFETY: `value` is a `T` of the size given, the data type of this query.
            // The raw call keeps S_FALSE (not ready), which the wrapper folds into Ok.
            let hr: HRESULT = unsafe {
                (Interface::vtable(&self.context).GetData)(
                    self.context.as_raw(),
                    query.as_raw(),
                    (&mut value as *mut T).cast(),
                    std::mem::size_of::<T>() as u32,
                    0,
                )
            };
            if hr == S_OK {
                return Ok(value);
            }
            if hr != S_FALSE || t.elapsed() > QUERY_TIMEOUT || self.cancel.cancelled() {
                return Err(failed.into());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Records `work` between two timestamps and waits for it: the GPU
    /// seconds it took, or none when the timing is not valid.
    fn timed(&self, failed: &str, work: impl FnOnce(&ID3D11DeviceContext)) -> Result<Option<f64>, String> {
        let c = &self.context;
        // SAFETY: the queries were made on this context's device.
        unsafe {
            c.Begin(&self.disjoint);
            c.End(&self.begin);
        }
        work(c);
        // SAFETY: as above.
        unsafe {
            c.End(&self.end);
            c.End(&self.disjoint);
        }
        let timing: D3D11_QUERY_DATA_TIMESTAMP_DISJOINT = self.data(&self.disjoint, failed)?;
        let begin: u64 = self.data(&self.begin, failed)?;
        let end: u64 = self.data(&self.end, failed)?;
        if timing.Disjoint.as_bool() || timing.Frequency == 0 || end <= begin {
            return Ok(None);
        }
        Ok(Some((end - begin) as f64 / timing.Frequency as f64))
    }
}

impl Compute for Device {
    fn threads(&self) -> u64 {
        DISPATCH_THREADS
    }

    fn run(&mut self, dispatches: u32) -> Result<Option<f64>, String> {
        self.timed(COMPUTE_FAILED, |c| {
            // SAFETY: the shader and view were made on this context's device.
            unsafe {
                c.CSSetShader(&self.compute, None);
                c.CSSetUnorderedAccessViews(0, 1, Some(&Some(self.output.clone())), None);
                for _ in 0..dispatches {
                    c.Dispatch(DISPATCH_GROUPS, 1, 1);
                }
            }
        })
    }
}

impl Graphics for Device {
    fn pixels(&self) -> u64 {
        TARGET_SIDE as u64 * TARGET_SIDE as u64
    }

    fn draw(&mut self, draws: u32) -> Result<Option<f64>, String> {
        self.timed(GRAPHICS_FAILED, |c| {
            let viewport = D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: TARGET_SIDE as f32,
                Height: TARGET_SIDE as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            };
            // SAFETY: the shaders and target view were made on this context's device;
            // no input layout is needed as the vertex shader reads only SV_VertexID.
            unsafe {
                c.IASetInputLayout(None);
                c.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
                c.VSSetShader(&self.vertex, None);
                c.PSSetShader(&self.pixel, None);
                c.OMSetRenderTargets(Some(&[Some(self.target.clone())]), None);
                c.RSSetViewports(Some(&[viewport]));
                for _ in 0..draws {
                    c.Draw(3, 0);
                }
            }
        })
    }
}

/// Both parts' shaders on `adapter`, with Direct3D 11.
pub fn open(adapter: &Adapter, cancel: Cancel) -> Result<Box<dyn Gpu>, String> {
    Ok(Box::new(Device::open(adapter, cancel)?))
}
