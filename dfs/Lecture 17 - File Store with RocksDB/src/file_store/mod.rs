use std::path::PathBuf;

use rocksdb::RocksDbStoreError;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::file_processor::FileProcessResult;

pub mod rocksdb;

#[derive(Debug, Serialize, Deserialize)]
pub struct PublishedFileRecord {
    pub original_file_name: String,
    pub chunks_directory: PathBuf,
    pub public: bool,
}

impl From<FileProcessResult> for PublishedFileRecord {
    fn from(result: FileProcessResult) -> Self {
        Self {
            original_file_name: result.original_file_name,
            chunks_directory: result.chunks_directory,
            public: result.public,
        }
    }
}

impl PublishedFileRecord {
    pub fn new(
        original_file_name: String,
        chunks_directory: PathBuf,
        public: bool,
    ) -> Self {
            Self {
                original_file_name,
                chunks_directory,
                public,
            }
        }

    pub fn key(&self) -> Vec<u8> {
        self.original_file_name.clone().into_bytes()
    }
}

impl TryInto<Vec<u8>> for PublishedFileRecord {
    type Error = serde_cbor::Error;

    fn try_into(self) -> Result<Vec<u8>, Self::Error> {
        serde_cbor::to_vec(&self)
    }
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("RocksDB store error: {0}")]
    RocksDbStore(#[from] RocksDbStoreError),
}

pub trait Store {
    fn add_published_file(&self, record: PublishedFileRecord) -> Result<(), Error>;
}