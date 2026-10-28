//! ASUS ATK ACPI calls through the \\.\ATKACPI device, the route G-Helper uses (the WMI route returned nothing on
//! this Vivobook in Phase 0). The writes are the battery charge limit and the fan mode.
use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows::Win32::System::IO::DeviceIoControl;

const CONTROL_CODE: u32 = 0x0022_240C;
const DSTS: u32 = 0x5354_5344;
const DEVS: u32 = 0x5356_4544;
pub const CPU_FAN: u32 = 0x0011_0013;
pub const GPU_FAN: u32 = 0x0011_0014;
pub const CHARGE_LIMIT: u32 = 0x0012_0057;
/// The VivoBook fan mode, the one Fn+F cycles. This model has no custom fan curves (PLAN.md, Phase 9).
pub const FAN_MODE: u32 = 0x0011_0019;

/// The one model the firmware writes (fan mode, charge limit) were tested on. On any other model the first write needs
/// the user's OK, because another ASUS firmware may read the same values differently.
pub const TESTED_MODEL: &str = "M3500QC";

/// The laptop's model from the BIOS, for example "Vivobook_ASUSLaptop M3500QC_M3500QC". Empty when Windows does not say.
pub fn model() -> String {
  use winreg::{enums::HKEY_LOCAL_MACHINE, RegKey};
  RegKey::predef(HKEY_LOCAL_MACHINE)
    .open_subkey(r"HARDWARE\DESCRIPTION\System\BIOS")
    .and_then(|k| k.get_value::<String, _>("SystemProductName"))
    .map(|m| m.trim().to_string())
    .unwrap_or_default()
}

pub fn is_tested(model: &str) -> bool {
  model.to_uppercase().contains(TESTED_MODEL)
}

/// Fan mode names and the firmware's values on this Vivobook, measured in Phase 9 (not G-Helper's ROG order).
pub const FAN_MODES: [(&str, u32); 3] = [("silent", 1), ("standard", 0), ("performance", 2)];

pub struct Atk(HANDLE);
unsafe impl Send for Atk {}
unsafe impl Sync for Atk {}

impl Atk {
  pub fn open() -> Option<Atk> {
    unsafe {
      CreateFileW(w!(r"\\.\ATKACPI"), (GENERIC_READ | GENERIC_WRITE).0, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, None)
        .ok()
        .map(Atk)
    }
  }

  /// Raw DSTS status for a device id, or None when the call fails.
  pub fn get(&self, device: u32) -> Option<u32> {
    let mut input = [0u8; 16];
    input[0..4].copy_from_slice(&DSTS.to_le_bytes());
    input[4..8].copy_from_slice(&8u32.to_le_bytes());
    input[8..12].copy_from_slice(&device.to_le_bytes());
    let mut out = [0u8; 16];
    let mut got = 0u32;
    unsafe {
      DeviceIoControl(self.0, CONTROL_CODE, Some(input.as_ptr() as *const _), 16, Some(out.as_mut_ptr() as *mut _), 16, Some(&mut got), None).ok()?;
    }
    (got >= 4).then(|| u32::from_le_bytes([out[0], out[1], out[2], out[3]]))
  }

  /// Sets a device value through DEVS. Returns the firmware's result word (1 means accepted on ASUS firmware).
  pub fn set(&self, device: u32, value: u32) -> Option<u32> {
    let mut input = [0u8; 16];
    input[0..4].copy_from_slice(&DEVS.to_le_bytes());
    input[4..8].copy_from_slice(&8u32.to_le_bytes());
    input[8..12].copy_from_slice(&device.to_le_bytes());
    input[12..16].copy_from_slice(&value.to_le_bytes());
    let mut out = [0u8; 16];
    let mut got = 0u32;
    unsafe {
      DeviceIoControl(self.0, CONTROL_CODE, Some(input.as_ptr() as *const _), 16, Some(out.as_mut_ptr() as *mut _), 16, Some(&mut got), None).ok()?;
    }
    (got >= 4).then(|| u32::from_le_bytes([out[0], out[1], out[2], out[3]]))
  }

  /// The active fan mode's name, or None when the firmware does not offer it.
  pub fn fan_mode(&self) -> Option<&'static str> {
    let raw = self.get(FAN_MODE)?;
    if raw & 0x0001_0000 == 0 {
      return None;
    }
    FAN_MODES.iter().find(|(_, v)| *v == raw & 0xFFFF).map(|(n, _)| *n)
  }

  /// Sets the fan mode by name. ASUS firmware answers 1 when it takes a value.
  pub fn set_fan_mode(&self, name: &str) -> Result<(), String> {
    let (_, v) = FAN_MODES.iter().find(|(n, _)| *n == name).ok_or_else(|| format!("Unknown fan mode {name}."))?;
    match self.set(FAN_MODE, *v) {
      Some(1) => Ok(()),
      Some(r) => Err(format!("The firmware refused the fan mode (it answered {r}).")),
      None => Err("The ASUS control interface did not answer.".into()),
    }
  }

  /// Fan speed in RPM. The firmware reports hundreds of RPM in the low word, with bit 16 set when the device exists.
  pub fn fan_rpm(&self, device: u32) -> Option<u32> {
    let raw = self.get(device)?;
    let hundreds = raw & 0xFFFF;
    (raw & 0x0001_0000 != 0 && hundreds <= 100).then_some(hundreds * 100)
  }
}

impl Drop for Atk {
  fn drop(&mut self) {
    unsafe {
      let _ = CloseHandle(self.0);
    }
  }
}

#[cfg(test)]
mod tests {
  #[test]
  fn tested_model() {
    assert!(super::is_tested("Vivobook_ASUSLaptop M3500QC_M3500QC"));
    assert!(super::is_tested("vivobook m3500qc"));
    assert!(!super::is_tested("Vivobook_ASUSLaptop X1505VA_X1505VA"));
    assert!(!super::is_tested(""));
  }
}
