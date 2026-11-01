//! What Windows and the Mac do not share. On Windows this holds only the message below. On the Mac it also gives the
//! Windows-only standard library calls that other modules use a harmless stand-in, so their lines stay as they are.
//! Mac stand-ins refuse rather than guess: anything that deletes, closes or changes something does nothing until the
//! Mac version of it is written and tested (Phases 1 to 5 of the Mac plan).

pub const NOT_ON_MAC: &str = "This is not in the Mac version yet.";

#[cfg(not(windows))]
pub use mac::*;

#[cfg(not(windows))]
mod mac {
  /// `creation_flags` hides a console window, which a Mac program does not open.
  pub trait CommandExt {
    fn creation_flags(&mut self, flags: u32) -> &mut Self;
  }

  impl CommandExt for std::process::Command {
    fn creation_flags(&mut self, _flags: u32) -> &mut Self {
      self
    }
  }

  /// A Mac file has none of Windows' attribute bits. Hidden files are not told apart by this, so nothing that deletes may
  /// rely on it: `store::really_at` and `store::delete_exact` refuse everything on the Mac for now.
  pub trait MetadataExt {
    fn file_attributes(&self) -> u32;
  }

  impl MetadataExt for std::fs::Metadata {
    fn file_attributes(&self) -> u32 {
      0
    }
  }
}
