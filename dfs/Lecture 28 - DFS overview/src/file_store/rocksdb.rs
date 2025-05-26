use std::path::PathBuf;

use libp2p::kad::RecordKey;
use log::error;
use rocksdb::{ColumnFamily, ColumnFamilyDescriptor, IteratorMode, Options};
use thiserror::Error;

use crate::{
    file_processor::{FileProcessResultHash, CHUNK_FILES_EXTENSION, PROCESSING_RESULT_FILE_NAME},
    file_store::PublishedFileRecord,
};

use super::{PendingDownloadRecord, Store};

const PUBLISHED_FILES_COLUMN_FAMILY_NAME: &str = "published_files";
const PENDING_DOWNLOADS_COLUMN_FAMILY_NAME: &str = "pending_downloads";

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
        let published_files_cf =
            ColumnFamilyDescriptor::new(PUBLISHED_FILES_COLUMN_FAMILY_NAME, opts.clone());
        let pending_downloads_cf =
            ColumnFamilyDescriptor::new(PENDING_DOWNLOADS_COLUMN_FAMILY_NAME, opts.clone());
        Ok(Self {
            db: rocksdb::DB::open_cf_descriptors(
                &opts,
                folder.into(),
                vec![published_files_cf, pending_downloads_cf],
            )?,
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

    fn fetch_published_file_chunk_path(
        &self,
        id: &FileProcessResultHash,
        chunk_id: usize,
    ) -> Result<Option<PathBuf>, super::Error> {
        let cf = self.column_family(PUBLISHED_FILES_COLUMN_FAMILY_NAME)?;
        let raw_published_file_result = self
            .db
            .get_cf(cf, id.to_bytes())
            .map_err(|error| super::Error::RocksDbStore(RocksDbStoreError::RocksDb(error)))?;
        if raw_published_file_result.is_none() {
            return Ok(None);
        }
        let raw_published_file = raw_published_file_result.unwrap();
        let published_file: PublishedFileRecord = raw_published_file
            .to_vec()
            .try_into()
            .map_err(|error| super::Error::RocksDbStore(RocksDbStoreError::Cbor(error)))?;
        Ok(Some(
            published_file
                .chunks_directory
                .join(format!("{}.{}", chunk_id, CHUNK_FILES_EXTENSION)),
        ))
    }
    fn fetch_all_public_published_files(
        &self,
    ) -> Result<impl Iterator<Item = PublishedFileRecord> + Send + Sync, super::Error> {
        let cf = self.column_family(PUBLISHED_FILES_COLUMN_FAMILY_NAME)?;
        let iter = self
            .db
            .iterator_cf(cf, IteratorMode::Start)
            .filter_map(|result| {
                if let Ok((_, value)) = result {
                    let value_result: Result<PublishedFileRecord, serde_cbor::Error> =
                        value.to_vec().try_into();
                    if let Ok(record) = value_result {
                        if record.public {
                            return Some(record);
                        }
                    }
                }
                None
            });
        Ok(iter)
    }

    fn add_pending_download(
        &self,
        record: super::PendingDownloadRecord,
    ) -> Result<(), super::Error> {
        let cf = self.column_family(PENDING_DOWNLOADS_COLUMN_FAMILY_NAME)?;
        let key = record.key();
        let value: Vec<u8> = record
            .try_into()
            .map_err(|error| RocksDbStoreError::Cbor(error))?;
        self.db
            .put_cf(cf, key, value)
            .map_err(|error| RocksDbStoreError::RocksDb(error))?;
        Ok(())
    }

    fn remove_pending_download(&self, id: FileProcessResultHash) -> Result<(), super::Error> {
        let cf = self.column_family(PENDING_DOWNLOADS_COLUMN_FAMILY_NAME)?;
        let key = RecordKey::new(&id.to_array());
        self.db
            .delete_cf(cf, key)
            .map_err(|error| RocksDbStoreError::RocksDb(error))?;
        Ok(())
    }

    fn fetch_all_pending_downloads(
        &self,
    ) -> Result<impl Iterator<Item = super::PendingDownloadRecord> + Send + Sync, super::Error>
    {
        let cf = self.column_family(PENDING_DOWNLOADS_COLUMN_FAMILY_NAME)?;
        let iter = self
            .db
            .iterator_cf(cf, IteratorMode::Start)
            .filter_map(|result| {
                if let Ok((_, value)) = result {
                    let value_result: Result<PendingDownloadRecord, serde_cbor::Error> =
                        value.to_vec().try_into();
                    if let Ok(record) = value_result {
                        return Some(record);
                    }
                }
                None
            });
        Ok(iter)
    }

    fn add_downloaded_chunk_to_pending_download(
        &self,
        id: FileProcessResultHash,
        chunk_id: usize,
    ) -> Result<(), super::Error> {
        let cf = self.column_family(PENDING_DOWNLOADS_COLUMN_FAMILY_NAME)?;
        if let Some(value) = self
            .db
            .get_cf(cf, id.to_bytes())
            .map_err(|error| super::Error::RocksDbStore(RocksDbStoreError::RocksDb(error)))?
        {
            let value_result: Result<PendingDownloadRecord, serde_cbor::Error> =
                value.to_vec().try_into();
            if let Ok(mut record) = value_result {
                record.downloaded_chunks.insert(chunk_id, ());
                self.add_pending_download(record)?;
            }
        }
        Ok(())
    }

    fn already_downloaded_chunks_in_pending_download(
        &self,
        id: &FileProcessResultHash,
    ) -> Result<Vec<u64>, super::Error> {
        let cf = self.column_family(PENDING_DOWNLOADS_COLUMN_FAMILY_NAME)?;
        if let Some(value) = self
            .db
            .get_cf(cf, id.to_bytes())
            .map_err(|error| super::Error::RocksDbStore(RocksDbStoreError::RocksDb(error)))?
        {
            let value_result: Result<PendingDownloadRecord, serde_cbor::Error> =
                value.to_vec().try_into();
            if let Ok(record) = value_result {
                return Ok(record
                    .downloaded_chunks
                    .keys()
                    .map(|value| *value as u64)
                    .collect());
            }
        }

        Err(super::Error::FailedToGetDownloadedChunks)
    }

    fn chunk_downloaded_in_pending_downloads(
        &self,
        id: &FileProcessResultHash,
        chunk_id: usize,
    ) -> Result<bool, super::Error> {
        let cf = self.column_family(PENDING_DOWNLOADS_COLUMN_FAMILY_NAME)?;
        if let Some(value) = self
            .db
            .get_cf(cf, id.to_bytes())
            .map_err(|error| super::Error::RocksDbStore(RocksDbStoreError::RocksDb(error)))?
        {
            let value_result: Result<PendingDownloadRecord, serde_cbor::Error> =
                value.to_vec().try_into();
            if let Ok(record) = value_result {
                return Ok(record.downloaded_chunks.contains_key(&chunk_id));
            }
        }

        Err(super::Error::FailedToGetDownloadedChunks)
    }

    fn fetch_pending_downloaded_chunk_path(
        &self,
        id: &FileProcessResultHash,
        chunk_id: usize,
    ) -> Result<Option<PathBuf>, super::Error> {
        let cf = self.column_family(PENDING_DOWNLOADS_COLUMN_FAMILY_NAME)?;
        let raw_published_file_result = self
            .db
            .get_cf(cf, id.to_bytes())
            .map_err(|error| super::Error::RocksDbStore(RocksDbStoreError::RocksDb(error)))?;
        if raw_published_file_result.is_none() {
            return Ok(None);
        }
        let raw_published_file = raw_published_file_result.unwrap();
        let published_file: PendingDownloadRecord = raw_published_file
            .to_vec()
            .try_into()
            .map_err(|error| super::Error::RocksDbStore(RocksDbStoreError::Cbor(error)))?;
        Ok(Some(
            published_file
                .download_path
                .join(format!("{}.{}", chunk_id, CHUNK_FILES_EXTENSION)),
        ))
    }
}
