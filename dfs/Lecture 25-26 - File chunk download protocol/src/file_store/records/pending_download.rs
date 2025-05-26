use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::file_processor::FileProcessResultHash;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct PendingDownloadRecord {
    pub id: FileProcessResultHash,
    pub original_file_name: String,
    pub download_path: PathBuf,
    pub number_of_chunks: u64,
    pub downloaded_chunks: HashMap<usize, ()>,
}

impl PendingDownloadRecord {
    pub fn new(
        id: FileProcessResultHash,
        original_file_name: String,
        download_path: PathBuf,
        number_of_chunks: u64,
    ) -> Self {
        Self {
            id,
            original_file_name,
            download_path,
            downloaded_chunks: HashMap::new(),
            number_of_chunks,
        }
    }

    pub fn key(&self) -> Vec<u8> {
        self.id.to_bytes()
    }
}

impl TryInto<Vec<u8>> for PendingDownloadRecord {
    type Error = serde_cbor::Error;

    fn try_into(self) -> Result<Vec<u8>, Self::Error> {
        serde_cbor::to_vec(&self)
    }
}

impl TryFrom<Vec<u8>> for PendingDownloadRecord {
    type Error = serde_cbor::Error;

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        serde_cbor::from_slice(value.as_slice())
    }
}
