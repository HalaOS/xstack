use std::collections::VecDeque;

use multiaddr::Multiaddr;

use crate::AutoNAT;

#[derive(Default)]
pub(super) struct MutableSwitch {
    laddrs: Vec<Multiaddr>,
    max_observed_addrs_len: usize,
    observed_addrs: VecDeque<Multiaddr>,
    nat_addrs: Vec<Multiaddr>,
    nat: AutoNAT,
}

impl MutableSwitch {
    pub(super) fn new(max_observed_addrs_len: usize) -> Self {
        Self {
            max_observed_addrs_len,
            ..Default::default()
        }
    }
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
        for addr in addrs {
            if self.observed_addrs.len() == self.max_observed_addrs_len {
                self.observed_addrs.pop_front();
            }

            self.observed_addrs.push_back(addr);
        }
    }

    pub(super) fn observed_addrs(&self) -> Vec<Multiaddr> {
        self.observed_addrs.iter().cloned().collect()
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
