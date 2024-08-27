use std::sync::OnceLock;

use crate::Switch;

static GLOBAL_SWITCH: OnceLock<Switch> = OnceLock::new();

/// Register one `Switch` as a global instance.
pub fn register_switch(switch: Switch) {
    if GLOBAL_SWITCH.set(switch).is_err() {
        panic!("Call register_switch twice.")
    }
}

/// Returns the registered global `Switch` instance.
///
/// Without calling [`register_switch`] before, this fn will raise a panic.
pub fn global_switch() -> &'static Switch {
    GLOBAL_SWITCH
        .get()
        .expect("Call register_switch to register one first.")
}
