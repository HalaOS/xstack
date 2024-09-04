use multiaddr::Multiaddr;

use crate::event::{Event, EventArgument, EventMediator, EventSource};

use super::AutoNAT;

#[derive(Default)]
pub(super) struct MutableSwitch {
    laddrs: Vec<Multiaddr>,
    nat_addrs: Vec<Multiaddr>,
    event_mediator: EventMediator,
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
        if self.nat == AutoNAT::NAT {
            self.nat_addrs.clone()
        } else {
            self.laddrs.clone()
        }
    }

    pub(super) fn set_net_addrs(&mut self, addrs: Vec<Multiaddr>) {
        self.nat_addrs = addrs;
    }

    pub(super) fn notify(&mut self, arg: EventArgument) {
        self.event_mediator.notify(arg)
    }

    pub(super) fn new_listener<E: Event>(&mut self, buffer: usize) -> EventSource<E> {
        self.event_mediator.new_listener(buffer)
    }

    pub(super) fn auto_nat(&self) -> AutoNAT {
        self.nat
    }

    pub(super) fn set_nat(&mut self, state: AutoNAT) {
        if self.nat != state {
            self.notify(EventArgument::AutoNAT(state));
        }

        self.nat = state;
    }
}
