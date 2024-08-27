//! This module provides some useful primitive types for kad system.

use std::fmt::Display;

use uint::construct_uint;

use crate::kbucket::{KBucketDistance, KBucketKey};

construct_uint! {
    pub(crate) struct U256(4);
}

/// A kad key with 256 bits length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key(pub(crate) U256);

impl Key {
    /// Convert self into `Vec<u8>`
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = [0; 32];
        self.0.to_big_endian(&mut buf);

        buf.to_vec()
    }
}

impl From<[u8; 32]> for Key {
    fn from(value: [u8; 32]) -> Self {
        Self(value.into())
    }
}
impl From<Vec<u8>> for Key {
    fn from(value: Vec<u8>) -> Self {
        value.as_slice().into()
    }
}

impl From<&[u8]> for Key {
    fn from(value: &[u8]) -> Self {
        use sha2::Digest;

        let mut hasher = sha2::Sha256::new();

        hasher.update(value);

        let buf: [u8; 32] = hasher.finalize().into();

        Self::from(buf)
    }
}

impl From<identity::PeerId> for Key {
    fn from(value: identity::PeerId) -> Self {
        Key::from(&value)
    }
}
impl From<&identity::PeerId> for Key {
    fn from(value: &identity::PeerId) -> Self {
        Self::from(value.to_bytes())
    }
}

impl KBucketKey for Key {
    type Length = generic_array::typenum::U256;
    type Distance = Distance;

    /// Calculate the distance between two [`Key`]s.
    fn distance(&self, rhs: &Self) -> Distance {
        Distance(self.0 ^ rhs.0)
    }

    /// Returns the uniquely determined key with the given distance to `self`.
    ///
    /// This implements the following equivalence:
    ///
    /// `self xor other = distance <==> other = self xor distance`
    fn for_distance(&self, distance: Distance) -> Self {
        let key_int = self.0 ^ distance.0;

        Self(key_int.into())
    }

    /// Returns the longest common prefix length with `rhs`.
    fn longest_common_prefix(&self, rhs: &Self) -> usize {
        self.distance(rhs).0.leading_zeros() as usize
    }
}

/// The distance between two kad Keys.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Distance(pub(crate) U256);

impl KBucketDistance for Distance {
    /// Returns the integer part of the base 2 logarithm of the [`Distance`].
    ///
    /// Returns `None` if the distance is zero.
    fn k_index(&self) -> Option<u32> {
        (256 - self.0.leading_zeros()).checked_sub(1)
    }
}

impl Display for Distance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#066x}", self.0)
    }
}

#[cfg(test)]
mod tests {

    use std::net::SocketAddr;

    use super::*;

    use identity::PeerId;
    use quickcheck::*;

    impl Arbitrary for Key {
        fn arbitrary(_: &mut Gen) -> Key {
            Key::from(PeerId::random())
        }
    }

    #[test]
    fn distance_symmetry() {
        fn prop(a: Key, b: Key) -> bool {
            a.distance(&b) == b.distance(&a)
        }
        quickcheck(prop as fn(_, _) -> _)
    }

    #[test]
    fn for_distance() {
        fn prop(a: Key, b: Key) -> bool {
            a.for_distance(a.distance(&b)) == b
        }
        quickcheck(prop as fn(_, _) -> _)
    }

    #[test]
    fn k_distance_0() {
        assert_eq!(Distance(U256::from(0)).k_index(), None);
        assert_eq!(Distance(U256::from(1)).k_index(), Some(0));
        assert_eq!(Distance(U256::from(2)).k_index(), Some(1));
        assert_eq!(Distance(U256::from(3)).k_index(), Some(1));
        assert_eq!(Distance(U256::from(4)).k_index(), Some(2));
        assert_eq!(Distance(U256::from(5)).k_index(), Some(2));
        assert_eq!(Distance(U256::from(6)).k_index(), Some(2));
        assert_eq!(Distance(U256::from(7)).k_index(), Some(2));
    }

    #[test]
    fn distance_self() {
        let key = Key::from(PeerId::random());

        let distance = key.distance(&key);

        assert_eq!(distance.0, U256::from(0));
    }

    #[test]
    fn k_bucket_update() {
        let local_key = Key::from(PeerId::random());
        let mut k_bucket_table =
            crate::kbucket::KBucketTable::<Key, SocketAddr, 20>::new(local_key);

        assert_eq!(k_bucket_table.len(), 0);

        let mut key: Key;

        // select a valid key.
        loop {
            key = Key::from(PeerId::random());

            if U256::from(2).pow(
                key.distance(k_bucket_table.local_key())
                    .k_index()
                    .unwrap()
                    .into(),
            ) > k_bucket_table.k().into()
            {
                break;
            }
        }

        assert!(k_bucket_table
            .insert(key.clone(), "127.0.0.1:1921".parse().unwrap())
            .is_none());

        assert_eq!(k_bucket_table.len(), 1);

        let value = k_bucket_table.get(&key);

        assert_eq!(value, Some(&"127.0.0.1:1921".parse().unwrap()));

        assert!(k_bucket_table
            .insert(key.clone(), "127.0.0.1:1922".parse().unwrap())
            .is_none());

        assert_eq!(k_bucket_table.len(), 1);

        let value = k_bucket_table.get(&key);

        assert_eq!(value, Some(&"127.0.0.1:1922".parse().unwrap()));

        let k_index = key.distance(k_bucket_table.local_key()).k_index().unwrap();

        let mut key_value = key.0.clone();

        for _ in 0..k_bucket_table.k() {
            key_value = key_value - 1usize;

            let mut buf = [0u8; 32];

            key_value.to_big_endian(&mut buf);

            let new_k_index = Key::from(buf)
                .distance(k_bucket_table.local_key())
                .k_index()
                .unwrap();

            if new_k_index != k_index {
                break;
            }
        }

        // k-bucket is full
        for _ in 0..k_bucket_table.k() {
            key_value = key_value + 1;

            let mut buf = [0u8; 32];

            key_value.to_big_endian(&mut buf);

            let key = Key::from(buf);

            assert!(k_bucket_table
                .insert(key, "127.0.0.1:1921".parse().unwrap())
                .is_none());
        }

        assert_eq!(k_bucket_table.len(), k_bucket_table.k());

        key_value = key_value + 1;

        let mut buf = [0u8; 32];

        key_value.to_big_endian(&mut buf);

        // pop lru .
        assert!(k_bucket_table
            .insert(Key::from(buf), "127.0.0.1:1921".parse().unwrap())
            .is_some());
    }
}
