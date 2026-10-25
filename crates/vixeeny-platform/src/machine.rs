// SPDX-License-Identifier: GPL-3.0-or-later
//! What this computer is made of, for the settings' general page: processor, memory, graphics adapters, disks and
//! screens. Each part is read on its own; one that cannot be read is left out rather than failing the rest.

use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_MODE_INFO,
    DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME,
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS};
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_FEATURE_ARCHITECTURE, D3D12_FEATURE_DATA_ARCHITECTURE, D3D12CreateDevice, ID3D12Device,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_DESC1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIAdapter1, IDXGIFactory1,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, GetDiskFreeSpaceExW, GetDriveTypeW,
    GetLogicalDrives, GetVolumeInformationW, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{
    DISK_GEOMETRY_EX, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, IOCTL_STORAGE_GET_DEVICE_NUMBER, IOCTL_STORAGE_QUERY_PROPERTY,
    PropertyStandardQuery, STORAGE_DEVICE_DESCRIPTOR, STORAGE_DEVICE_NUMBER, STORAGE_PROPERTY_QUERY,
    StorageDeviceProperty,
};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW};
use windows::Win32::System::SystemInformation::{
    GetLogicalProcessorInformationEx, GetPhysicallyInstalledSystemMemory, GetSystemFirmwareTable, GlobalMemoryStatusEx,
    MEMORYSTATUSEX, RSMB, RelationProcessorCore,
};
use windows::Win32::System::WindowsProgramming::DRIVE_FIXED;
use windows::core::{HSTRING, PCWSTR, w};

use vixeeny_common::machine::{Cpu, Disk, Gpu, Machine, Module, Screen};

/// Reads the parts of this computer (a few tens of milliseconds: call it off the UI thread).
pub fn machine() -> Machine {
    Machine { cpu: cpu(), ram_bytes: ram_bytes(), modules: modules(), gpus: gpus(), disks: disks(), screens: screens() }
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
    let (base, boost) = frequencies();
    Some(Cpu { name, cores: cores().unwrap_or(threads), threads, base_mhz: base, boost_mhz: boost })
}

/// `(base, boost)` in MHz. Windows gives the base; only Intel processors tell their maximum (CPUID leaf 0x16), AMD
/// keeps it in registers a program cannot read.
fn frequencies() -> (u32, u32) {
    let mut mhz = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: `mhz` holds `size` bytes; RRF_RT_REG_DWORD checks the type.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0"),
            w!("~MHz"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut mhz).cast()),
            Some(&raw mut size),
        )
    };
    let base = if status == ERROR_SUCCESS { mhz } else { 0 };
    #[cfg(target_arch = "x86_64")]
    {
        use std::arch::x86_64::__cpuid;
        let vendor = __cpuid(0);
        let intel = (vendor.ebx, vendor.edx, vendor.ecx) == (0x756e_6547, 0x4965_6e69, 0x6c65_746e);
        if intel && vendor.eax >= 0x16 {
            // Within the leaves the processor reports.
            let leaf = __cpuid(0x16);
            let (b, max) = (leaf.eax & 0xffff, leaf.ebx & 0xffff);
            return (if b > 0 { b } else { base }, max);
        }
    }
    (base, 0)
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
    unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, Some(buffer.as_mut_ptr().cast()), &raw mut size) }
        .ok()?;
    let bytes = buffer.as_ptr().cast::<u8>();
    let (mut offset, mut count) = (0usize, 0u32);
    while offset + 8 <= size as usize {
        // SAFETY: each record starts with its relationship and its size (both u32), inside the `size` bytes the call
        // filled.
        let record = unsafe { bytes.add(offset).cast::<[u32; 2]>().read_unaligned() };
        if record[1] == 0 {
            break;
        }
        count += 1;
        offset += record[1] as usize;
    }
    (count > 0).then_some(count)
}

/// The installed memory, in bytes (0 when it cannot be read).
pub fn ram_bytes() -> u64 {
    let mut kib = 0u64;
    // SAFETY: a valid out-pointer.
    match unsafe { GetPhysicallyInstalledSystemMemory(&raw mut kib) } {
        Ok(()) => kib * 1024,
        Err(_) => 0,
    }
}

/// The memory sticks, from the SMBIOS tables of the firmware (type 17: memory device).
fn modules() -> Vec<Module> {
    // SAFETY: a first call without a buffer asks for the size.
    let size = unsafe { GetSystemFirmwareTable(RSMB, 0, None) };
    if size == 0 {
        return Vec::new();
    }
    let mut raw = vec![0u8; size as usize];
    // SAFETY: `raw` holds `size` bytes.
    if unsafe { GetSystemFirmwareTable(RSMB, 0, Some(&mut raw)) } != size {
        return Vec::new();
    }
    // RawSMBIOSData: 4 bytes of version, the length of the tables, the tables.
    let Some(len) = raw.get(4..8).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])) else {
        return Vec::new();
    };
    let tables = &raw[8..raw.len().min(8 + len as usize)];
    smbios_modules(tables)
}

fn smbios_modules(tables: &[u8]) -> Vec<Module> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 4 <= tables.len() {
        let kind = tables[at];
        let length = tables[at + 1] as usize;
        if length < 4 || at + length > tables.len() {
            break;
        }
        let formatted = &tables[at..at + length];
        // The strings follow, ended by an empty one.
        let mut end = at + length;
        while end + 1 < tables.len() && (tables[end] != 0 || tables[end + 1] != 0) {
            end += 1;
        }
        let strings: Vec<&[u8]> = tables[at + length..end].split(|b| *b == 0).collect();
        let string = |index: u8| {
            (index > 0)
                .then(|| strings.get(index as usize - 1))
                .flatten()
                .map(|s| String::from_utf8_lossy(s).trim().to_owned())
                .unwrap_or_default()
        };
        let word = |o: usize| formatted.get(o..o + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
        let dword = |o: usize| formatted.get(o..o + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        match kind {
            17 => {
                let size = word(0x0c);
                let bytes = match size {
                    0 | 0xffff => 0,
                    0x7fff => u64::from(dword(0x1c) & 0x7fff_ffff) << 20,
                    s if s & 0x8000 != 0 => u64::from(s & 0x7fff) << 10,
                    s => u64::from(s) << 20,
                };
                // An empty slot.
                if bytes > 0 {
                    let kind = match formatted.get(0x12) {
                        Some(0x18) => "DDR3",
                        Some(0x1a) => "DDR4",
                        Some(0x1b) => "LPDDR",
                        Some(0x1d) => "LPDDR3",
                        Some(0x1e) => "LPDDR4",
                        Some(0x22) => "DDR5",
                        Some(0x23) => "LPDDR5",
                        _ => "",
                    };
                    let rated = match word(0x15) {
                        0xffff => dword(0x54),
                        s => u32::from(s),
                    };
                    let configured = match word(0x20) {
                        0xffff => dword(0x58),
                        s => u32::from(s),
                    };
                    let part = formatted.get(0x1a).map_or_else(String::new, |i| string(*i));
                    out.push(Module {
                        bytes,
                        kind: kind.to_owned(),
                        mts: if configured > 0 { configured } else { rated },
                        cl: latency(&part),
                    });
                }
            }
            127 => break,
            _ => {}
        }
        at = end + 2;
    }
    out
}

/// The CAS latency the maker writes in its part number after the speed: `F5-6000J3038F16G` (G.Skill),
/// `CMK32GX5M2B6000C30` or `…Z30` (Corsair), `KF560C30-16` (Kingston), `FF3D532G6000HC38A` (TeamGroup). 0 when the
/// number does not say (sticks sold at the standard timings).
fn latency(part: &str) -> u32 {
    let b = part.as_bytes();
    for i in 0..b.len() {
        // 3 or 4 digits of speed, a letter or two, 2 digits of latency.
        let digits = b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
        if !(3..=4).contains(&digits) || (i > 0 && b[i - 1].is_ascii_digit()) {
            continue;
        }
        let rest = &b[i + digits..];
        let skip = match rest {
            [b'H', b'C', ..] | [b'C', b'L', ..] => 2,
            [b'C' | b'J' | b'U' | b'Z', ..] => 1,
            _ => continue,
        };
        let cl = &rest[skip..];
        if cl.len() >= 2 && cl[0].is_ascii_digit() && cl[1].is_ascii_digit() {
            let n = u32::from(cl[0] - b'0') * 10 + u32::from(cl[1] - b'0');
            if (9..=60).contains(&n) {
                return n;
            }
        }
    }
    0
}

/// The memory free right now, in bytes (`None` when it cannot be read).
pub fn free_ram_bytes() -> Option<u64> {
    let mut status = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    // SAFETY: `status` is a valid structure with its length set.
    unsafe { GlobalMemoryStatusEx(&raw mut status) }.ok()?;
    Some(status.ullAvailPhys)
}

fn gpus() -> Vec<Gpu> {
    // SAFETY: plain DXGI enumeration; every interface is released when dropped.
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut seen = Vec::new();
    let mut index = 0;
    // SAFETY: as above.
    while let Ok(adapter) = unsafe { factory.EnumAdapters1(index) } {
        index += 1;
        // SAFETY: as above.
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else {
            continue;
        };
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 || !first_sight(&mut seen, &desc) {
            continue;
        }
        let len = desc.Description.iter().position(|c| *c == 0).unwrap_or(desc.Description.len());
        let integrated = shares_memory(&adapter).unwrap_or(false);
        out.push(Gpu {
            name: String::from_utf16_lossy(&desc.Description[..len]),
            integrated,
            memory_bytes: if integrated { 0 } else { desc.DedicatedVideoMemory as u64 },
        });
    }
    // The graphics card first, the processor's graphics after.
    out.sort_by_key(|g| g.integrated);
    out
}

/// Whether this adapter is a card not met yet in the enumeration. On some multi-GPU PCs Windows lists the card driving
/// the desktop twice, at times under another LUID: the same model, board and memory is taken for the same card (two
/// identical cards show as one).
pub(crate) fn first_sight(seen: &mut Vec<(u32, u32, u32, u32, usize)>, desc: &DXGI_ADAPTER_DESC1) -> bool {
    let card = (desc.VendorId, desc.DeviceId, desc.SubSysId, desc.Revision, desc.DedicatedVideoMemory);
    if seen.contains(&card) {
        return false;
    }
    seen.push(card);
    true
}

/// Whether the adapter works in the system memory (Direct3D 12 calls it UMA): the graphics of the processor.
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

/// `(free, total)` bytes of the disk that holds `dir` (or the nearest folder above it that exists), as this user may
/// fill it.
pub fn disk_space(dir: &std::path::Path) -> Option<(u64, u64)> {
    let dir = dir.ancestors().find(|d| d.is_dir())?;
    let path = HSTRING::from(dir.as_os_str());
    let (mut free, mut total) = (0u64, 0u64);
    // SAFETY: a NUL-terminated path and valid out-pointers.
    unsafe { GetDiskFreeSpaceExW(PCWSTR(path.as_ptr()), Some(&raw mut free), Some(&raw mut total), None) }.ok()?;
    Some((free, total))
}

/// The letters of the fixed disks, with their sizes and the model of the disk under them.
fn disks() -> Vec<Disk> {
    // SAFETY: no arguments.
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8).filter(|i| mask & (1 << i) != 0).filter_map(|i| volume(char::from(b'A' + i))).collect()
}

fn volume(letter: char) -> Option<Disk> {
    let root = HSTRING::from(format!(r"{letter}:\"));
    // SAFETY: a NUL-terminated path.
    if unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } != DRIVE_FIXED {
        return None;
    }
    let (mut total, mut free) = (0u64, 0u64);
    // SAFETY: valid out-pointers.
    unsafe { GetDiskFreeSpaceExW(PCWSTR(root.as_ptr()), None, Some(&raw mut total), Some(&raw mut free)) }.ok()?;
    let mut name = [0u16; 261];
    // SAFETY: `name` is a writable buffer; the other outputs are not asked for.
    let label = unsafe { GetVolumeInformationW(PCWSTR(root.as_ptr()), Some(&mut name), None, None, None, None) }
        .map_or_else(
            |_| String::new(),
            |()| {
                let len = name.iter().position(|c| *c == 0).unwrap_or(0);
                String::from_utf16_lossy(&name[..len])
            },
        );
    let model = disk_number(letter).and_then(physical_disk).map(|(model, _)| model).unwrap_or_default();
    Some(Disk { letter: format!("{letter}:"), label, model, bytes: total, free_bytes: free })
}

/// The physical disk a volume is on (`\\.\C:`, opened without read access).
fn disk_number(letter: char) -> Option<u32> {
    let path = HSTRING::from(format!(r"\\.\{letter}:"));
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
    let mut number = STORAGE_DEVICE_NUMBER::default();
    // SAFETY: the output buffer is passed with its size and outlives the call.
    let asked = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_STORAGE_GET_DEVICE_NUMBER,
            None,
            0,
            Some((&raw mut number).cast()),
            size_of::<STORAGE_DEVICE_NUMBER>() as u32,
            None,
            None,
        )
    };
    // SAFETY: the handle was opened above.
    let _ = unsafe { CloseHandle(handle) };
    asked.ok().map(|()| number.DeviceNumber)
}

/// `\\.\PhysicalDriveN`, opened without read access (no administrator rights needed): its model and size.
fn physical_disk(n: u32) -> Option<(String, u64)> {
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
        let head = unsafe { descriptor.as_ptr().cast::<STORAGE_DEVICE_DESCRIPTOR>().read() };
        let text = |offset: u32| {
            let start = offset as usize;
            if start == 0 || start >= raw.len() {
                return String::new();
            }
            let end = raw[start..].iter().position(|b| *b == 0).map_or(raw.len(), |p| start + p);
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
    Some((model, bytes))
}

fn screens() -> Vec<Screen> {
    let (mut paths_len, mut modes_len) = (0u32, 0u32);
    // SAFETY: valid out-pointers.
    let status = unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &raw mut paths_len, &raw mut modes_len) };
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
    // Its `DISPLAY` number, and its output: adapter (low, high) and target.
    type Key = (u32, (u32, i32, u32));
    let mut screens: Vec<(Key, Screen)> = paths
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
            let len = name.monitorFriendlyDeviceName.iter().position(|c| *c == 0).unwrap_or(0);
            // SAFETY: without QDC_VIRTUAL_MODE_AWARE the union holds the index of the mode.
            let index = unsafe { path.sourceInfo.Anonymous.modeInfoIdx } as usize;
            // SAFETY: the mode a source path points to is a source mode.
            let source = modes.get(index).map(|m| unsafe { m.Anonymous.sourceMode })?;
            let rate = target.refreshRate;
            let hz = if rate.Denominator == 0 {
                0
            } else {
                (f64::from(rate.Numerator) / f64::from(rate.Denominator)).round() as u32
            };
            // `\\.\DISPLAY2`: the same number for the outputs of a duplicated screen.
            let mut gdi = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            gdi.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            gdi.header.size = size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
            gdi.header.adapterId = path.sourceInfo.adapterId;
            gdi.header.id = path.sourceInfo.id;
            // SAFETY: `gdi` starts with its header, whose size field is set.
            let _ = unsafe { DisplayConfigGetDeviceInfo(&raw mut gdi.header) };
            let gdi_len = gdi.viewGdiDeviceName.iter().position(|c| *c == 0).unwrap_or(0);
            let gdi_name = String::from_utf16_lossy(&gdi.viewGdiDeviceName[..gdi_len]);
            let gdi = gdi_name.trim_start_matches(|c: char| !c.is_ascii_digit()).parse().unwrap_or(u32::MAX);
            // The display settings number the screens by their output: its adapter, then its target (not the `DISPLAY`
            // number, which can differ).
            let output = (target.adapterId.LowPart, target.adapterId.HighPart, target.id);
            Some((
                (gdi, output),
                Screen {
                    number: 0,
                    primary: source.position.x == 0 && source.position.y == 0,
                    name: String::from_utf16_lossy(&name.monitorFriendlyDeviceName[..len]),
                    width: source.width,
                    height: source.height,
                    hz,
                },
            ))
        })
        .collect();
    // A screen shown on two outputs (duplicated) is one screen, numbered by its first output.
    screens.sort_by_key(|((gdi, output), _)| (*gdi, *output));
    screens.dedup_by_key(|((gdi, _), _)| *gdi);
    screens.sort_by_key(|((_, output), _)| *output);
    screens.into_iter().enumerate().map(|(i, (_, screen))| Screen { number: i as u32 + 1, ..screen }).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn this_computer_has_a_processor_memory_and_a_screen() {
        let m = super::machine();
        println!("{m:#?}");
        let cpu = m.cpu.expect("no processor");
        assert!(!cpu.name.is_empty() && cpu.cores >= 1 && cpu.threads >= cpu.cores);
        assert!(m.ram_bytes > 0);
    }

    #[test]
    fn latencies_read_from_part_numbers() {
        assert_eq!(super::latency("F5-6000J3038F16G"), 30);
        assert_eq!(super::latency("CMK32GX5M2B6000C30"), 30);
        assert_eq!(super::latency("KF560C30-16"), 30);
        assert_eq!(super::latency("CMH32GX5M2B6000Z30"), 30);
        assert_eq!(super::latency("FF3D532G6000HC38ADC01"), 38);
        assert_eq!(super::latency("F4-3600C16-16GTZNC"), 16);
        assert_eq!(super::latency("CP16G4DFRA32A"), 0);
        assert_eq!(super::latency("M425R2GA3BB0-CQKOD"), 0);
    }

    #[test]
    fn memory_sticks_read_from_smbios() {
        // One type 17 structure (DDR5, 32 GB, 6000 MT/s), then the end (type 127).
        let mut t = vec![0u8; 0x5c];
        t[0] = 17;
        t[1] = 0x5c;
        t[0x0c..0x0e].copy_from_slice(&0x7fffu16.to_le_bytes());
        t[0x1c..0x20].copy_from_slice(&32768u32.to_le_bytes());
        t[0x12] = 0x22;
        t[0x15..0x17].copy_from_slice(&4800u16.to_le_bytes());
        t[0x1a] = 1;
        t[0x20..0x22].copy_from_slice(&6000u16.to_le_bytes());
        t.extend_from_slice(b"F5-6000J3038F16G\0\0");
        t.extend_from_slice(&[127, 4, 0, 0, 0, 0]);
        let m = super::smbios_modules(&t);
        assert_eq!(m, [super::Module { bytes: 32 << 30, kind: "DDR5".into(), mts: 6000, cl: 30 }]);
    }

    #[test]
    fn the_free_space_of_a_folder_yet_to_be_made_is_its_disk_s() {
        let dir = std::env::temp_dir().join("vixeeny-not-there").join("deeper");
        let (free, total) = super::disk_space(&dir).expect("the temp disk");
        assert!(total > 0 && free <= total, "{free} / {total}");
    }
}
