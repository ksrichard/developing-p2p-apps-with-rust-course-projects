use std::{option::Iter, path::PathBuf};

use log::error;
use rocksdb::{ColumnFamily, ColumnFamilyDescriptor, IteratorMode, Options};
use thiserror::Error;

use crate::{
    file_processor::{FileProcessResultHash, PROCESSING_RESULT_FILE_NAME},
    file_store::PublishedFileRecord,
};

use super::Store;

const LOG_TARGET: &str = "file_store::rocksdb";

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
        Ok(Self {
            db: rocksdb::DB::open_cf_descriptors(&opts, folder.into(), vec![cfs])?,
        })
    }

    fn column_family(&self, cf_name: &str) -> Result<&ColumnFamily, RocksDbStoreError> {
        self.db
            .cf_handle(cf_name)
            .ok_or(RocksDbStoreError::ColumnFamilyMissing(cf_name.to_string()))
    }
}

impl Store for RocksDb {
    fn add_published_file(&self, record: super::PublishedFileRecord) -> Result<(), super::Error> {
        let cf = self.column_family(PUBLISHED_FILES_COLUMN_FAMILY_NAME)?;
        let key = record.key();
        let value: Vec<u8> = record
            .try_into()
            .map_err(|error| RocksDbStoreError::Cbor(error))?;
        self.db
            .put_cf(cf, key, value)
            .map_err(|error| RocksDbStoreError::RocksDb(error))?;
        Ok(())
    }

    fn published_file_exists(&self, file_id: u64) -> Result<bool, super::Error> {
        let cf = self.column_family(PUBLISHED_FILES_COLUMN_FAMILY_NAME)?;
        Ok(self
            .db
            .key_may_exist_cf(cf, FileProcessResultHash::new(file_id).to_bytes()))
    }

    fn published_file_metadata_path(&self, file_id: u64) -> Result<PathBuf, super::Error> {
        let cf = self.column_family(PUBLISHED_FILES_COLUMN_FAMILY_NAME)?;
        let value = self
            .db
            .get_cf(cf, FileProcessResultHash::new(file_id).to_bytes())
            .map_err(|error| super::Error::RocksDbStore(RocksDbStoreError::RocksDb(error)))?
            .ok_or(super::Error::PublishedFileNotFound(file_id))?;
        let result: PublishedFileRecord = value
            .try_into()
            .map_err(|error| super::Error::RocksDbStore(RocksDbStoreError::Cbor(error)))?;
        Ok(result.chunks_directory.join(PROCESSING_RESULT_FILE_NAME))
    }

    fn fetch_all_published_files(
        &self,
    ) -> Result<impl Iterator<Item = PublishedFileRecord>, super::Error> {
        let cf = self.column_family(PUBLISHED_FILES_COLUMN_FAMILY_NAME)?;
        let iter = self
            .db
            .iterator_cf(cf, IteratorMode::Start)
            .filter_map(|result| {
                if let Ok((_, value)) = result {
                    let value_result: Result<PublishedFileRecord, serde_cbor::Error> =
                        value.to_vec().try_into();
                    if let Ok(record) = value_result {
                        return Some(record);
                    }
                }
                None
            });
        Ok(iter)
    }
}
