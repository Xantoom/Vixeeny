// SPDX-License-Identifier: GPL-3.0-or-later
//! HDR screens made SDR on the graphics card: a compute shader runs the tone mapper of
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
    D3D11_BUFFER_DESC, D3D11_CPU_ACCESS_READ, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING, ID3D11Buffer, ID3D11ComputeShader, ID3D11Device, ID3D11DeviceContext,
    ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11UnorderedAccessView,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_R32_UINT, DXGI_SAMPLE_DESC};
use windows::core::s;

use crate::CaptureError;
use crate::wgc::os;

/// `tonemap_pixel`, line for line; the result is packed as BGRA bytes, rounded as the processor
/// rounds them.
const SHADER: &str = r"
Texture2D<float4> src : register(t0);
RWTexture2D<uint> dst : register(u0);
cbuffer Params : register(b0) { float scale; float headroom; uint2 size; };

static const float KNEE = 0.8;

float luminance(float3 c) { return 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b; }

float3 into_gamut(float3 c) {
    float y = luminance(c);
    if (y <= 0.0) return 0.0;
    float low = min(c.r, min(c.g, c.b));
    if (low >= 0.0) return c;
    float t = y / (y - low);
    return max(y + (c - y) * t, 0.0);
}

float roll_off(float v, float h) {
    if (v <= KNEE) return max(v, 0.0);
    float span = 1.0 - KNEE;
    float x = (v - KNEE) / span;
    float w = max((max(h, 1.0) - KNEE) / span, 1.0);
    float g = x * (1.0 + x / (w * w)) / (1.0 + x);
    return KNEE + span * min(g, 1.0);
}

float srgb(float v) {
    v = saturate(v);
    return v <= 0.0031308 ? 12.92 * v : 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}

uint q(float v) { return (uint)(v * 255.0 + 0.5); }

[numthreads(16, 16, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= size.x || id.y >= size.y) return;
    float3 c = into_gamut(src[id.xy].rgb * scale);
    float y = luminance(c);
    float3 o = 0.0;
    if (y > 1.1920929e-7) {
        o = c * (roll_off(y, headroom) / y);
        float m = max(o.r, max(o.g, o.b));
        if (m > 1.0) o /= m;
        o = float3(srgb(o.r), srgb(o.g), srgb(o.b));
    }
    dst[id.xy] = q(o.b) | (q(o.g) << 8) | (q(o.r) << 16) | 0xff000000u;
}
";

const GROUP: u32 = 16;

/// The compiled shader, made once per process.
fn bytecode() -> Result<&'static [u8], CaptureError> {
    static CODE: OnceLock<Result<Vec<u8>, String>> = OnceLock::new();
    CODE.get_or_init(compile)
        .as_deref()
        .map_err(|e| CaptureError::Os(e.clone()))
}

fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    // SAFETY: a blob exposes `GetBufferSize` bytes at `GetBufferPointer` while alive.
    unsafe { std::slice::from_raw_parts(blob.GetBufferPointer().cast(), blob.GetBufferSize()) }
}

fn compile() -> Result<Vec<u8>, String> {
    let mut code = None;
    let mut errors = None;
    // SAFETY: the source is a live string of the given length; out-pointers are valid.
    let compiled = unsafe {
        D3DCompile(
            SHADER.as_ptr().cast(),
            SHADER.len(),
            s!("tonemap"),
            None,
            None,
            s!("main"),
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
    shader: ID3D11ComputeShader,
    params: ID3D11Buffer,
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
        let mut shader = None;
        // SAFETY: bytecode compiled for cs_5_0; the out-pointer is valid.
        unsafe { device.CreateComputeShader(bytecode()?, None, Some(&raw mut shader)) }
            .map_err(os("CreateComputeShader"))?;
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
        Ok(Self {
            device: device.clone(),
            shader: shader.ok_or_else(|| CaptureError::Os("no shader".into()))?,
            params: params.ok_or_else(|| CaptureError::Os("no buffer".into()))?,
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
        context.CSSetShader(&card.shader, None);
        context.CSSetShaderResources(0, Some(&[view]));
        context.CSSetUnorderedAccessViews(0, 1, Some(&target), None);
        context.CSSetConstantBuffers(0, Some(&[Some(card.params.clone())]));
        context.Dispatch(area.width.div_ceil(GROUP), area.height.div_ceil(GROUP), 1);
        context.CSSetShaderResources(0, Some(&[None]));
        context.CSSetUnorderedAccessViews(0, 1, Some(&None), None);
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
        let halves: Vec<u16> = rgba
            .iter()
            .map(|&v| vixeeny_common::half::from_f32(v))
            .collect();
        let exact: Vec<f32> = halves
            .iter()
            .map(|&v| vixeeny_common::half::to_f32(v))
            .collect();
        let info = HdrInfo {
            sdr_white_nits: 203.0,
            peak_nits: 1000.0,
        };
        let expected = tonemap_frame(
            &exact,
            &ToneMapParams {
                sdr_white_nits: info.sdr_white_nits,
                peak_nits: info.peak_nits,
            },
        );
        let source = texture(
            &device,
            (w, h),
            DXGI_FORMAT_R16G16B16A16_FLOAT,
            D3D11_BIND_SHADER_RESOURCE.0 as u32,
        )
        .unwrap();
        // SAFETY: `halves` holds `h` rows of `w * 8` bytes; same device.
        unsafe { context.UpdateSubresource(&source, 0, None, halves.as_ptr().cast(), w * 8, 0) };
        // A part, as a zone capture asks for.
        let area = PhysicalRect::new(5, 3, w - 9, h - 7);
        let staging = stage(&device, &context, &source, &area, &info).unwrap();
        let raw = map(&context, &staging, area.width, area.height).unwrap();
        let mut worst = 0;
        for y in 0..area.height as usize {
            for x in 0..area.width as usize {
                let got = &raw.data[y * raw.stride + x * 4..][..4];
                let at = ((y + area.y as usize) * w as usize + x + area.x as usize) * 4;
                let want = &expected[at..at + 4];
                for (a, b) in got.iter().zip(want) {
                    worst = worst.max(a.abs_diff(*b));
                }
            }
        }
        assert!(worst <= 1, "off by {worst}");
    }
}
