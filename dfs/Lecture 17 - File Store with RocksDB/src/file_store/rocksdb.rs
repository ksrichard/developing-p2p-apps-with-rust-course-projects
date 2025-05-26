use std::path::PathBuf;

use rocksdb::{BoundColumnFamily, ColumnFamily, ColumnFamilyDescriptor, ColumnFamilyRef, Options};
use thiserror::Error;

use super::Store;

const PUBLISHED_FILES_COLUMN_FAMILY_NAME: &str = "published_files";

pub struct RocksDb {
    db: rocksdb::DB,
}

#[derive(Error, Debug)]
pub enum RocksDbStoreError {
    #[error("RocksDB error: {0}")]
    RocksDb(#[from] rocksdb::Error),
    #[error("Column family does not exist: {0}")]
    ColumnFamilyMissing(String),
    #[error("Cbor error: {0}")]
    Cbor(#[from] serde_cbor::Error),
}

impl RocksDb {
    pub fn new<T: Into<PathBuf>>(folder: T) -> Result<Self, RocksDbStoreError> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);
        let cfs = ColumnFamilyDescriptor::new(PUBLISHED_FILES_COLUMN_FAMILY_NAME, opts.clone());
        Ok(
            Self {
            db: rocksdb::DB::open_cf_descriptors(&opts, folder.into(), vec![cfs])?,
        }
    )
    }
}

impl Store for RocksDb {
    fn add_published_file(&self, record: super::PublishedFileRecord) -> Result<(), super::Error> {
        let cf = self.db.cf_handle(PUBLISHED_FILES_COLUMN_FAMILY_NAME)
            .ok_or(RocksDbStoreError::ColumnFamilyMissing(PUBLISHED_FILES_COLUMN_FAMILY_NAME.to_string()))?;
        let key = record.key();
        let value: Vec<u8> = record.try_into().map_err(|error| RocksDbStoreError::Cbor(error))?;
        self.db.put_cf(cf, key, value)
        .map_err(|error| RocksDbStoreError::RocksDb(error))?;
    Ok(())
    }
}



