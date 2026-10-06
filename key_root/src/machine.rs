//! The machine identifier. It is a PUBLIC device label: every local user can read `/etc/machine-id`. It is used only as associated
//! data (a binding context that makes a wrapper refuse to open on another machine) — never as key material, never as entropy.
pub trait MachineContext {
    fn id(&self) -> String;
}

/// What the legacy code used when `/etc/machine-id` cannot be read (containers, macOS): kept so that files written then still open.
const LEGACY_FALLBACK_MACHINE_ID: &str = "YANDI_FALLBACK_MACHINE_ID";

pub struct SystemMachine;

impl MachineContext for SystemMachine {
    fn id(&self) -> String {
        std::fs::read_to_string("/etc/machine-id")
            .map(|s| s.trim().to_owned())
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| LEGACY_FALLBACK_MACHINE_ID.to_owned())
    }
}

/// A fixed identifier, for tests and for tools that act on behalf of another machine.
pub struct FixedMachine(pub String);

impl MachineContext for FixedMachine {
    fn id(&self) -> String {
        self.0.clone()
    }
}
