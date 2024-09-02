/// A variant for [*autonat protocol*](https://github.com/libp2p/specs/tree/master/autonat) state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AutoNAT {
    /// the node is a public node.
    Public,
    /// the node is behind  nat/firewall.
    NAT,
    /// Cannot determine if the node is behind nat/firewall
    Unknown,
}

impl Default for AutoNAT {
    fn default() -> Self {
        Self::Unknown
    }
}
