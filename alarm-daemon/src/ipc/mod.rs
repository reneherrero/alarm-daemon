//! D-Bus IPC surface.
//!
//! Contains the exported bus/object/interface constants and the `Control`
//! service object. Method implementations live in `control.rs`.

mod control;
pub use control::Control;

/// Well-known D-Bus bus name for this daemon.
pub const BUS_NAME: &str = "org.helm.AlarmDaemon";
/// Object path where the control interface is served.
pub const OBJECT_PATH: &str = "/org/helm/AlarmDaemon";
/// Fully qualified D-Bus interface name for control operations.
pub const CONTROL_INTERFACE: &str = "org.helm.AlarmDaemon.Control";
