use serde::{Deserialize, Serialize};

use crate::{file_processor::FileProcessResultHash, file_store::PublishedFileRecord};

#[derive(Debug, Serialize, Deserialize)]
pub struct PublishedFile {
    pub number_of_chunks: usize,
    pub merkle_root: [u8; 32],
}

impl PublishedFile {
    pub fn new(number_of_chunks: usize, merkle_root: [u8; 32]) -> Self {
        Self {
            number_of_chunks,
            merkle_root,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PublishedFileChunk {
    pub chunk_number: u64,
}

impl PublishedFileChunk {
    pub fn new(chunk_number: u64) -> Self {
        Self { chunk_number }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
pub struct PublicPublishedFile {
    pub id: FileProcessResultHash,
    pub original_file_name: String,
}

impl From<&PublishedFileRecord> for PublicPublishedFile {
    fn from(record: &PublishedFileRecord) -> Self {
        Self {
            id: record.id.clone(),
            original_file_name: record.original_file_name.clone(),
        }
    }
}
