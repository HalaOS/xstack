use std::collections::HashMap;

use multiaddr::Multiaddr;

/// A variant for autonat state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AutoNAT {
    /// the node is a public node.
    Public,
    /// the node is behind  nat/firewall.
    Nat,
    /// Cannot determine if the node is behind nat/firewall
    Unknown,
}

#[derive(Default)]
pub(super) struct RawAutoNATState {
    dial_succ: HashMap<Multiaddr, usize>,
    dial_failed: usize,
}

impl RawAutoNATState {
    /// reset the autonat state.
    #[allow(unused)]
    pub(super) fn reset(&mut self) {
        self.dial_succ.clear();
        self.dial_failed = 0;
    }

    pub(super) fn success(&mut self, addr: Multiaddr) -> (AutoNAT, AutoNAT) {
        let before = self.state();

        if before == AutoNAT::Unknown {
            if let Some(counter) = self.dial_succ.get_mut(&addr) {
                *counter += 1;
            } else {
                self.dial_succ.insert(addr, 1);
            }
        }

        (before, self.state())
    }

    pub(super) fn failed(&mut self) -> (AutoNAT, AutoNAT) {
        let before = self.state();

        if before == AutoNAT::Unknown {
            self.dial_failed += 1;
        }

        (before, self.state())
    }

    pub(super) fn state(&self) -> AutoNAT {
        log::trace!(target:"autonat_client","dial_succ={}, dial_failed={}",self.dial_succ.len(),self.dial_failed);
        if self
            .dial_succ
            .iter()
            .map(|(_, counter)| *counter)
            .sum::<usize>()
            > 2
        {
            AutoNAT::Public
        } else if self.dial_failed > 2 {
            AutoNAT::Nat
        } else {
            AutoNAT::Unknown
        }
    }
}
