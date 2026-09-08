# Hardware

The `hardware` module is the backend service for system facts. It collects
processor, graphics and memory data from Linux sources and writes the
snapshot to `sys.fico` (Fish Config format) on every daemon start. There is
no query API yet on purpose: socket access and the client library are
roadmap items.

## Sources

| Fact | Basis source | Detailed source (`detailed=true`) |
|---|---|---|
| CPU name, vendor, cores, threads, MHz | `/proc/cpuinfo` | Same |
| RAM size | `/proc/meminfo` (`MemTotal`) | Same |
| RAM type (DDR4/DDR5) | `Unknown` | `dmidecode -t memory` |
| RAM modules (speed, vendor, part, slots) | Omitted | `dmidecode -t memory` |
| GPU ids, VRAM | DRM sysfs (`/sys/class/drm/cardN/device`) | Same |
| GPU name, driver, PCI slot | Omitted | `uevent` + `lspci -s <slot>` |

Missing tools or permissions degrade to `Unknown` or omitted keys, never to
an error. `dmidecode` needs root, which the system daemon has.

## API

### Types

```rust
pub enum RamType {
  Ddr4,
  Ddr5,
  Unknown,
}
```

| Variant | fico value | Meaning |
|---|---|---|
| `Ddr4` | `DDR4` | DDR4 detected via DMI type 17 |
| `Ddr5` | `DDR5` | DDR5 detected via DMI type 17 |
| `Unknown` | `unknown` | Anything else, mixed kits, or undetectable |

```rust
pub struct CpuInfo {
  pub name: Option<String>,
  pub vendor: Option<String>,
  pub cores: Option<u32>,
  pub threads: Option<u32>,
  pub mhz: Option<u64>,
}
```

```rust
pub struct GpuInfo {
  pub name: Option<String>,
  pub vendor_id: Option<String>,
  pub device_id: Option<String>,
  pub vram_mb: Option<u64>,
  pub pci_slot: Option<String>,
  pub driver: Option<String>,
  pub subsystem: Option<String>,
}
```

`vram_mb` is `None` for shared-memory GPUs or when the driver exposes no
total. `pci_slot`, `driver` and `subsystem` are detailed only.

```rust
pub struct RamInfo {
  pub total_mb: u64,
  pub total_gb: f64,
  pub ram_type: RamType,
  pub slots_used: Option<u32>,
  pub slots_total: Option<u32>,
  pub modules: Vec<RamModule>,
}
```

```rust
pub struct HardwareInfo {
  pub cpu: CpuInfo,
  pub gpus: Vec<GpuInfo>,
  pub ram: RamInfo,
  pub detailed: bool,
}
```

### Functions

```rust
pub fn collect(detailed: bool) -> HardwareInfo;
```

Collects a snapshot. `detailed` enables `dmidecode` and `lspci`. Never
fails, missing sources become `None`/`Unknown`.

```rust
pub fn write_sys_fico(path: &Path, detailed: bool) -> io::Result<HardwareInfo>;
```

Collects and writes the file, creating parent directories. Returns the
snapshot that was written. Returns `Err` only on real I/O errors.

## File Format

`sys.fico` sections are `processor`, `gpu0`..`gpuN` and `ram`. Unknown
fields are omitted, never written as `null`:

```text
processor {
    name: "AMD Ryzen 5 5600X 6-Core Processor"
    vendor: AMD
    cores: 12
    threads: 12
    mhz: 3700
}
gpu0 {
    name: "Advanced Micro Devices, Inc. [AMD/ATI] Navi 23 [Radeon RX 6600] (rev c1)"
    vendor_id: 0x1002
    device_id: 0x73ff
    vram_mb: 8192
    pci_slot: "0000:01:00.0"
    driver: amdgpu
}
ram {
    total_mb: 32577
    total_gb: 31.81
    type: DDR5
    slots_used: 2
    slots_total: 4
    module0 {
        locator: "DIMM 0"
        size_mb: 16384
        speed_mts: 4800
        manufacturer: Samsung
        part_number: M323R2GA3BB0-CQKOL
    }
}
```

## Usage / Example

```rust
use settings_daemon::hardware;
use std::path::Path;

// Daemon startup: detailed snapshot, refreshed every start.
let info = hardware::write_sys_fico(Path::new("/Library/Preferences/SystemConfiguration/sys.fico"), true)?;
println!("RAM: {} GB {}", info.ram.total_gb, info.ram.ram_type.as_str());
```

## Cross References

- [Daemon.md](Daemon.md) – startup refresh wiring
- [Socket.md](Socket.md) – future query dispatch for these facts
