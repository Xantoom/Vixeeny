// SPDX-License-Identifier: GPL-3.0-or-later
//! The frames of an HDR monitor (scRGB half floats) made ready for the encoder on the graphics card, by shaders rather
//! than the video processor: drivers differ there (NVIDIA's takes no half floats in, and makes no PQ out of RGB).
//!
//! - SDR: tone mapped as the screenshots are (`colour::HLSL`: the same steps and numbers), up to the brightest pixel of
//!   the picture, followed smoothly from frame to frame so that the brightness never jumps; then 8-bit BGRA, which the
//!   video processor converts and scales.
//! - HDR: BT.2020 / PQ, written straight into the luma and chroma planes of a P010 frame (limited range, chroma sited
//!   left), scaled on the way.
//!
//! The brightest pixel and its smoothed value never leave the card.

use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BIND_UNORDERED_ACCESS,
    D3D11_BOX, D3D11_BUFFER_DESC, D3D11_COMPARISON_NEVER, D3D11_FILTER_MIN_MAG_MIP_LINEAR,
    D3D11_RENDER_TARGET_VIEW_DESC, D3D11_RENDER_TARGET_VIEW_DESC_0, D3D11_RESOURCE_MISC_BUFFER_STRUCTURED,
    D3D11_RTV_DIMENSION_TEXTURE2DARRAY, D3D11_SAMPLER_DESC, D3D11_TEX2D_ARRAY_RTV, D3D11_TEXTURE_ADDRESS_CLAMP,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_VIEWPORT, ID3D11Buffer, ID3D11ComputeShader, ID3D11Device,
    ID3D11DeviceContext, ID3D11PixelShader, ID3D11RenderTargetView, ID3D11SamplerState, ID3D11ShaderResourceView,
    ID3D11Texture2D, ID3D11UnorderedAccessView, ID3D11VertexShader,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16_UNORM, DXGI_FORMAT_R16G16_UNORM,
};
use windows::core::{Interface, s};

use vixeeny_platform::d3d::{self, compile};

/// The levels of the recorded monitor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Levels {
    /// SDR white on it, in nits (Windows' "SDR content brightness").
    pub sdr_white_nits: f32,
    /// Its peak brightness, in nits.
    pub peak_nits: f32,
}

const SHADERS: &str = r"
Texture2D<float4> src : register(t0);
StructuredBuffer<float> smoothed : register(t1);
SamplerState linear_clamp : register(s0);
RWStructuredBuffer<uint> brightest : register(u0);
RWStructuredBuffer<float> state : register(u1);
cbuffer Params : register(b0) {
    float scale; float display; float rise; float fall;
    uint2 size; float2 out_size;
};

groupshared uint group_brightest;

// Positive floats order as their bits do.
[numthreads(16, 16, 1)]
void peak(uint3 id : SV_DispatchThreadID, uint index : SV_GroupIndex) {
    if (index == 0) group_brightest = 0;
    GroupMemoryBarrierWithGroupSync();
    if (id.x < size.x && id.y < size.y)
        InterlockedMax(group_brightest, asuint(brightness(src[id.xy].rgb, scale)));
    GroupMemoryBarrierWithGroupSync();
    if (index == 0) InterlockedMax(brightest[0], group_brightest);
}

// The headroom followed (`colour::follow`): the first frame takes its own.
[numthreads(1, 1, 1)]
void follow() {
    float h = headroom(asfloat(brightest[0]), display);
    float s = state[0];
    state[0] = s >= 1.0 ? s + (h - s) * (h > s ? rise : fall) : h;
}

float4 corner(uint id : SV_VertexID) : SV_Position {
    float2 uv = float2((id << 1) & 2, id & 2);
    return float4(uv * float2(2, -2) + float2(-1, 1), 0, 1);
}

float4 to_sdr(float4 pos : SV_Position) : SV_Target {
    float3 c = src.Load(int3(pos.xy, 0)).rgb;
    return float4(tonemap(c, scale, smoothed[0]), 1.0);
}

// The PQ signals at a point of the output (its pixel centres at .5).
float3 pq_at(float2 p) {
    return scrgb_to_pq(src.SampleLevel(linear_clamp, p / out_size, 0).rgb);
}

float luma(float3 p) { return dot(p, float3(0.2627, 0.6780, 0.0593)); }

// A 10-bit code as P010 stores it: in the top bits of 16.
float code(float v) { return round(v) * 64.0 / 65535.0; }

float to_luma(float4 pos : SV_Position) : SV_Target {
    return code(64.0 + 876.0 * luma(pq_at(pos.xy)));
}

// Chroma sited left: 1-2-1 across the columns around the left luma, the two rows averaged.
float2 to_chroma(float4 pos : SV_Position) : SV_Target {
    float2 l = floor(pos.xy) * 2.0 + 0.5;
    float3 p = 0;
    for (int dy = 0; dy < 2; dy++) {
        float y = l.y + dy;
        p += pq_at(float2(l.x - 1.0, y)) + 2.0 * pq_at(float2(l.x, y)) + pq_at(float2(l.x + 1.0, y));
    }
    p /= 8.0;
    float y = luma(p);
    return float2(code(512.0 + 896.0 * (p.b - y) / 1.8814),
                  code(512.0 + 896.0 * (p.r - y) / 1.4746));
}
";

fn err(what: &str, e: impl std::fmt::Display) -> String {
    format!("{what}: {e}")
}

struct Code {
    peak: Vec<u8>,
    follow: Vec<u8>,
    corner: Vec<u8>,
    to_sdr: Vec<u8>,
    to_luma: Vec<u8>,
    to_chroma: Vec<u8>,
}

/// The compiled shaders, made once per process.
fn code() -> Result<&'static Code, String> {
    static CODE: OnceLock<Result<Code, String>> = OnceLock::new();
    CODE.get_or_init(|| {
        let source = format!("{}{SHADERS}", vixeeny_common::colour::HLSL);
        let (cs, vs, ps) = (s!("cs_5_0"), s!("vs_5_0"), s!("ps_5_0"));
        Ok(Code {
            peak: compile(&source, s!("peak"), cs)?,
            follow: compile(&source, s!("follow"), cs)?,
            corner: compile(&source, s!("corner"), vs)?,
            to_sdr: compile(&source, s!("to_sdr"), ps)?,
            to_luma: compile(&source, s!("to_luma"), ps)?,
            to_chroma: compile(&source, s!("to_chroma"), ps)?,
        })
    })
    .as_ref()
    .map_err(Clone::clone)
}

/// A one-element structured buffer of 4-byte values, with a view for the compute shaders and, when `read`, one for the
/// pixel shaders.
fn cell(
    device: &ID3D11Device,
    read: bool,
) -> Result<(ID3D11UnorderedAccessView, Option<ID3D11ShaderResourceView>), String> {
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: 4,
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_UNORDERED_ACCESS.0 as u32 | if read { D3D11_BIND_SHADER_RESOURCE.0 as u32 } else { 0 },
        MiscFlags: D3D11_RESOURCE_MISC_BUFFER_STRUCTURED.0 as u32,
        StructureByteStride: 4,
        ..Default::default()
    };
    let mut buffer: Option<ID3D11Buffer> = None;
    let (mut uav, mut srv) = (None, None);
    // SAFETY: a one-element structured buffer and views of all of it; valid out-pointers.
    unsafe {
        device.CreateBuffer(&raw const desc, None, Some(&raw mut buffer)).map_err(|e| err("CreateBuffer", e))?;
        let buffer = buffer.ok_or("no buffer")?;
        device
            .CreateUnorderedAccessView(&buffer, None, Some(&raw mut uav))
            .map_err(|e| err("CreateUnorderedAccessView", e))?;
        if read {
            device
                .CreateShaderResourceView(&buffer, None, Some(&raw mut srv))
                .map_err(|e| err("CreateShaderResourceView", e))?;
        }
    }
    Ok((uav.ok_or("no view")?, srv))
}

/// What changes from frame to frame: the copy of the frame the shaders read, and the targets.
#[derive(Default)]
struct Frames {
    /// The part recorded, copied (a capture frame may not be readable by shaders).
    input: Option<(ID3D11Texture2D, ID3D11ShaderResourceView, (u32, u32))>,
    /// The SDR picture, for the video processor.
    sdr: Option<(ID3D11Texture2D, ID3D11RenderTargetView, (u32, u32))>,
    /// The planes of the pool's frames met (texture address, array slice): a pool goes round the same few.
    planes: Vec<((usize, u32), [ID3D11RenderTargetView; 2])>,
}

/// Plane views kept: the slices of one pool.
const KEPT_PLANES: usize = 32;

pub struct CardHdr {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    peak: ID3D11ComputeShader,
    follow: ID3D11ComputeShader,
    corner: ID3D11VertexShader,
    to_sdr: ID3D11PixelShader,
    to_luma: ID3D11PixelShader,
    to_chroma: ID3D11PixelShader,
    sampler: ID3D11SamplerState,
    params: ID3D11Buffer,
    brightest: ID3D11UnorderedAccessView,
    state: (ID3D11UnorderedAccessView, ID3D11ShaderResourceView),
    levels: Levels,
    /// The share of the way to the brightest pixel made at each frame, rising and falling.
    steps: (f32, f32),
    frames: Mutex<Frames>,
}

// SAFETY: the device is free-threaded and multithread protected (see `d3d_convert`), and what changes is behind the
// mutex.
unsafe impl Send for CardHdr {}
// SAFETY: see above.
unsafe impl Sync for CardHdr {}

impl CardHdr {
    /// The shaders on `device`, for a monitor of `levels` recorded at `fps` frames a second.
    pub fn new(device: &ID3D11Device, context: &ID3D11DeviceContext, levels: Levels, fps: f64) -> Result<Self, String> {
        let code = code()?;
        let (mut peak, mut follow, mut corner) = (None, None, None);
        let (mut to_sdr, mut to_luma, mut to_chroma) = (None, None, None);
        let (mut sampler, mut params) = (None, None);
        let sampling = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            ComparisonFunc: D3D11_COMPARISON_NEVER,
            MaxLOD: f32::MAX,
            ..Default::default()
        };
        let constants = D3D11_BUFFER_DESC {
            ByteWidth: 32,
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            ..Default::default()
        };
        // SAFETY: bytecode compiled for these stages; valid descriptions and out-pointers.
        unsafe {
            let shader = |e| err("shader", e);
            device.CreateComputeShader(&code.peak, None, Some(&raw mut peak)).map_err(shader)?;
            device.CreateComputeShader(&code.follow, None, Some(&raw mut follow)).map_err(shader)?;
            device.CreateVertexShader(&code.corner, None, Some(&raw mut corner)).map_err(shader)?;
            device.CreatePixelShader(&code.to_sdr, None, Some(&raw mut to_sdr)).map_err(shader)?;
            device.CreatePixelShader(&code.to_luma, None, Some(&raw mut to_luma)).map_err(shader)?;
            device.CreatePixelShader(&code.to_chroma, None, Some(&raw mut to_chroma)).map_err(shader)?;
            device
                .CreateSamplerState(&raw const sampling, Some(&raw mut sampler))
                .map_err(|e| err("CreateSamplerState", e))?;
            device
                .CreateBuffer(&raw const constants, None, Some(&raw mut params))
                .map_err(|e| err("CreateBuffer", e))?;
        }
        let (brightest, _) = cell(device, false)?;
        let (state, read) = cell(device, true)?;
        let missing = || "shader missing".to_owned();
        Ok(Self {
            device: device.clone(),
            context: context.clone(),
            peak: peak.ok_or_else(missing)?,
            follow: follow.ok_or_else(missing)?,
            corner: corner.ok_or_else(missing)?,
            to_sdr: to_sdr.ok_or_else(missing)?,
            to_luma: to_luma.ok_or_else(missing)?,
            to_chroma: to_chroma.ok_or_else(missing)?,
            sampler: sampler.ok_or_else(missing)?,
            params: params.ok_or_else(missing)?,
            brightest,
            state: (state, read.ok_or_else(missing)?),
            levels,
            steps: vixeeny_common::colour::follow_steps(fps),
            frames: Mutex::default(),
        })
    }

    /// The `rect` part of `texture` (scRGB) tone mapped to SDR: a BGRA texture of its size, valid until the next call.
    pub fn to_sdr(&self, texture: &ID3D11Texture2D, rect: RECT) -> Result<ID3D11Texture2D, String> {
        let mut frames = self.frames.lock().map_err(|_| "lock poisoned")?;
        let (input, size) = self.input(&mut frames, texture, rect)?;
        if frames.sdr.as_ref().is_none_or(|(_, _, s)| *s != size) {
            let made = d3d::texture(
                &self.device,
                size,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                0,
                None,
            )?;
            let mut view = None;
            // SAFETY: a render target view of a texture of this device.
            unsafe { self.device.CreateRenderTargetView(&made, None, Some(&raw mut view)) }
                .map_err(|e| err("CreateRenderTargetView", e))?;
            frames.sdr = Some((made, view.ok_or("no view")?, size));
        }
        let Some((sdr, target, _)) = frames.sdr.clone() else {
            return Err("no SDR texture".into());
        };
        self.set_params(size, size);
        // SAFETY: every resource belongs to this device; views are unbound once used.
        unsafe {
            let c = &self.context;
            c.ClearUnorderedAccessViewUint(&self.brightest, &[0; 4]);
            c.CSSetShaderResources(0, Some(&[Some(input.clone())]));
            let views = [Some(self.brightest.clone()), Some(self.state.0.clone())];
            c.CSSetUnorderedAccessViews(0, 2, Some(views.as_ptr()), None);
            c.CSSetConstantBuffers(0, Some(&[Some(self.params.clone())]));
            c.CSSetShader(&self.peak, None);
            c.Dispatch(size.0.div_ceil(16), size.1.div_ceil(16), 1);
            c.CSSetShader(&self.follow, None);
            c.Dispatch(1, 1, 1);
            c.CSSetShaderResources(0, Some(&[None]));
            c.CSSetUnorderedAccessViews(0, 2, Some([None, None].as_ptr()), None);
            c.CSSetShader(None::<&ID3D11ComputeShader>, None);
            self.draw(&self.to_sdr, &target, size, &input);
        }
        Ok(sdr)
    }

    /// The `rect` part of `texture` (scRGB) as BT.2020 / PQ, scaled into slice `slice` of the P010 texture `dst` (`out`
    /// pixels: both even).
    pub fn to_pq(
        &self,
        texture: &ID3D11Texture2D,
        rect: RECT,
        dst: &ID3D11Texture2D,
        slice: u32,
        out: (u32, u32),
    ) -> Result<(), String> {
        let mut frames = self.frames.lock().map_err(|_| "lock poisoned")?;
        let (input, size) = self.input(&mut frames, texture, rect)?;
        let key = (dst.as_raw() as usize, slice);
        let planes = match frames.planes.iter().find(|(k, _)| *k == key) {
            Some((_, planes)) => planes.clone(),
            None => {
                let view = |format| -> Result<ID3D11RenderTargetView, String> {
                    // The view's format picks the plane: R16 the luma, R16G16 the chroma.
                    let desc = D3D11_RENDER_TARGET_VIEW_DESC {
                        Format: format,
                        ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2DARRAY,
                        Anonymous: D3D11_RENDER_TARGET_VIEW_DESC_0 {
                            Texture2DArray: D3D11_TEX2D_ARRAY_RTV { MipSlice: 0, FirstArraySlice: slice, ArraySize: 1 },
                        },
                    };
                    let mut view = None;
                    // SAFETY: a plane view of a P010 texture of this device.
                    unsafe { self.device.CreateRenderTargetView(dst, Some(&raw const desc), Some(&raw mut view)) }
                        .map_err(|e| err("plane view", e))?;
                    view.ok_or_else(|| "no plane view".to_owned())
                };
                let planes = [view(DXGI_FORMAT_R16_UNORM)?, view(DXGI_FORMAT_R16G16_UNORM)?];
                if frames.planes.len() >= KEPT_PLANES {
                    frames.planes.remove(0);
                }
                frames.planes.push((key, planes.clone()));
                planes
            }
        };
        self.set_params(size, out);
        // SAFETY: every resource belongs to this device; views are unbound once used.
        unsafe {
            self.draw(&self.to_luma, &planes[0], out, &input);
            self.draw(&self.to_chroma, &planes[1], (out.0 / 2, out.1 / 2), &input);
        }
        Ok(())
    }

    /// The copy of the `rect` part of `texture` the shaders read, and its size.
    fn input(
        &self,
        frames: &mut Frames,
        texture: &ID3D11Texture2D,
        rect: RECT,
    ) -> Result<(ID3D11ShaderResourceView, (u32, u32)), String> {
        let size = ((rect.right - rect.left).max(1) as u32, (rect.bottom - rect.top).max(1) as u32);
        if frames.input.as_ref().is_none_or(|(_, _, s)| *s != size) {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            // SAFETY: `desc` is a valid out-pointer.
            unsafe { texture.GetDesc(&raw mut desc) };
            let made = d3d::texture(&self.device, size, desc.Format, D3D11_BIND_SHADER_RESOURCE.0 as u32, 0, None)?;
            let mut view = None;
            // SAFETY: a shader resource view of a texture of this device.
            unsafe { self.device.CreateShaderResourceView(&made, None, Some(&raw mut view)) }
                .map_err(|e| err("CreateShaderResourceView", e))?;
            frames.input = Some((made, view.ok_or("no view")?, size));
        }
        let Some((input, view, _)) = frames.input.as_ref() else {
            return Err("no input".into());
        };
        let from = D3D11_BOX {
            left: rect.left.max(0) as u32,
            top: rect.top.max(0) as u32,
            front: 0,
            right: rect.left.max(0) as u32 + size.0,
            bottom: rect.top.max(0) as u32 + size.1,
            back: 1,
        };
        // SAFETY: both textures belong to this device, in the same format; the box is the part the caller asks for,
        // inside the frame.
        unsafe {
            self.context.CopySubresourceRegion(input, 0, 0, 0, 0, texture, 0, Some(&raw const from));
        }
        Ok((view.clone(), size))
    }

    fn set_params(&self, size: (u32, u32), out: (u32, u32)) {
        let white = self.levels.sdr_white_nits.max(1.0);
        let mut params = [0_u8; 32];
        let floats = [80.0 / white, self.levels.peak_nits / white, self.steps.0, self.steps.1];
        for (at, v) in floats.iter().enumerate() {
            params[at * 4..at * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        params[16..20].copy_from_slice(&size.0.to_le_bytes());
        params[20..24].copy_from_slice(&size.1.to_le_bytes());
        params[24..28].copy_from_slice(&(out.0 as f32).to_le_bytes());
        params[28..32].copy_from_slice(&(out.1 as f32).to_le_bytes());
        // SAFETY: a 32-byte constant buffer of this device, written whole.
        unsafe {
            self.context.UpdateSubresource(&self.params, 0, None, params.as_ptr().cast(), 0, 0);
        }
    }

    /// Runs `shader` over all of `target` (`size` pixels), reading `input`.
    ///
    /// # Safety
    /// Every resource belongs to this device.
    unsafe fn draw(
        &self,
        shader: &ID3D11PixelShader,
        target: &ID3D11RenderTargetView,
        size: (u32, u32),
        input: &ID3D11ShaderResourceView,
    ) {
        let viewport = D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: size.0 as f32,
            Height: size.1 as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        let c = &self.context;
        // SAFETY: per the caller; nothing stays bound once drawn.
        unsafe {
            c.IASetInputLayout(None);
            c.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            c.VSSetShader(&self.corner, None);
            c.PSSetShader(shader, None);
            c.PSSetShaderResources(0, Some(&[Some(input.clone()), Some(self.state.1.clone())]));
            c.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            c.PSSetConstantBuffers(0, Some(&[Some(self.params.clone())]));
            c.RSSetViewports(Some(&[viewport]));
            c.OMSetRenderTargets(Some(&[Some(target.clone())]), None);
            c.Draw(3, 0);
            c.OMSetRenderTargets(Some(&[None]), None);
            c.PSSetShaderResources(0, Some(&[None, None]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vixeeny_common::colour;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_USAGE_STAGING,
    };
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_P010, DXGI_FORMAT_R16G16B16A16_FLOAT};

    const LEVELS: Levels = Levels { sdr_white_nits: 240.0, peak_nits: 1000.0 };

    fn device() -> Option<(ID3D11Device, ID3D11DeviceContext)> {
        crate::d3d_convert::create_device(None).ok()
    }

    /// A scRGB texture of `size`, each pixel given by `pixel(x, y)`, as a capture frame is (not readable by shaders).
    fn frame(device: &ID3D11Device, (w, h): (u32, u32), pixel: impl Fn(u32, u32) -> [f32; 3]) -> ID3D11Texture2D {
        let halves: Vec<u16> = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .flat_map(|(x, y)| {
                let [r, g, b] = pixel(x, y);
                [r, g, b, 1.0].map(vixeeny_common::half::from_f32)
            })
            .collect();
        let bytes: Vec<u8> = halves.iter().flat_map(|v| v.to_le_bytes()).collect();
        d3d::texture(
            device,
            (w, h),
            DXGI_FORMAT_R16G16B16A16_FLOAT,
            D3D11_BIND_RENDER_TARGET.0 as u32,
            0,
            Some((&bytes, w * 8)),
        )
        .unwrap()
    }

    /// The bytes of `texture` (`rows` rows of its pitch, planes one after the other).
    fn read(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        texture: &ID3D11Texture2D,
        rows: u32,
    ) -> (Vec<u8>, usize) {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: valid out-pointers; a staging copy of a texture of this device.
        unsafe {
            texture.GetDesc(&raw mut desc);
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut staging = None;
            device.CreateTexture2D(&raw const desc, None, Some(&raw mut staging)).unwrap();
            let staging = staging.unwrap();
            context.CopyResource(&staging, texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&raw mut mapped)).unwrap();
            let pitch = mapped.RowPitch as usize;
            let bytes = std::slice::from_raw_parts(mapped.pData.cast::<u8>(), pitch * rows as usize).to_vec();
            context.Unmap(&staging, 0);
            (bytes, pitch)
        }
    }

    /// The 10-bit Y, Cb, Cr of a scRGB colour, from the processor's PQ coding.
    fn expected(rgb: [f32; 3]) -> [i32; 3] {
        let [r, g, b] = colour::scrgb_to_pq(rgb).map(|c| f64::from(c) / 65535.0);
        let y = 0.2627 * r + 0.6780 * g + 0.0593 * b;
        [64.0 + 876.0 * y, 512.0 + 896.0 * (b - y) / 1.8814, 512.0 + 896.0 * (r - y) / 1.4746].map(|v| v.round() as i32)
    }

    /// HDR frames come out as the processor codes them, scaled or not: SDR white, a bright saturated orange, a 1000-nit
    /// white, a colour wider than BT.709, black.
    #[test]
    fn pq_planes_hold_the_processor_s_codes() {
        let Some((device, context)) = device() else {
            vixeeny_common::skip_test("no Direct3D device");
            return;
        };
        let hdr = CardHdr::new(&device, &context, LEVELS, 60.0).unwrap();
        for out in [(64u32, 48u32), (32, 24)] {
            for rgb in [[3.0, 3.0, 3.0], [10.0, 4.0, 0.2], [12.5, 12.5, 12.5], [-0.3, 1.2, 1.1], [0.0, 0.0, 0.0]] {
                let source = frame(&device, (80, 60), |_, _| rgb);
                let dst =
                    d3d::texture(&device, out, DXGI_FORMAT_P010, D3D11_BIND_RENDER_TARGET.0 as u32, 0, None).unwrap();
                let rect = RECT { left: 8, top: 6, right: 72, bottom: 54 };
                hdr.to_pq(&source, rect, &dst, 0, out).unwrap();
                let (bytes, pitch) = read(&device, &context, &dst, out.1 * 3 / 2);
                let at = |offset: usize| i32::from(u16::from_le_bytes([bytes[offset], bytes[offset + 1]]) >> 6);
                let (cx, cy) = (out.0 as usize / 4, out.1 as usize / 4);
                let chroma = out.1 as usize * pitch + cy * pitch + cx * 4;
                let got = [at((out.1 as usize / 2) * pitch + out.0 as usize), at(chroma), at(chroma + 2)];
                let want = expected(rgb);
                for (g, w) in got.iter().zip(want) {
                    assert!((g - w).abs() <= 1, "{rgb:?} at {out:?}: {got:?}, not {want:?}");
                }
            }
        }
    }

    /// SDR frames are tone mapped as the screenshots are: up to the picture's brightest pixel, white staying white
    /// without highlights, and followed smoothly from frame to frame.
    #[test]
    fn sdr_frames_follow_the_brightest_pixel() {
        let Some((device, context)) = device() else {
            vixeeny_common::skip_test("no Direct3D device");
            return;
        };
        let hdr = CardHdr::new(&device, &context, LEVELS, 60.0).unwrap();
        let white = LEVELS.sdr_white_nits / 80.0;
        let rect = RECT { left: 0, top: 0, right: 32, bottom: 16 };
        let pixel = |sdr: &ID3D11Texture2D, x: usize| {
            let (bytes, _) = read(&device, &context, sdr, 1);
            [bytes[x * 4], bytes[x * 4 + 1], bytes[x * 4 + 2]]
        };
        // The first frame takes its own brightest pixel: a highlight on the right half.
        let lit = frame(&device, (32, 16), |x, _| if x < 16 { [white; 3] } else { [12.5; 3] });
        let sdr = hdr.to_sdr(&lit, rect).unwrap();
        assert_eq!(pixel(&sdr, 20), [255; 3]);
        let scale = 80.0 / LEVELS.sdr_white_nits;
        let h = colour::headroom(colour::brightness([12.5; 3], scale), 1000.0 / 240.0);
        let code = |v: f32| (colour::srgb_encode(v) * 255.0).round() as u8;
        let rolled = code(colour::roll_off(1.0, h));
        let got = pixel(&sdr, 4);
        assert!(got.iter().all(|&c| c.abs_diff(rolled) <= 1), "{got:?}, not {rolled}");
        // The highlight gone: white comes back to 255 slowly (half a second on), not at once.
        let plain = frame(&device, (32, 16), |_, _| [white; 3]);
        for _ in 0..30 {
            hdr.to_sdr(&plain, rect).unwrap();
        }
        let sdr = hdr.to_sdr(&plain, rect).unwrap();
        let next = pixel(&sdr, 4)[0];
        assert!(next > rolled && next < 255, "{next}");
        for _ in 0..600 {
            hdr.to_sdr(&plain, rect).unwrap();
        }
        let sdr = hdr.to_sdr(&plain, rect).unwrap();
        assert_eq!(pixel(&sdr, 4), [255; 3]);
    }
}
