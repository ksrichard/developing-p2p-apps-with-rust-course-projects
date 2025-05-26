use std::path::PathBuf;

use rocksdb::RocksDbStoreError;
use thiserror::Error;

mod records;
pub mod rocksdb;
pub use records::*;

use crate::file_processor::FileProcessResultHash;

#[derive(Error, Debug)]
pub enum Error {
    #[error("RocksDB store error: {0}")]
    RocksDbStore(#[from] RocksDbStoreError),
    #[error("Published file not found by ID: {0}")]
    PublishedFileNotFound(u64),
    #[error("Failed to get number of downloaded chunk(s)!")]
    FailedToGetDownloadedChunks,
}

pub trait Store {
    // Published file operations
    fn add_published_file(&self, record: PublishedFileRecord) -> Result<(), Error>;
    fn published_file_exists(&self, file_id: u64) -> Result<bool, Error>;
    fn published_file_metadata_path(&self, file_id: u64) -> Result<PathBuf, Error>;
    fn fetch_all_published_files(&self)
        -> Result<impl Iterator<Item = PublishedFileRecord>, Error>;
    fn fetch_published_file_chunk_path(
        &self,
        id: &FileProcessResultHash,
        chunk_id: usize,
    ) -> Result<Option<PathBuf>, Error>;
    fn fetch_all_public_published_files(
        &self,
    ) -> Result<impl Iterator<Item = PublishedFileRecord> + Send + Sync, Error>;

    // Pending download operations
    fn add_pending_download(&self, record: PendingDownloadRecord) -> Result<(), Error>;
    fn remove_pending_download(&self, id: FileProcessResultHash) -> Result<(), Error>;
    fn fetch_all_pending_downloads(
        &self,
    ) -> Result<impl Iterator<Item = PendingDownloadRecord> + Send + Sync, Error>;
    fn add_downloaded_chunk_to_pending_download(
        &self,
        id: FileProcessResultHash,
        chunk_id: usize,
    ) -> Result<(), Error>;
    fn already_downloaded_chunks_in_pending_download(
        &self,
        id: &FileProcessResultHash,
    ) -> Result<Vec<u64>, Error>;
    fn chunk_downloaded_in_pending_downloads(
        &self,
        id: &FileProcessResultHash,
        chunk_id: usize,
    ) -> Result<bool, Error>;
    fn fetch_pending_downloaded_chunk_path(
        &self,
        id: &FileProcessResultHash,
        chunk_id: usize,
    ) -> Result<Option<PathBuf>, Error>;
}
