use multiaddr::Multiaddr;

use crate::AutoNAT;

#[derive(Default)]
pub(super) struct MutableSwitch {
    laddrs: Vec<Multiaddr>,
    observed_addrs: Vec<Multiaddr>,
    nat_addrs: Vec<Multiaddr>,
    nat: AutoNAT,
}

impl MutableSwitch {
    /// Register transport bind addresses.
    pub(super) fn transport_bind_to(&mut self, addr: Multiaddr) {
        self.laddrs.push(addr)
    }

    /// Returns the local bound addrs.
    pub(super) fn local_addrs(&self) -> Vec<Multiaddr> {
        self.laddrs.clone()
    }

    pub(super) fn listen_addrs(&self) -> Vec<Multiaddr> {
        match self.nat {
            AutoNAT::Public => self.laddrs.clone(),
            AutoNAT::NAT => self.nat_addrs.clone(),
            AutoNAT::Unknown => vec![],
        }
    }

    pub(super) fn set_observed_addrs(&mut self, addrs: Vec<Multiaddr>) {
        self.observed_addrs = addrs;
    }

    pub(super) fn observed_addrs(&self) -> Vec<Multiaddr> {
        self.observed_addrs.clone()
    }

    pub(super) fn set_net_addrs(&mut self, addrs: Vec<Multiaddr>) {
        self.nat_addrs = addrs;
    }

    pub(super) fn auto_nat(&self) -> AutoNAT {
        self.nat
    }

    pub(super) fn set_nat(&mut self, state: AutoNAT) {
        self.nat = state;
    }
}
