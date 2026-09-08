use std::io;
use std::path::Path;

use sdk::FishFile::{FishDocument, FishValue};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// RAM module type. Only DDR4 and DDR5 are classified, everything else
/// (including undetectable) is `Unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RamType {
  Ddr4,
  Ddr5,
  Unknown,
}

impl RamType {
  pub fn as_str(&self) -> &'static str {
    match self {
      Self::Ddr4 => "DDR4",
      Self::Ddr5 => "DDR5",
      Self::Unknown => "unknown",
    }
  }

  pub fn from_dmidecode(s: &str) -> Self {
    match s.trim().to_uppercase().as_str() {
      "DDR4" => Self::Ddr4,
      "DDR5" => Self::Ddr5,
      _ => Self::Unknown,
    }
  }
}

/// Processor facts. Every field is optional because no source is guaranteed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CpuInfo {
  pub name: Option<String>,
  pub vendor: Option<String>,
  pub cores: Option<u32>,
  pub threads: Option<u32>,
  pub mhz: Option<u64>,
}

/// Graphics device facts. `vram_mb` is `None` for shared-memory GPUs or when
/// the driver does not expose a total.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GpuInfo {
  pub name: Option<String>,
  pub vendor_id: Option<String>,
  pub device_id: Option<String>,
  pub vram_mb: Option<u64>,
  /// Detailed only: PCI slot (`0000:01:00.0`), kernel driver, subsystem ids.
  pub pci_slot: Option<String>,
  pub driver: Option<String>,
  pub subsystem: Option<String>,
}

/// One physical RAM module. Only filled when `detailed` collection succeeds.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RamModule {
  pub locator: Option<String>,
  pub size_mb: Option<u64>,
  pub speed_mts: Option<u64>,
  pub manufacturer: Option<String>,
  pub part_number: Option<String>,
}

/// Memory facts. `ram_type` is DDR4/DDR5 only, `Unknown` otherwise.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RamInfo {
  pub total_mb: u64,
  pub total_gb: f64,
  pub ram_type: RamType,
  pub slots_used: Option<u32>,
  pub slots_total: Option<u32>,
  /// Detailed only: per-module facts.
  pub modules: Vec<RamModule>,
}

impl Default for RamInfo {
  fn default() -> Self {
    Self {
      total_mb: 0,
      total_gb: 0.0,
      ram_type: RamType::Unknown,
      slots_used: None,
      slots_total: None,
      modules: Vec::new(),
    }
  }
}

/// Full hardware snapshot written to `sys.fico`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HardwareInfo {
  pub cpu: CpuInfo,
  pub gpus: Vec<GpuInfo>,
  pub ram: RamInfo,
  /// True when privileged/detailed sources were attempted.
  pub detailed: bool,
}

// ---------------------------------------------------------------------------
// Collection entry points
// ---------------------------------------------------------------------------

/// Collect a hardware snapshot.
///
/// `detailed` enables slow or privileged sources (`dmidecode`, `lspci`,
/// per-module SPD data). Missing sources yield `None`/`Unknown`, never an
/// error.
pub fn collect(detailed: bool) -> HardwareInfo {
  HardwareInfo {
    cpu: collect_cpu(),
    gpus: collect_gpus(detailed),
    ram: collect_ram(detailed),
    detailed,
  }
}

/// Collect and write `sys.fico` to `path`, creating parent directories.
/// Returns the snapshot that was written. Failing sources degrade to
/// `Unknown`, only real I/O errors return `Err`.
pub fn write_sys_fico(path: &Path, detailed: bool) -> io::Result<HardwareInfo> {
  let info = collect(detailed);
  let doc = info.to_fico();
  if let Some(parent) = path.parent() {
    std::fs::create_dir_all(parent)?;
  }
  doc
    .write_to_file(path)
    .map_err(|e| io::Error::other(e.to_string()))?;
  Ok(info)
}

// ---------------------------------------------------------------------------
// CPU: /proc/cpuinfo
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
fn collect_cpu() -> CpuInfo {
  let raw = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
  parse_cpuinfo(&raw)
}

#[cfg(not(target_os = "linux"))]
fn collect_cpu() -> CpuInfo {
  CpuInfo::default()
}

fn parse_cpuinfo(raw: &str) -> CpuInfo {
  let mut info = CpuInfo::default();
  let mut threads: u32 = 0;
  let mut physical_ids = std::collections::HashSet::new();
  let mut cores_per_package: Option<u32> = None;

  for block in raw.split("\n\n") {
    if !block.contains("processor") {
      continue;
    }
    threads += 1;
    for line in block.lines() {
      let (key, value) = match line.split_once(':') {
        Some(pair) => (pair.0.trim(), pair.1.trim()),
        None => continue,
      };
      match key {
        "model name" if info.name.is_none() => info.name = Some(value.to_string()),
        "vendor_id" if info.vendor.is_none() => {
          info.vendor = Some(match value {
            "GenuineIntel" => "Intel".to_string(),
            "AuthenticAMD" => "AMD".to_string(),
            other => other.to_string(),
          })
        }
        "cpu MHz" if info.mhz.is_none() => {
          if let Ok(mhz) = value.parse::<f64>() {
            info.mhz = Some(mhz.round() as u64);
          }
        }
        "cpu cores" if cores_per_package.is_none() => {
          cores_per_package = value.parse::<u32>().ok();
        }
        "physical id" => {
          physical_ids.insert(value.to_string());
        }
        _ => {}
      }
    }
  }

  if threads > 0 {
    info.threads = Some(threads);
    let packages = physical_ids.len().max(1) as u32;
    info.cores = cores_per_package
      .map(|c| c * packages)
      .or(Some(threads));
  }
  info
}

// ---------------------------------------------------------------------------
// RAM: /proc/meminfo + dmidecode (detailed)
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
fn collect_ram(detailed: bool) -> RamInfo {
  let mut info = RamInfo::default();
  if let Ok(raw) = std::fs::read_to_string("/proc/meminfo") {
    info.total_mb = parse_memtotal_kb(&raw) / 1024;
    info.total_gb = (info.total_mb as f64 / 1024.0 * 100.0).round() / 100.0;
  }
  if detailed {
    if let Some(dmi) = run_dmidecode_memory() {
      let parsed = parse_dmidecode_memory(&dmi);
      info.ram_type = parsed.ram_type;
      info.slots_used = Some(parsed.modules.len() as u32);
      info.slots_total = Some(parsed.slots_total.max(parsed.modules.len() as u32));
      info.modules = parsed.modules;
    }
  }
  info
}

#[cfg(not(target_os = "linux"))]
fn collect_ram(_detailed: bool) -> RamInfo {
  RamInfo::default()
}

/// Parse `MemTotal:  32577204 kB` from meminfo, returns kB (0 when missing).
fn parse_memtotal_kb(raw: &str) -> u64 {
  for line in raw.lines() {
    if let Some(rest) = line.strip_prefix("MemTotal:") {
      if let Some(num) = rest.split_whitespace().next() {
        return num.parse::<u64>().unwrap_or(0);
      }
    }
  }
  0
}

#[cfg(target_os = "linux")]
fn run_dmidecode_memory() -> Option<String> {
  let output = std::process::Command::new("dmidecode")
    .args(["-t", "memory"])
    .output()
    .ok()?;
  if !output.status.success() {
    return None;
  }
  String::from_utf8(output.stdout).ok()
}

struct DmiMemory {
  ram_type: RamType,
  slots_total: u32,
  modules: Vec<RamModule>,
}

/// Parse `dmidecode -t memory`. Only populated DIMMs (Size != `No Module
/// Installed`) become modules. Common type wins, mixed/unknown stays Unknown.
fn parse_dmidecode_memory(raw: &str) -> DmiMemory {
  let mut slots_total: u32 = 0;
  let mut modules: Vec<RamModule> = Vec::new();
  let mut current: RamModule = RamModule::default();
  let mut current_type = RamType::Unknown;
  let mut current_size_mb: u64 = 0;
  let mut in_device = false;
  let mut type_votes: Vec<RamType> = Vec::new();

  let flush = |modules: &mut Vec<RamModule>,
               current: &mut RamModule,
               current_type: &mut RamType,
               current_size_mb: &mut u64,
               type_votes: &mut Vec<RamType>| {
    if *current_size_mb > 0 {
      current.size_mb = Some(*current_size_mb);
      modules.push(std::mem::take(current));
      if *current_type != RamType::Unknown {
        type_votes.push(*current_type);
      }
    }
    *current = RamModule::default();
    *current_type = RamType::Unknown;
    *current_size_mb = 0;
  };

  for line in raw.lines() {
    if line.starts_with("Memory Device") {
      if in_device {
        flush(&mut modules, &mut current, &mut current_type, &mut current_size_mb, &mut type_votes);
      }
      in_device = true;
      slots_total += 1;
      continue;
    }
    if !in_device {
      continue;
    }
    let trimmed = line.trim();
    if let Some(value) = trimmed.strip_prefix("Size:") {
      let value = value.trim();
      if value == "No Module Installed" {
        current_size_mb = 0;
      } else {
        // `8192 MB` or `16 GB`
        let mut parts = value.split_whitespace();
        let num: u64 = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        current_size_mb = match parts.next().unwrap_or("MB") {
          "GB" => num * 1024,
          "TB" => num * 1024 * 1024,
          _ => num,
        };
      }
    } else if let Some(value) = trimmed.strip_prefix("Type:") {
      current_type = RamType::from_dmidecode(value);
    } else if let Some(value) = trimmed.strip_prefix("Locator:") {
      let value = value.trim();
      if !value.is_empty() && value != "Unknown" && value != "None" {
        current.locator = Some(value.to_string());
      }
    } else if let Some(value) = trimmed.strip_prefix("Speed:") {
      current.speed_mts = parse_speed_mts(value);
    } else if let Some(value) = trimmed.strip_prefix("Configured Memory Speed:") {
      if let Some(mts) = parse_speed_mts(value) {
        current.speed_mts = Some(mts);
      }
    } else if let Some(value) = trimmed.strip_prefix("Manufacturer:") {
      let value = value.trim();
      if !value.is_empty() && value != "Unknown" {
        current.manufacturer = Some(value.to_string());
      }
    } else if let Some(value) = trimmed.strip_prefix("Part Number:") {
      let value = value.trim();
      if !value.is_empty() && value != "Unknown" {
        current.part_number = Some(value.to_string());
      }
    }
  }
  if in_device {
    flush(&mut modules, &mut current, &mut current_type, &mut current_size_mb, &mut type_votes);
  }

  let ram_type = if type_votes.is_empty() {
    RamType::Unknown
  } else if type_votes.iter().all(|t| *t == type_votes[0]) {
    type_votes[0]
  } else {
    RamType::Unknown
  };
  DmiMemory {
    ram_type,
    slots_total,
    modules,
  }
}

/// Parse `3200 MT/s` / `Unknown` speed fields.
fn parse_speed_mts(raw: &str) -> Option<u64> {
  let raw = raw.trim();
  if raw.eq_ignore_ascii_case("unknown") || raw.is_empty() {
    return None;
  }
  raw
    .split_whitespace()
    .next()?
    .parse::<u64>()
    .ok()
    .filter(|v| *v > 0)
}

// ---------------------------------------------------------------------------
// GPU: DRM sysfs + lspci (detailed)
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
fn collect_gpus(detailed: bool) -> Vec<GpuInfo> {
  let mut gpus = Vec::new();
  let drm = match std::fs::read_dir("/sys/class/drm") {
    Ok(entries) => entries,
    Err(_) => return gpus,
  };
  let mut cards: Vec<String> = drm
    .filter_map(|e| e.ok())
    .map(|e| e.file_name().to_string_lossy().into_owned())
    .filter(|n| {
      n.starts_with("card")
        && n["card".len()..].chars().next().is_some_and(|c| c.is_ascii_digit())
        && !n.contains('-')
    })
    .collect();
  cards.sort();

  for card in cards {
    let base = format!("/sys/class/drm/{}/device", card);
    let mut gpu = GpuInfo::default();
    gpu.vendor_id = read_trimmed(format!("{}/vendor", base));
    gpu.device_id = read_trimmed(format!("{}/device", base));
    gpu.subsystem = read_trimmed(format!("{}/subsystem_device", base));
    gpu.vram_mb = read_vram_mb(&base);
    if detailed {
      let uevent = std::fs::read_to_string(format!("{}/uevent", base)).unwrap_or_default();
      let parsed = parse_uevent(&uevent);
      gpu.driver = parsed.driver;
      gpu.pci_slot = parsed.pci_slot.clone();
      gpu.name = parsed
        .pci_slot
        .as_deref()
        .and_then(run_lspci_name)
        .filter(|n| !n.is_empty());
    }
    gpus.push(gpu);
  }
  gpus
}

#[cfg(not(target_os = "linux"))]
fn collect_gpus(_detailed: bool) -> Vec<GpuInfo> {
  Vec::new()
}

#[cfg(target_os = "linux")]
fn read_trimmed(path: String) -> Option<String> {
  std::fs::read_to_string(path)
    .ok()
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
}

/// VRAM total in MB from driver sysfs (`mem_info_vram_total`, bytes).
/// Returns `None` for shared-memory GPUs or unexposed totals.
#[cfg(target_os = "linux")]
fn read_vram_mb(device_base: &str) -> Option<u64> {
  let raw = std::fs::read_to_string(format!("{}/mem_info_vram_total", device_base)).ok()?;
  raw
    .trim()
    .parse::<u64>()
    .ok()
    .filter(|v| *v > 0)
    .map(|bytes| bytes / 1024 / 1024)
}

struct Uevent {
  driver: Option<String>,
  pci_slot: Option<String>,
}

/// Parse `/sys/.../device/uevent` (`DRIVER=amdgpu`, `PCI_SLOT_NAME=0000:01:00.0`).
fn parse_uevent(raw: &str) -> Uevent {
  let mut out = Uevent {
    driver: None,
    pci_slot: None,
  };
  for line in raw.lines() {
    if let Some(value) = line.strip_prefix("DRIVER=") {
      if out.driver.is_none() {
        out.driver = Some(value.trim().to_string());
      }
    } else if let Some(value) = line.strip_prefix("PCI_SLOT_NAME=") {
      if out.pci_slot.is_none() {
        out.pci_slot = Some(value.trim().to_string());
      }
    }
  }
  out
}

/// Best-effort GPU name via `lspci -s <slot>`. Returns `None` when lspci is
/// missing or the slot is unknown.
#[cfg(target_os = "linux")]
fn run_lspci_name(slot: &str) -> Option<String> {
  let output = std::process::Command::new("lspci")
    .args(["-s", slot])
    .output()
    .ok()?;
  if !output.status.success() {
    return None;
  }
  parse_lspci_line(&String::from_utf8_lossy(&output.stdout))
}

/// Parse `01:00.0 VGA compatible controller: NVIDIA Corporation GA106 ...`.
fn parse_lspci_line(line: &str) -> Option<String> {
  let line = line.trim();
  if line.is_empty() {
    return None;
  }
  line.split_once(": ").map(|(_, name)| name.trim().to_string())
}

// ---------------------------------------------------------------------------
// fico serialization
// ---------------------------------------------------------------------------

impl HardwareInfo {
  /// Serialize the snapshot to a `FishDocument`:
  /// `processor {}`, `gpu0 {}..gpuN {}`, `ram {}`.
  pub fn to_fico(&self) -> FishDocument {
    let mut doc = FishDocument::new();
    let cpu = &self.cpu;
    set_opt(&mut doc, "processor.name", cpu.name.as_deref());
    set_opt(&mut doc, "processor.vendor", cpu.vendor.as_deref());
    set_opt_u64(&mut doc, "processor.cores", cpu.cores.map(u64::from));
    set_opt_u64(&mut doc, "processor.threads", cpu.threads.map(u64::from));
    set_opt_u64(&mut doc, "processor.mhz", cpu.mhz);

    for (index, gpu) in self.gpus.iter().enumerate() {
      let section = format!("gpu{}", index);
      set_opt(&mut doc, &format!("{}.name", section), gpu.name.as_deref());
      set_opt(&mut doc, &format!("{}.vendor_id", section), gpu.vendor_id.as_deref());
      set_opt(&mut doc, &format!("{}.device_id", section), gpu.device_id.as_deref());
      set_opt_u64(&mut doc, &format!("{}.vram_mb", section), gpu.vram_mb);
      set_opt(&mut doc, &format!("{}.pci_slot", section), gpu.pci_slot.as_deref());
      set_opt(&mut doc, &format!("{}.driver", section), gpu.driver.as_deref());
      set_opt(&mut doc, &format!("{}.subsystem", section), gpu.subsystem.as_deref());
    }

    let ram = &self.ram;
    doc.set("ram.total_mb", ram.total_mb as i64);
    doc.set("ram.total_gb", ram.total_gb);
    doc.set("ram.type", ram.ram_type.as_str());
    set_opt_u64(&mut doc, "ram.slots_used", ram.slots_used.map(u64::from));
    set_opt_u64(&mut doc, "ram.slots_total", ram.slots_total.map(u64::from));
    for (index, module) in ram.modules.iter().enumerate() {
      let section = format!("ram.module{}", index);
      set_opt(&mut doc, &format!("{}.locator", section), module.locator.as_deref());
      set_opt_u64(&mut doc, &format!("{}.size_mb", section), module.size_mb);
      set_opt_u64(&mut doc, &format!("{}.speed_mts", section), module.speed_mts);
      set_opt(&mut doc, &format!("{}.manufacturer", section), module.manufacturer.as_deref());
      set_opt(&mut doc, &format!("{}.part_number", section), module.part_number.as_deref());
    }
    doc
  }
}

fn set_opt(doc: &mut FishDocument, path: &str, value: Option<&str>) {
  if let Some(value) = value {
    doc.set(path, FishValue::from(value));
  }
}

fn set_opt_u64(doc: &mut FishDocument, path: &str, value: Option<u64>) {
  if let Some(value) = value {
    doc.set(path, FishValue::from(value.min(i64::MAX as u64) as i64));
  }
}

// ---------------------------------------------------------------------------
// Tests (fixture-based, no hardware required)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
  use super::*;

  const CPUINFO: &str = "processor\t: 0\nvendor_id\t: AuthenticAMD\ncpu family\t: 25\ncpu MHz\t\t: 3593.265\ncache size\t: 512 KB\nphysical id\t: 0\ncpu cores\t: 4\n\nprocessor\t: 1\nvendor_id\t: AuthenticAMD\ncpu family\t: 25\ncpu MHz\t\t: 3593.265\nphysical id\t: 0\ncpu cores\t: 4\n\nprocessor\t: 2\nvendor_id\t: AuthenticAMD\nphysical id\t: 1\ncpu cores\t: 4\nmodel name\t: AMD Ryzen 5 5600X 6-Core Processor\n";

  #[test]
  fn parse_cpuinfo_counts_threads_and_packages() {
    let cpu = parse_cpuinfo(CPUINFO);
    assert_eq!(cpu.threads, Some(3));
    // 4 cores x 2 packages.
    assert_eq!(cpu.cores, Some(8));
    assert_eq!(cpu.vendor.as_deref(), Some("AMD"));
    assert_eq!(cpu.mhz, Some(3593));
    assert_eq!(cpu.name.as_deref(), Some("AMD Ryzen 5 5600X 6-Core Processor"));
  }

  #[test]
  fn parse_cpuinfo_empty_is_default() {
    let cpu = parse_cpuinfo("");
    assert_eq!(cpu.threads, None);
    assert_eq!(cpu.cores, None);
  }

  #[test]
  fn parse_memtotal() {
    let raw = "MemTotal:        32577204 kB\nMemFree:          1234 kB\n";
    assert_eq!(parse_memtotal_kb(raw), 32577204);
    assert_eq!(parse_memtotal_kb(""), 0);
  }

  const DMIDECODE: &str = "# dmidecode 3.4\nHandle 0x0010, DMI type 17, 84 bytes\nMemory Device\n\tSize: 16 GB\n\tLocator: DIMM 0\n\tType: DDR5\n\tSpeed: 4800 MT/s\n\tConfigured Memory Speed: 4800 MT/s\n\tManufacturer: Samsung\n\tPart Number: M323R2GA3BB0-CQKOL\nHandle 0x0011, DMI type 17, 84 bytes\nMemory Device\n\tSize: No Module Installed\n\tLocator: DIMM 1\n\tType: Unknown\n\tSpeed: Unknown\n\tManufacturer: Unknown\nHandle 0x0012, DMI type 17, 84 bytes\nMemory Device\n\tSize: 16777216 kB\n\tLocator: DIMM 2\n\tType: DDR5\n\tSpeed: Unknown\n\tConfigured Memory Speed: 5200 MT/s\n\tManufacturer: Micron\n\tPart Number: CT16G52C42U5.M8A1\n";

  #[test]
  fn parse_dmidecode_memory_modules() {
    let dmi = parse_dmidecode_memory(DMIDECODE);
    assert_eq!(dmi.ram_type, RamType::Ddr5);
    assert_eq!(dmi.slots_total, 3);
    assert_eq!(dmi.modules.len(), 2);
    assert_eq!(dmi.modules[0].size_mb, Some(16384));
    assert_eq!(dmi.modules[0].speed_mts, Some(4800));
    assert_eq!(dmi.modules[0].manufacturer.as_deref(), Some("Samsung"));
    assert_eq!(dmi.modules[1].speed_mts, Some(5200));
    assert_eq!(dmi.modules[1].locator.as_deref(), Some("DIMM 2"));
  }

  #[test]
  fn ram_type_only_ddr4_ddr5() {
    assert_eq!(RamType::from_dmidecode("DDR4"), RamType::Ddr4);
    assert_eq!(RamType::from_dmidecode("ddr5"), RamType::Ddr5);
    assert_eq!(RamType::from_dmidecode("LPDDR5"), RamType::Unknown);
    assert_eq!(RamType::from_dmidecode("Unknown"), RamType::Unknown);
  }

  #[test]
  fn parse_uevent_and_lspci() {
    let uevent = parse_uevent("DRIVER=amdgpu\nPCI_CLASS=30000\nPCI_SLOT_NAME=0000:01:00.0\n");
    assert_eq!(uevent.driver.as_deref(), Some("amdgpu"));
    assert_eq!(uevent.pci_slot.as_deref(), Some("0000:01:00.0"));
    let name = parse_lspci_line("01:00.0 VGA compatible controller: Advanced Micro Devices, Inc. [AMD/ATI] Navi 23 [Radeon RX 6600] (rev c1)\n");
    assert!(name.unwrap().contains("Radeon RX 6600"));
    assert_eq!(parse_lspci_line(""), None);
  }

  #[test]
  fn to_fico_roundtrip() {
    let mut info = HardwareInfo::default();
    info.cpu.name = Some("Test CPU".to_string());
    info.cpu.cores = Some(8);
    info.ram.total_mb = 32768;
    info.ram.total_gb = 32.0;
    info.ram.ram_type = RamType::Ddr4;
    let doc = info.to_fico();
    assert_eq!(doc.get("processor.name").and_then(|v| v.as_str()), Some("Test CPU"));
    assert_eq!(doc.get("ram.type").and_then(|v| v.as_str()), Some("DDR4"));
    assert_eq!(doc.get("ram.total_gb").and_then(|v| v.as_f64()), Some(32.0));
    // Unknown fields are omitted, not null.
    assert_eq!(doc.get("processor.vendor"), None);
    // Serialized text parses back.
    let reparsed = FishDocument::parse(&doc.to_string()).unwrap();
    assert_eq!(reparsed.get("ram.type").and_then(|v| v.as_str()), Some("DDR4"));
  }
}
