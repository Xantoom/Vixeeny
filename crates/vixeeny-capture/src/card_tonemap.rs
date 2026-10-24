// SPDX-License-Identifier: GPL-3.0-or-later
//! HDR screens made SDR on the graphics card: compute shaders run the tone mapper of
//! `vixeeny_image::tonemap` (same steps, same numbers) on the half floats Windows composes them
//! in, so only 8-bit BGRA crosses to memory: half the bytes, and no float work left for the
//! processor. Any failure falls back to the processor's tone mapper.

use std::cell::RefCell;
use std::sync::OnceLock;

use vixeeny_platform::{HdrInfo, PhysicalRect};
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile};
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_SHADER_RESOURCE, D3D11_BIND_UNORDERED_ACCESS, D3D11_BOX,
    D3D11_BUFFER_DESC, D3D11_CPU_ACCESS_READ, D3D11_RESOURCE_MISC_BUFFER_STRUCTURED,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, ID3D11Buffer,
    ID3D11ComputeShader, ID3D11Device, ID3D11DeviceContext, ID3D11ShaderResourceView,
    ID3D11Texture2D, ID3D11UnorderedAccessView,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_R32_UINT, DXGI_SAMPLE_DESC};
use windows::core::s;

use crate::CaptureError;
use crate::wgc::os;

/// `tonemap_frame`, step for step (the functions are `colour::HLSL`): `peak` finds the brightest
/// pixel, `main` rolls the picture off up to it and packs it as BGRA bytes, rounded as the
/// processor rounds them. The brightest pixel stays on the card between the two.
const SHADER: &str = r"
Texture2D<float4> src : register(t0);
RWTexture2D<uint> dst : register(u0);
RWStructuredBuffer<uint> brightest : register(u1);
cbuffer Params : register(b0) { float scale; float display; uint2 size; };

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

uint q(float v) { return (uint)(v * 255.0 + 0.5); }

[numthreads(16, 16, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= size.x || id.y >= size.y) return;
    float h = headroom(asfloat(brightest[0]), display);
    float3 o = tonemap(src[id.xy].rgb, scale, h);
    dst[id.xy] = q(o.b) | (q(o.g) << 8) | (q(o.r) << 16) | 0xff000000u;
}
";

const GROUP: u32 = 16;

/// The bytecode of `peak` and of `main`.
type Code = (Vec<u8>, Vec<u8>);

/// The compiled shaders, made once per process.
fn bytecode() -> Result<&'static Code, CaptureError> {
    static CODE: OnceLock<Result<Code, String>> = OnceLock::new();
    CODE.get_or_init(|| {
        let source = format!("{}{SHADER}", vixeeny_common::colour::HLSL);
        Ok((compile(&source, s!("peak"))?, compile(&source, s!("main"))?))
    })
    .as_ref()
    .map_err(|e| CaptureError::Os(e.clone()))
}

fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    // SAFETY: a blob exposes `GetBufferSize` bytes at `GetBufferPointer` while alive.
    unsafe { std::slice::from_raw_parts(blob.GetBufferPointer().cast(), blob.GetBufferSize()) }
}

fn compile(source: &str, entry: windows::core::PCSTR) -> Result<Vec<u8>, String> {
    let mut code = None;
    let mut errors = None;
    // SAFETY: the source is a live string of the given length; out-pointers are valid.
    let compiled = unsafe {
        D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            s!("tonemap"),
            None,
            None,
            entry,
            s!("cs_5_0"),
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &raw mut code,
            Some(&raw mut errors),
        )
    };
    match (compiled, code) {
        (Ok(()), Some(code)) => Ok(blob_bytes(&code).to_vec()),
        (result, _) => Err(format!(
            "D3DCompile: {} {}",
            result.err().map(|e| e.to_string()).unwrap_or_default(),
            errors
                .map(|e| String::from_utf8_lossy(blob_bytes(&e)).into_owned())
                .unwrap_or_default()
        )),
    }
}

/// Compiles the shader in the background, so that the first HDR capture does not wait for it.
pub fn warm() {
    std::thread::spawn(|| {
        let _ = bytecode();
    });
}

#[derive(Clone)]
struct CardToneMap {
    device: ID3D11Device,
    peak: ID3D11ComputeShader,
    shader: ID3D11ComputeShader,
    params: ID3D11Buffer,
    /// The brightest pixel of the picture being mapped (one `uint`: the bits of a float).
    brightest: ID3D11UnorderedAccessView,
}

thread_local! {
    /// The shader of the last device used on this thread.
    static CARD: RefCell<Option<CardToneMap>> = const { RefCell::new(None) };
}

fn texture(
    device: &ID3D11Device,
    (width, height): (u32, u32),
    format: DXGI_FORMAT,
    bind: u32,
) -> Result<ID3D11Texture2D, CaptureError> {
    let staging = bind == 0;
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: if staging {
            D3D11_USAGE_STAGING
        } else {
            D3D11_USAGE_DEFAULT
        },
        BindFlags: bind,
        CPUAccessFlags: if staging {
            D3D11_CPU_ACCESS_READ.0 as u32
        } else {
            0
        },
        MiscFlags: 0,
    };
    let mut out = None;
    // SAFETY: `desc` describes a plain texture; the out-pointer is valid.
    unsafe { device.CreateTexture2D(&desc, None, Some(&raw mut out)) }
        .map_err(os("CreateTexture2D"))?;
    out.ok_or_else(|| CaptureError::Os("no texture".into()))
}

impl CardToneMap {
    fn new(device: &ID3D11Device) -> Result<Self, CaptureError> {
        let (peak_code, main_code) = bytecode()?;
        let (mut peak, mut shader) = (None, None);
        // SAFETY: bytecode compiled for cs_5_0; the out-pointers are valid.
        unsafe {
            device
                .CreateComputeShader(peak_code, None, Some(&raw mut peak))
                .map_err(os("CreateComputeShader"))?;
            device
                .CreateComputeShader(main_code, None, Some(&raw mut shader))
                .map_err(os("CreateComputeShader"))?;
        }
        let desc = D3D11_BUFFER_DESC {
            ByteWidth: 16,
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            ..Default::default()
        };
        let mut params = None;
        // SAFETY: a 16-byte constant buffer; the out-pointer is valid.
        unsafe { device.CreateBuffer(&desc, None, Some(&raw mut params)) }
            .map_err(os("CreateBuffer"))?;
        let desc = D3D11_BUFFER_DESC {
            ByteWidth: 4,
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_UNORDERED_ACCESS.0 as u32,
            MiscFlags: D3D11_RESOURCE_MISC_BUFFER_STRUCTURED.0 as u32,
            StructureByteStride: 4,
            ..Default::default()
        };
        let mut buffer = None;
        let mut brightest = None;
        // SAFETY: a one-element structured buffer and a view of all of it; valid out-pointers.
        unsafe {
            device
                .CreateBuffer(&desc, None, Some(&raw mut buffer))
                .map_err(os("CreateBuffer"))?;
            let buffer = buffer.ok_or_else(|| CaptureError::Os("no buffer".into()))?;
            device
                .CreateUnorderedAccessView(&buffer, None, Some(&raw mut brightest))
                .map_err(os("CreateUnorderedAccessView"))?;
        }
        Ok(Self {
            device: device.clone(),
            peak: peak.ok_or_else(|| CaptureError::Os("no shader".into()))?,
            shader: shader.ok_or_else(|| CaptureError::Os("no shader".into()))?,
            params: params.ok_or_else(|| CaptureError::Os("no buffer".into()))?,
            brightest: brightest.ok_or_else(|| CaptureError::Os("no view".into()))?,
        })
    }

    fn of(device: &ID3D11Device) -> Result<Self, CaptureError> {
        CARD.with(|card| {
            let mut card = card.borrow_mut();
            if let Some(made) = card.as_ref().filter(|c| c.device == *device) {
                return Ok(made.clone());
            }
            let made = Self::new(device)?;
            *card = Some(made.clone());
            Ok(made)
        })
    }
}

/// `area` of `source` (scRGB half floats) tone-mapped to SDR in a staging texture of BGRA
/// pixels, queued on the GPU: mapping it waits for the work.
pub(crate) fn stage(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    source: &ID3D11Texture2D,
    area: &PhysicalRect,
    info: &HdrInfo,
) -> Result<ID3D11Texture2D, CaptureError> {
    let card = CardToneMap::of(device)?;
    let size = (area.width, area.height);
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: `desc` is a valid out-pointer.
    unsafe { source.GetDesc(&mut desc) };
    // The source may not be readable by shaders (a capture frame, a shared texture): its area
    // is copied first, a cheap copy on the card.
    let input = texture(
        device,
        size,
        desc.Format,
        D3D11_BIND_SHADER_RESOURCE.0 as u32,
    )?;
    let output = texture(
        device,
        size,
        DXGI_FORMAT_R32_UINT,
        D3D11_BIND_UNORDERED_ACCESS.0 as u32,
    )?;
    let staging = texture(device, size, DXGI_FORMAT_R32_UINT, 0)?;
    let mut view: Option<ID3D11ShaderResourceView> = None;
    let mut target: Option<ID3D11UnorderedAccessView> = None;
    // SAFETY: views of textures of this device, with their own formats.
    unsafe {
        device
            .CreateShaderResourceView(&input, None, Some(&raw mut view))
            .map_err(os("CreateShaderResourceView"))?;
        device
            .CreateUnorderedAccessView(&output, None, Some(&raw mut target))
            .map_err(os("CreateUnorderedAccessView"))?;
    }
    let sdr_white = info.sdr_white_nits.max(1.0);
    let mut params = [0_u8; 16];
    params[..4].copy_from_slice(&(80.0 / sdr_white).to_le_bytes());
    params[4..8].copy_from_slice(&(info.peak_nits / sdr_white).to_le_bytes());
    params[8..12].copy_from_slice(&area.width.to_le_bytes());
    params[12..].copy_from_slice(&area.height.to_le_bytes());
    let from = D3D11_BOX {
        left: area.x as u32,
        top: area.y as u32,
        front: 0,
        right: area.x as u32 + area.width,
        bottom: area.y as u32 + area.height,
        back: 1,
    };
    // SAFETY: every resource belongs to this device; the box lies inside the source (checked by
    // the callers); the views are unbound once dispatched.
    unsafe {
        context.CopySubresourceRegion(&input, 0, 0, 0, 0, source, 0, Some(&from));
        context.UpdateSubresource(&card.params, 0, None, params.as_ptr().cast(), 0, 0);
        context.ClearUnorderedAccessViewUint(&card.brightest, &[0; 4]);
        context.CSSetShaderResources(0, Some(&[view]));
        let views = [target, Some(card.brightest.clone())];
        context.CSSetUnorderedAccessViews(0, 2, Some(views.as_ptr()), None);
        context.CSSetConstantBuffers(0, Some(&[Some(card.params.clone())]));
        let groups = (area.width.div_ceil(GROUP), area.height.div_ceil(GROUP));
        context.CSSetShader(&card.peak, None);
        context.Dispatch(groups.0, groups.1, 1);
        context.CSSetShader(&card.shader, None);
        context.Dispatch(groups.0, groups.1, 1);
        context.CSSetShaderResources(0, Some(&[None]));
        context.CSSetUnorderedAccessViews(0, 2, Some([None, None].as_ptr()), None);
        context.CSSetShader(None::<&ID3D11ComputeShader>, None);
        context.CopyResource(&staging, &output);
    }
    Ok(staging)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wgc::{d3d_device, map};
    use vixeeny_image::tonemap::{ToneMapParams, tonemap_frame};
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R16G16B16A16_FLOAT;

    /// The card gives what the processor's tone mapper gives, to the rounding.
    #[test]
    fn the_card_tone_maps_as_the_processor_does() {
        let Ok((device, context)) = d3d_device(None) else {
            vixeeny_common::skip_test("no Direct3D device");
            return;
        };
        let (w, h) = (67u32, 41u32);
        // Dark, mid, bright, beyond the peak, saturated and wider-gamut colours.
        let rgba: Vec<f32> = (0..w * h)
            .flat_map(|i| {
                let t = i as f32 / (w * h) as f32;
                let v = (t * 12.0).powi(2) * 0.5;
                match i % 5 {
                    0 => [v, v, v, 1.0],
                    1 => [v, v * 0.3, 0.05, 1.0],
                    2 => [-0.2 * v, v, v * 0.9, 1.0],
                    3 => [0.01 * v, 0.02, v * 3.0, 1.0],
                    _ => [t, 1.0 - t, 0.5, 1.0],
                }
            })
            .collect();
        let info = HdrInfo {
            sdr_white_nits: 203.0,
            peak_nits: 1000.0,
        };
        // With highlights, and the same with nothing above SDR white (2.5375 here).
        let dim: Vec<f32> = rgba.iter().map(|&v| v.min(2.5)).collect();
        for rgba in [rgba, dim] {
            let worst = compare(&device, &context, (w, h), &rgba, &info);
            assert!(worst <= 1, "off by {worst}");
        }
    }

    /// The largest difference between the card and the processor on a part of `rgba`.
    fn compare(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        (w, h): (u32, u32),
        rgba: &[f32],
        info: &HdrInfo,
    ) -> u8 {
        let halves: Vec<u16> = rgba
            .iter()
            .map(|&v| vixeeny_common::half::from_f32(v))
            .collect();
        let exact: Vec<f32> = halves
            .iter()
            .map(|&v| vixeeny_common::half::to_f32(v))
            .collect();
        let source = texture(
            device,
            (w, h),
            DXGI_FORMAT_R16G16B16A16_FLOAT,
            D3D11_BIND_SHADER_RESOURCE.0 as u32,
        )
        .unwrap();
        // SAFETY: `halves` holds `h` rows of `w * 8` bytes; same device.
        unsafe { context.UpdateSubresource(&source, 0, None, halves.as_ptr().cast(), w * 8, 0) };
        // A part, as a zone capture asks for: rolled off up to its own brightest pixel.
        let area = PhysicalRect::new(5, 3, w - 9, h - 7);
        let part: Vec<f32> = (0..area.height as usize)
            .flat_map(|y| {
                let at = ((y + area.y as usize) * w as usize + area.x as usize) * 4;
                exact[at..at + area.width as usize * 4].to_vec()
            })
            .collect();
        let expected = tonemap_frame(
            &part,
            &ToneMapParams {
                sdr_white_nits: info.sdr_white_nits,
                peak_nits: info.peak_nits,
            },
        );
        let staging = stage(device, context, &source, &area, info).unwrap();
        let raw = map(context, &staging, area.width, area.height).unwrap();
        let mut worst = 0;
        for y in 0..area.height as usize {
            for x in 0..area.width as usize {
                let got = &raw.data[y * raw.stride + x * 4..][..4];
                let at = (y * area.width as usize + x) * 4;
                for (a, b) in got.iter().zip(&expected[at..at + 4]) {
                    worst = worst.max(a.abs_diff(*b));
                }
            }
        }
        worst
    }
}
