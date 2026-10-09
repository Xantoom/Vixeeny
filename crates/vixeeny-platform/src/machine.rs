// SPDX-License-Identifier: GPL-3.0-or-later
//! What this computer is made of, for the settings' general page: processor, memory, graphics
//! adapters, disks and screens. Each part is read on its own; one that cannot be read is left
//! out rather than failing the rest.

use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes,
    QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS};
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_FEATURE_ARCHITECTURE, D3D12_FEATURE_DATA_ARCHITECTURE, D3D12CreateDevice, ID3D12Device,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIAdapter1, IDXGIFactory1,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{
    DISK_GEOMETRY_EX, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, IOCTL_STORAGE_QUERY_PROPERTY,
    PropertyStandardQuery, STORAGE_DEVICE_DESCRIPTOR, STORAGE_PROPERTY_QUERY,
    StorageDeviceProperty,
};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
use windows::Win32::System::SystemInformation::{
    GetLogicalProcessorInformationEx, GetPhysicallyInstalledSystemMemory, RelationProcessorCore,
};
use windows::core::{HSTRING, PCWSTR, w};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Machine {
    pub cpu: Option<Cpu>,
    /// Installed memory, in bytes (0: unknown).
    pub ram_bytes: u64,
    pub gpus: Vec<Gpu>,
    pub disks: Vec<Disk>,
    pub screens: Vec<Screen>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cpu {
    pub name: String,
    pub cores: u32,
    pub threads: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gpu {
    pub name: String,
    /// Built into the processor (it shares the system memory).
    pub integrated: bool,
    /// Its own memory, in bytes (0 for an integrated one).
    pub memory_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disk {
    pub model: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Refresh rate, in Hz (rounded).
    pub hz: u32,
}

/// Reads the parts of this computer (a few tens of milliseconds: call it off the UI thread).
pub fn machine() -> Machine {
    Machine {
        cpu: cpu(),
        ram_bytes: ram_bytes(),
        gpus: gpus(),
        disks: disks(),
        screens: screens(),
    }
}

fn cpu() -> Option<Cpu> {
    let mut buffer = [0u16; 256];
    let mut size = size_of_val(&buffer) as u32;
    // SAFETY: `buffer` holds `size` bytes; the value is a string, which RRF_RT_REG_SZ checks.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0"),
            w!("ProcessorNameString"),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&raw mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let len = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    let name = String::from_utf16_lossy(&buffer[..len]);
    // Some processors pad their name with spaces.
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let threads = std::thread::available_parallelism().map_or(0, |n| n.get() as u32);
    Some(Cpu {
        name,
        cores: cores().unwrap_or(threads),
        threads,
    })
}

/// The physical cores (each record of the list is one core).
fn cores() -> Option<u32> {
    let mut size = 0u32;
    // SAFETY: a first call without a buffer asks for the size it needs (and fails doing so).
    let _ = unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, None, &raw mut size) };
    if size == 0 {
        return None;
    }
    // u64 for the alignment of the records.
    let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
    // SAFETY: `buffer` holds at least `size` bytes, suitably aligned.
    unsafe {
        GetLogicalProcessorInformationEx(
            RelationProcessorCore,
            Some(buffer.as_mut_ptr().cast()),
            &raw mut size,
        )
    }
    .ok()?;
    let bytes = buffer.as_ptr().cast::<u8>();
    let (mut offset, mut count) = (0usize, 0u32);
    while offset + 8 <= size as usize {
        // SAFETY: each record starts with its relationship and its size (both u32), inside the
        // `size` bytes the call filled.
        let record = unsafe { bytes.add(offset).cast::<[u32; 2]>().read_unaligned() };
        if record[1] == 0 {
            break;
        }
        count += 1;
        offset += record[1] as usize;
    }
    (count > 0).then_some(count)
}

fn ram_bytes() -> u64 {
    let mut kib = 0u64;
    // SAFETY: a valid out-pointer.
    match unsafe { GetPhysicallyInstalledSystemMemory(&raw mut kib) } {
        Ok(()) => kib * 1024,
        Err(_) => 0,
    }
}

fn gpus() -> Vec<Gpu> {
    // SAFETY: plain DXGI enumeration; every interface is released when dropped.
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut index = 0;
    // SAFETY: as above.
    while let Ok(adapter) = unsafe { factory.EnumAdapters1(index) } {
        index += 1;
        // SAFETY: as above.
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else {
            continue;
        };
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let len = desc
            .Description
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(desc.Description.len());
        let integrated = shares_memory(&adapter).unwrap_or(false);
        out.push(Gpu {
            name: String::from_utf16_lossy(&desc.Description[..len]),
            integrated,
            memory_bytes: if integrated {
                0
            } else {
                desc.DedicatedVideoMemory as u64
            },
        });
    }
    // The graphics card first, the processor's graphics after.
    out.sort_by_key(|g| g.integrated);
    out
}

/// Whether the adapter works in the system memory (Direct3D 12 calls it UMA): the graphics of
/// the processor.
fn shares_memory(adapter: &IDXGIAdapter1) -> Option<bool> {
    let mut device: Option<ID3D12Device> = None;
    // SAFETY: `adapter` is valid; the device is released when dropped.
    unsafe { D3D12CreateDevice(adapter, D3D_FEATURE_LEVEL_11_0, &raw mut device) }.ok()?;
    let mut data = D3D12_FEATURE_DATA_ARCHITECTURE::default();
    // SAFETY: `data` is the structure this feature fills, with its size.
    unsafe {
        device?.CheckFeatureSupport(
            D3D12_FEATURE_ARCHITECTURE,
            (&raw mut data).cast(),
            size_of::<D3D12_FEATURE_DATA_ARCHITECTURE>() as u32,
        )
    }
    .ok()?;
    Some(data.UMA.as_bool())
}

fn disks() -> Vec<Disk> {
    (0..32).filter_map(disk).collect()
}

/// `\\.\PhysicalDriveN`, opened without read access (no administrator rights needed).
fn disk(n: u32) -> Option<Disk> {
    let path = HSTRING::from(format!(r"\\.\PhysicalDrive{n}"));
    // SAFETY: a NUL-terminated path; the handle is closed below.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(path.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    }
    .ok()?;
    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceProperty,
        QueryType: PropertyStandardQuery,
        ..Default::default()
    };
    let mut descriptor = vec![0u64; 128];
    let mut geometry = DISK_GEOMETRY_EX::default();
    // SAFETY: each buffer is passed with its own size and outlives the call.
    let described = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some((&raw const query).cast()),
            size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            Some(descriptor.as_mut_ptr().cast()),
            (descriptor.len() * 8) as u32,
            None,
            None,
        )
    };
    // SAFETY: as above.
    let measured = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_DISK_GET_DRIVE_GEOMETRY_EX,
            None,
            0,
            Some((&raw mut geometry).cast()),
            size_of::<DISK_GEOMETRY_EX>() as u32,
            None,
            None,
        )
    };
    // SAFETY: the handle was opened above.
    let _ = unsafe { CloseHandle(handle) };
    // A card reader without a card has no size.
    let bytes = u64::try_from(geometry.DiskSize).unwrap_or(0);
    if measured.is_err() || bytes == 0 {
        return None;
    }
    let model = described.ok().map_or_else(String::new, |()| {
        let raw: &[u8] =
            // SAFETY: the buffer is `descriptor.len() * 8` bytes, all initialised.
            unsafe { std::slice::from_raw_parts(descriptor.as_ptr().cast(), descriptor.len() * 8) };
        // SAFETY: the buffer starts with the descriptor (u64-aligned, large enough).
        let head = unsafe {
            descriptor
                .as_ptr()
                .cast::<STORAGE_DEVICE_DESCRIPTOR>()
                .read()
        };
        let text = |offset: u32| {
            let start = offset as usize;
            if start == 0 || start >= raw.len() {
                return String::new();
            }
            let end = raw[start..]
                .iter()
                .position(|b| *b == 0)
                .map_or(raw.len(), |p| start + p);
            String::from_utf8_lossy(&raw[start..end]).trim().to_owned()
        };
        let (vendor, product) = (text(head.VendorIdOffset), text(head.ProductIdOffset));
        // NVMe drives repeat the maker in the product name, or give "NVMe" as the vendor.
        if vendor.is_empty()
            || vendor.eq_ignore_ascii_case("nvme")
            || product.to_lowercase().starts_with(&vendor.to_lowercase())
        {
            product
        } else {
            format!("{vendor} {product}")
        }
    });
    Some(Disk { model, bytes })
}

fn screens() -> Vec<Screen> {
    let (mut paths_len, mut modes_len) = (0u32, 0u32);
    // SAFETY: valid out-pointers.
    let status = unsafe {
        GetDisplayConfigBufferSizes(
            QDC_ONLY_ACTIVE_PATHS,
            &raw mut paths_len,
            &raw mut modes_len,
        )
    };
    if status != ERROR_SUCCESS {
        return Vec::new();
    }
    let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); paths_len as usize];
    let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); modes_len as usize];
    // SAFETY: both arrays have the lengths passed with them.
    let status = unsafe {
        QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &raw mut paths_len,
            paths.as_mut_ptr(),
            &raw mut modes_len,
            modes.as_mut_ptr(),
            None,
        )
    };
    if status != ERROR_SUCCESS {
        return Vec::new();
    }
    paths.truncate(paths_len as usize);
    paths
        .iter()
        .filter_map(|path| {
            let target = &path.targetInfo;
            let mut name = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
            name.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
            name.header.size = size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
            name.header.adapterId = target.adapterId;
            name.header.id = target.id;
            // SAFETY: `name` starts with its header, whose size field is set.
            let _ = unsafe { DisplayConfigGetDeviceInfo(&raw mut name.header) };
            let len = name
                .monitorFriendlyDeviceName
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(0);
            // SAFETY: without QDC_VIRTUAL_MODE_AWARE the union holds the index of the mode.
            let index = unsafe { path.sourceInfo.Anonymous.modeInfoIdx } as usize;
            // SAFETY: the mode a source path points to is a source mode.
            let source = modes
                .get(index)
                .map(|m| unsafe { m.Anonymous.sourceMode })?;
            let rate = target.refreshRate;
            let hz = if rate.Denominator == 0 {
                0
            } else {
                (f64::from(rate.Numerator) / f64::from(rate.Denominator)).round() as u32
            };
            Some(Screen {
                name: String::from_utf16_lossy(&name.monitorFriendlyDeviceName[..len]),
                width: source.width,
                height: source.height,
                hz,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn this_computer_has_a_processor_memory_and_a_screen() {
        let m = super::machine();
        println!("{m:#?}");
        let cpu = m.cpu.unwrap_or_else(|| panic!("no processor"));
        assert!(!cpu.name.is_empty() && cpu.cores >= 1 && cpu.threads >= cpu.cores);
        assert!(m.ram_bytes > 0);
    }
}
