//! ASUS ATK ACPI calls through the \\.\ATKACPI device, the route G-Helper uses (the WMI route returned nothing on
//! this Vivobook in Phase 0). The only write is the battery charge limit.
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
