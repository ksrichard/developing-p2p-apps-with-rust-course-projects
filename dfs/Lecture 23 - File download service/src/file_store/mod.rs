use std::path::PathBuf;

use rocksdb::RocksDbStoreError;
use thiserror::Error;

mod records;
pub mod rocksdb;
pub use records::*;

#[derive(Error, Debug)]
pub enum Error {
    #[error("RocksDB store error: {0}")]
    RocksDbStore(#[from] RocksDbStoreError),
    #[error("Published file not found by ID: {0}")]
    PublishedFileNotFound(u64),
}

pub trait Store {
    // Published file operations
    fn add_published_file(&self, record: PublishedFileRecord) -> Result<(), Error>;
    fn published_file_exists(&self, file_id: u64) -> Result<bool, Error>;
    fn published_file_metadata_path(&self, file_id: u64) -> Result<PathBuf, Error>;
    fn fetch_all_published_files(&self)
        -> Result<impl Iterator<Item = PublishedFileRecord>, Error>;

    // Pending download operations
    fn add_pending_download(&self, record: PendingDownloadRecord) -> Result<(), Error>;
    fn fetch_all_pending_downloads(
        &self,
    ) -> Result<impl Iterator<Item = PendingDownloadRecord> + Send + Sync, Error>;
}
