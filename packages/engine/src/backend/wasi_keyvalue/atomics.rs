use wasmtime::component::Resource;

use super::{
    bucket_keys::{Key, KeyValueBucket},
    context::KeyValueState,
};
use crate::bindings::operator::world::wasi::keyvalue::atomics;

pub type AtomicsResult<T> = std::result::Result<T, atomics::Error>;
pub type CasResult<T> = std::result::Result<T, atomics::CasError>;

impl<'a> KeyValueState<'a> {
    fn get_cas_atomics(&self, cas: &Resource<KeyValueCas>) -> AtomicsResult<&KeyValueCas> {
        self.resource_table
            .get::<KeyValueCas>(cas)
            .map_err(|e| atomics::Error::Other(format!("Failed to get keyvalue cas: {}", e)))
    }

    fn get_key_atomics(
        &self,
        bucket: &Resource<KeyValueBucket>,
        key: String,
    ) -> AtomicsResult<Key> {
        self.get_key(bucket, key).map_err(atomics::Error::Other)
    }

    fn get_atomic_count(&mut self, key: &Key) -> AtomicsResult<Option<i64>> {
        Ok(self.db.kv_atomics_counter.get_cloned(&key.to_string()))
    }

    fn save_atomic_count(&mut self, key: &Key, value: i64) -> AtomicsResult<()> {
        self.db
            .kv_atomics_counter
            .insert(key.to_string(), value)
            .map_err(|e| {
                atomics::Error::Other(format!("Failed to set key in keyvalue atomics: {}", e))
            })
    }
}

impl atomics::Host for KeyValueState<'_> {
    fn increment(
        &mut self,
        bucket: Resource<KeyValueBucket>,
        key_id: String,
        delta: i64,
    ) -> AtomicsResult<i64> {
        let key = self.get_key_atomics(&bucket, key_id)?;

        let mut count = self.get_atomic_count(&key)?.unwrap_or(0);
        count += delta;
        self.save_atomic_count(&key, count)?;
        Ok(count)
    }

    fn swap(&mut self, cas: Resource<KeyValueCas>, value: Vec<u8>) -> CasResult<()> {
        // Snapshot from when this CAS was created (HostCas::new).
        let snapshot = {
            let entry = self
                .get_cas_atomics(&cas)
                .map_err(atomics::CasError::StoreError)?;
            entry.snapshot.clone()
        };
        let key = {
            let entry = self
                .get_cas_atomics(&cas)
                .map_err(atomics::CasError::StoreError)?;
            entry.key.clone()
        };

        let current = self
            .get_store_value(&key)
            .map_err(atomics::CasError::StoreError)?;

        if current != snapshot {
            // Another writer modified the key since we snapshotted. Hand the
            // caller a fresh CAS pinned to the new value so they can retry.
            let new_cas = KeyValueCas {
                key,
                snapshot: current,
            };
            let new_resource = self.resource_table.push(new_cas).map_err(|e| {
                atomics::CasError::StoreError(atomics::Error::Other(format!(
                    "Failed to create replacement keyvalue cas: {}",
                    e
                )))
            })?;
            return Err(atomics::CasError::CasFailed(new_resource));
        }

        self.set_store_value(&key, value)
            .map_err(atomics::CasError::StoreError)
    }
}

impl atomics::HostCas for KeyValueState<'_> {
    fn new(
        &mut self,
        bucket: Resource<KeyValueBucket>,
        key_id: String,
    ) -> AtomicsResult<Resource<KeyValueCas>> {
        let key = self.get_key_atomics(&bucket, key_id)?;
        let snapshot = self
            .get_store_value(&key)
            .map_err(|e| atomics::Error::Other(e.to_string()))?;
        let cas = KeyValueCas { key, snapshot };
        self.resource_table
            .push(cas)
            .map_err(|e| atomics::Error::Other(format!("Failed to create keyvalue cas: {}", e)))
    }

    fn current(&mut self, cas: Resource<KeyValueCas>) -> AtomicsResult<Option<Vec<u8>>> {
        let cas = self.get_cas_atomics(&cas)?;
        Ok(cas.snapshot.clone())
    }

    fn drop(&mut self, cas: Resource<KeyValueCas>) -> std::result::Result<(), wasmtime::Error> {
        self.resource_table.delete(cas)?;
        Ok(())
    }
}

pub struct KeyValueCas {
    pub key: Key,
    /// Value of `key` at the time `Cas::new` (or the last fresh CAS handed
    /// back by `swap`'s `CasFailed`) was called.
    pub snapshot: Option<Vec<u8>>,
}
