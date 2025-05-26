use serde::{Deserialize, Serialize};

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
