// SPDX-License-Identifier: GPL-3.0-or-later
//! Direct3D 11 helpers shared by the shaders of the capture and of the encoder.

use windows::Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile};
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING, ID3D11Device, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC};
use windows::core::{PCSTR, s};

fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    // SAFETY: a blob exposes `GetBufferSize` bytes at `GetBufferPointer` while alive.
    unsafe { std::slice::from_raw_parts(blob.GetBufferPointer().cast(), blob.GetBufferSize()) }
}

/// The bytecode of the function `entry` of the HLSL `source`, for `target` (`cs_5_0`…).
pub fn compile(source: &str, entry: PCSTR, target: PCSTR) -> Result<Vec<u8>, String> {
    let mut code = None;
    let mut errors = None;
    // SAFETY: the source is a live string of the given length; out-pointers are valid.
    let compiled = unsafe {
        D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            s!("vixeeny"),
            None,
            None,
            entry,
            target,
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

/// A texture of `size` in `format`, for `bind` (none: a staging texture the processor reads),
/// with the `misc` flags, filled from `init` (its bytes, and the length of a row) when given.
pub fn texture(
    device: &ID3D11Device,
    (width, height): (u32, u32),
    format: DXGI_FORMAT,
    bind: u32,
    misc: u32,
    init: Option<(&[u8], u32)>,
) -> Result<ID3D11Texture2D, String> {
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
        MiscFlags: misc,
    };
    if init.is_some_and(|(bytes, pitch)| bytes.len() < pitch as usize * height as usize) {
        return Err("CreateTexture2D: too few bytes".into());
    }
    let init = init.map(|(bytes, pitch)| D3D11_SUBRESOURCE_DATA {
        pSysMem: bytes.as_ptr().cast(),
        SysMemPitch: pitch,
        SysMemSlicePitch: 0,
    });
    let mut out = None;
    // SAFETY: `desc` describes a plain texture; `init` points at `height` rows of `pitch` bytes,
    // which outlive the call; the out-pointer is valid.
    unsafe {
        device.CreateTexture2D(
            &raw const desc,
            init.as_ref().map(|i| i as *const _),
            Some(&raw mut out),
        )
    }
    .map_err(|e| format!("CreateTexture2D: {e}"))?;
    out.ok_or_else(|| "CreateTexture2D: no texture".into())
}
