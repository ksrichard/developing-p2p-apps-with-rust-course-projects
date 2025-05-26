use std::path::PathBuf;

use log::error;
use rocksdb::{
    BoundColumnFamily, ColumnFamily, ColumnFamilyDescriptor, ColumnFamilyRef, IteratorMode, Options,
};
use thiserror::Error;

use crate::{file_processor::{FileProcessResultHash, PROCESSING_RESULT_FILE_NAME}, file_store::PublishedFileRecord};

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
}

impl Store for RocksDb {
    fn add_published_file(&self, record: super::PublishedFileRecord) -> Result<(), super::Error> {
        let cf = self
            .db
            .cf_handle(PUBLISHED_FILES_COLUMN_FAMILY_NAME)
            .ok_or(RocksDbStoreError::ColumnFamilyMissing(
                PUBLISHED_FILES_COLUMN_FAMILY_NAME.to_string(),
            ))?;
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
        let cf = self
            .db
            .cf_handle(PUBLISHED_FILES_COLUMN_FAMILY_NAME)
            .ok_or(RocksDbStoreError::ColumnFamilyMissing(
                PUBLISHED_FILES_COLUMN_FAMILY_NAME.to_string(),
            ))?;
        Ok(self
            .db
            .full_iterator_cf(cf, IteratorMode::Start)
            .filter_map(|result| {
                if let Ok((key, _)) = result {
                    let key: Result<FileProcessResultHash, anyhow::Error> = key.to_vec().try_into();
                    if let Ok(key) = key {
                        return Some(key);
                    }
                }
                None
            })
            .any(|key| key.raw_hash() == file_id))
    }

    fn published_file_metadata_path(&self, file_id: u64) -> Result<PathBuf, super::Error> {
        let cf = self
            .db
            .cf_handle(PUBLISHED_FILES_COLUMN_FAMILY_NAME)
            .ok_or(RocksDbStoreError::ColumnFamilyMissing(
                PUBLISHED_FILES_COLUMN_FAMILY_NAME.to_string(),
            ))?;
        let value = self
            .db
            .get_cf(cf, FileProcessResultHash::new(file_id).to_bytes())
            .map_err(|error| super::Error::RocksDbStore(RocksDbStoreError::RocksDb(error)))?
            .ok_or(super::Error::PublishedFileNotFound(file_id))?;
        let result: PublishedFileRecord = value.try_into()
            .map_err(|error| super::Error::RocksDbStore(RocksDbStoreError::Cbor(error)))?;
        Ok(
            result.chunks_directory.join(PROCESSING_RESULT_FILE_NAME)
        )
    }
}
