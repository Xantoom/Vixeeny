// SPDX-License-Identifier: GPL-3.0-or-later
//! Dims only the frozen window's back buffer. The shared capture stays untouched for the editor and exports.

use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BLEND_DESC, D3D11_BLEND_INV_SRC_ALPHA, D3D11_BLEND_ONE, D3D11_BLEND_OP_ADD, D3D11_BLEND_SRC_ALPHA,
    D3D11_COLOR_WRITE_ENABLE_ALL, D3D11_RENDER_TARGET_BLEND_DESC, D3D11_VIEWPORT, ID3D11BlendState, ID3D11Device,
    ID3D11DeviceContext, ID3D11PixelShader, ID3D11Texture2D, ID3D11VertexShader,
};
use windows::core::s;

use crate::CaptureError;
use crate::wgc::os;

pub(crate) struct Veil {
    vertex: ID3D11VertexShader,
    pixel: ID3D11PixelShader,
    blend: ID3D11BlendState,
}

impl Veil {
    /// Made with the daemon's device at startup, so neither compilation nor shader creation delays a shortcut.
    pub fn new(device: &ID3D11Device) -> Result<Self, CaptureError> {
        let source = format!(
            r"
float4 vertex(uint id : SV_VertexID) : SV_Position {{
    float2 p = float2((id << 1) & 2, id & 2);
    return float4(p * float2(2, -2) + float2(-1, 1), 0, 1);
}}
float4 pixel() : SV_Target {{ return float4(0, 0, 0, {}); }}
",
            vixeeny_common::look::CAPTURE_DIM
        );
        let vertex_code = vixeeny_platform::d3d::compile(&source, s!("vertex"), s!("vs_4_0"))?;
        let pixel_code = vixeeny_platform::d3d::compile(&source, s!("pixel"), s!("ps_4_0"))?;
        let (mut vertex, mut pixel, mut blend) = (None, None, None);
        let mut desc = D3D11_BLEND_DESC::default();
        desc.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
            BlendEnable: true.into(),
            SrcBlend: D3D11_BLEND_SRC_ALPHA,
            DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
            BlendOp: D3D11_BLEND_OP_ADD,
            SrcBlendAlpha: D3D11_BLEND_ONE,
            DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
            BlendOpAlpha: D3D11_BLEND_OP_ADD,
            RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
        };
        // SAFETY: compiled bytecode, a valid blend description and live out-pointers.
        unsafe {
            device.CreateVertexShader(&vertex_code, None, Some(&raw mut vertex)).map_err(os("veil vertex"))?;
            device.CreatePixelShader(&pixel_code, None, Some(&raw mut pixel)).map_err(os("veil pixel"))?;
            device.CreateBlendState(&desc, Some(&raw mut blend)).map_err(os("veil blend"))?;
        }
        Ok(Self {
            vertex: vertex.ok_or_else(|| CaptureError::Os("no veil vertex shader".into()))?,
            pixel: pixel.ok_or_else(|| CaptureError::Os("no veil pixel shader".into()))?,
            blend: blend.ok_or_else(|| CaptureError::Os("no veil blend state".into()))?,
        })
    }

    pub fn draw(
        &self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        back: &ID3D11Texture2D,
        width: u32,
        height: u32,
    ) -> Result<(), CaptureError> {
        let mut target = None;
        // SAFETY: a swap-chain buffer supporting render-target views; a valid out-pointer.
        unsafe { device.CreateRenderTargetView(back, None, Some(&raw mut target)) }.map_err(os("veil target"))?;
        let target = target.ok_or_else(|| CaptureError::Os("no veil render target".into()))?;
        let viewport =
            D3D11_VIEWPORT { Width: width as f32, Height: height as f32, MaxDepth: 1.0, ..Default::default() };
        // SAFETY: this thread owns the immediate context. A full-screen triangle needs no vertex buffer; only the
        // back buffer is bound for writing, never the shared original. Unbind it before Present or another copy.
        unsafe {
            context.IASetInputLayout(None);
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&self.vertex, None);
            context.PSSetShader(&self.pixel, None);
            context.RSSetViewports(Some(&[viewport]));
            context.OMSetBlendState(&self.blend, None, u32::MAX);
            context.OMSetRenderTargets(Some(&[Some(target)]), None);
            context.Draw(3, 0);
            context.OMSetRenderTargets(None, None);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_WARP;
    use windows::Win32::Graphics::Direct3D11::{D3D11_BIND_RENDER_TARGET, D3D11_CREATE_DEVICE_FLAG};
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT};

    use vixeeny_platform::d3d::{device, read_staging, staging_for, texture};

    /// Use the software D3D device: no monitor is captured and this also checks the shaders on machines without a GPU.
    #[test]
    fn the_first_frame_is_dimmed_without_changing_the_exported_pixels() {
        let (device, context) = device(None, D3D_DRIVER_TYPE_WARP, D3D11_CREATE_DEVICE_FLAG(0)).unwrap();
        let veil = Veil::new(&device).unwrap();
        let (width, height) = (7, 5);
        let original = [200, 150, 100, 255].repeat((width * height) as usize);
        let bind = D3D11_BIND_RENDER_TARGET.0 as u32;
        let source =
            texture(&device, (width, height), DXGI_FORMAT_B8G8R8A8_UNORM, bind, 0, Some((&original, width * 4)))
                .unwrap();
        let back = texture(&device, (width, height), DXGI_FORMAT_B8G8R8A8_UNORM, bind, 0, None).unwrap();
        // SAFETY: same-sized textures on this device in the same format, as in the freeze's presentation path.
        unsafe { context.CopyResource(&back, &source) };
        veil.draw(&device, &context, &back, width, height).unwrap();
        let read = |texture: &ID3D11Texture2D, bytes_per_pixel| {
            let staging = staging_for(&device, texture, None).unwrap();
            // SAFETY: `staging_for` makes an identical staging texture.
            unsafe { context.CopyResource(&staging, texture) };
            read_staging(&context, &staging, height, |rows, stride| {
                (0..height as usize)
                    .flat_map(|y| rows[y * stride..y * stride + width as usize * bytes_per_pixel].iter().copied())
                    .collect::<Vec<u8>>()
            })
            .unwrap()
        };
        for pixel in read(&back, 4).as_chunks::<4>().0 {
            for (value, expected) in pixel.iter().zip([120, 90, 60, 255]) {
                assert!(value.abs_diff(expected) <= 1, "{pixel:?}");
            }
        }
        assert_eq!(read(&source, 4), original, "the shared/exported texture must stay undimmed");

        // HDR values beyond SDR white remain above white after dimming, with no clipping or 8-bit conversion.
        let hdr_pixel = [0x4400u16, 0x4000, 0x3c00, 0x3c00]; // 4, 2, 1, 1 as half floats
        let hdr_bytes = hdr_pixel.into_iter().flat_map(u16::to_le_bytes).collect::<Vec<_>>();
        let hdr = texture(
            &device,
            (width, height),
            DXGI_FORMAT_R16G16B16A16_FLOAT,
            bind,
            0,
            Some((&hdr_bytes.repeat((width * height) as usize), width * 8)),
        )
        .unwrap();
        veil.draw(&device, &context, &hdr, width, height).unwrap();
        let values = crate::wgc::halves_to_f32(&read(&hdr, 8), width as usize * 8, width as usize, height as usize);
        for pixel in values.as_chunks::<4>().0 {
            for (value, expected) in pixel.iter().zip([2.4, 1.2, 0.6, 1.0]) {
                assert!((value - expected).abs() < 0.003, "{pixel:?}");
            }
        }
    }
}
