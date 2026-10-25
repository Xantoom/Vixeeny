// SPDX-License-Identifier: GPL-3.0-or-later
//! What this computer is made of: processor, memory, graphics adapters, disks and screens. The
//! platform layer reads it, the general page of the settings shows it.

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Machine {
    pub cpu: Option<Cpu>,
    /// Installed memory, in bytes (0: unknown).
    pub ram_bytes: u64,
    /// The memory sticks (empty when the firmware does not list them).
    pub modules: Vec<Module>,
    /// The graphics cards first, then the processor's graphics.
    pub gpus: Vec<Gpu>,
    pub disks: Vec<Disk>,
    pub screens: Vec<Screen>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cpu {
    pub name: String,
    pub cores: u32,
    pub threads: u32,
    /// Base and maximum (boost) frequencies, in MHz (0: unknown).
    pub base_mhz: u32,
    pub boost_mhz: u32,
}

/// A memory stick, as the firmware describes it (SMBIOS).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    pub bytes: u64,
    /// `DDR5`, `DDR4`... (empty when unknown).
    pub kind: String,
    /// The speed it runs at, in MT/s (0: unknown).
    pub mts: u32,
    /// Its CAS latency, read from the maker's part number (0: unknown).
    pub cl: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gpu {
    pub name: String,
    /// Built into the processor (it shares the system memory).
    pub integrated: bool,
    /// Its own memory, in bytes (0 for an integrated one).
    pub memory_bytes: u64,
}

/// A drive letter on a fixed disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disk {
    /// `C:`.
    pub letter: String,
    /// The name given to the volume (often empty).
    pub label: String,
    /// The model of the disk the volume is on (empty when it cannot be read).
    pub model: String,
    pub bytes: u64,
    pub free_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    /// 1, 2, 3... as Windows' display settings number them.
    pub number: u32,
    /// The main screen (the taskbar's, at the origin of the desktop).
    pub primary: bool,
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Refresh rate, in Hz (rounded).
    pub hz: u32,
}
