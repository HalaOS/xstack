use std::sync::Once;

use rasi_mio::{net::register_mio_network, timer::register_mio_timer};

/// prepare test enviroment.
pub(crate) fn setup() {
    static INIT: Once = Once::new();

    INIT.call_once(|| {
        register_mio_network();
        register_mio_timer();
    });
}

pub mod transport;
