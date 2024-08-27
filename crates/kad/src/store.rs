//! This module provides route table abstraction for [`KadSwitch`](crate::KadSwitch).

use std::collections::HashMap;

use async_trait::async_trait;
use futures::lock::Mutex;
use xstack::driver_wrapper;

use crate::key::Key;

/// A record store must implement the `Driver-*` traits in this module.
pub mod syscall {
    use async_trait::async_trait;

    use crate::key::Key;

    /// A trait that provides functions to access peer informations.
    #[async_trait]
    pub trait DriverKadStore: Sync + Send {
        /// insert a new kad record.
        async fn insert(&self, key: Key, record: Vec<u8>) -> std::io::Result<()>;

        /// remove record by key.
        async fn remove(&self, key: &Key) -> std::io::Result<Option<Vec<u8>>>;

        /// Get the record by key.
        async fn get(&self, key: &Key) -> std::io::Result<Option<Vec<u8>>>;
    }
}

driver_wrapper!(
    ["A type wrapper of [`DriverKadStore`](syscall::DriverKadStore)"]
    KadStore[syscall::DriverKadStore]
);

pub struct KadMemoryStore(Mutex<HashMap<Key, Vec<u8>>>);

impl KadMemoryStore {
    pub fn new() -> Self {
        KadMemoryStore(Default::default())
    }
}

#[async_trait]
impl syscall::DriverKadStore for KadMemoryStore {
    /// insert a new kad record.
    async fn insert(&self, key: Key, record: Vec<u8>) -> std::io::Result<()> {
        self.0.lock().await.insert(key, record);

        Ok(())
    }

    /// remove record by key.
    async fn remove(&self, key: &Key) -> std::io::Result<Option<Vec<u8>>> {
        Ok(self.0.lock().await.remove(key))
    }

    /// Get the record by key.
    async fn get(&self, key: &Key) -> std::io::Result<Option<Vec<u8>>> {
        Ok(self.0.lock().await.get(key).map(|record| record.to_owned()))
    }
}
