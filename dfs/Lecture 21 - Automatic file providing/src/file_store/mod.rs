use std::path::PathBuf;

use ::rocksdb::DBIteratorWithThreadMode;
use rocksdb::RocksDbStoreError;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::file_processor::{FileProcessResult, FileProcessResultHash};

pub mod rocksdb;

#[derive(Debug, Serialize, Deserialize)]
pub struct PublishedFileRecord {
    pub id: FileProcessResultHash,
    pub original_file_name: String,
    pub chunks_directory: PathBuf,
    pub public: bool,
}

impl From<FileProcessResult> for PublishedFileRecord {
    fn from(result: FileProcessResult) -> Self {
        Self {
            id: result.hash_sha256(),
            original_file_name: result.original_file_name,
            chunks_directory: result.chunks_directory,
            public: result.public,
        }
    }
}

impl PublishedFileRecord {
    pub fn new(
        id: FileProcessResultHash,
        original_file_name: String,
        chunks_directory: PathBuf,
        public: bool,
    ) -> Self {
        Self {
            id,
            original_file_name,
            chunks_directory,
            public,
        }
    }

    pub fn key(&self) -> Vec<u8> {
        self.id.to_bytes()
    }
}

impl TryInto<Vec<u8>> for PublishedFileRecord {
    type Error = serde_cbor::Error;

    fn try_into(self) -> Result<Vec<u8>, Self::Error> {
        serde_cbor::to_vec(&self)
    }
}

impl TryFrom<Vec<u8>> for PublishedFileRecord {
    type Error = serde_cbor::Error;

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        serde_cbor::from_slice(value.as_slice())
    }
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("RocksDB store error: {0}")]
    RocksDbStore(#[from] RocksDbStoreError),
    #[error("Published file not found by ID: {0}")]
    PublishedFileNotFound(u64),
}

pub trait Store {
    fn add_published_file(&self, record: PublishedFileRecord) -> Result<(), Error>;
    fn published_file_exists(&self, file_id: u64) -> Result<bool, Error>;
    fn published_file_metadata_path(&self, file_id: u64) -> Result<PathBuf, Error>;
    fn fetch_all_published_files(&self)
        -> Result<impl Iterator<Item = PublishedFileRecord>, Error>;
}
