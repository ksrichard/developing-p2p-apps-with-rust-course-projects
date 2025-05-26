use libp2p::futures::SinkExt;
use log::info;
use rs_merkle::algorithms::Sha256;
use rs_merkle::{Hasher, MerkleTree};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncReadExt};
use tokio::{fs::File, io::BufReader};
use tonic::Status;

use crate::app::publish::PublishFileRequest;

const CHUNK_SIZE: usize = 1024 * 1024; // 1 MB

const LOG_TARGET: &str = "file_processor::processor";

#[derive(Debug)]
pub struct FileProcessResult {
    pub original_file_name: String,
    pub number_of_chunks: usize,
    pub chunks_directory: PathBuf,
    pub merkle_root: [u8; 32],
    pub merkle_proofs: HashMap<usize, Vec<u8>>,
}

impl FileProcessResult {
    pub fn new(
        original_file_name: String,
        number_of_chunks: usize,
        chunks_directory: PathBuf,
        merkle_root: [u8; 32],
        merkle_proofs: HashMap<usize, Vec<u8>>,
    ) -> Self {
        Self {
            original_file_name,
            number_of_chunks,
            chunks_directory,
            merkle_root,
            merkle_proofs,
        }
    }
}

pub struct Processor;

impl Processor {
    pub fn new() -> Self {
        Self {}
    }

    pub async fn process_file(
        &self,
        request: &PublishFileRequest,
    ) -> Result<FileProcessResult, tonic::Status> {
        let metadata = tokio::fs::metadata(request.file_path.clone())
            .await
            .map_err(|error| {
                Status::invalid_argument(format!("Cannot read file metadata: {error}"))
            })?;
        if !metadata.is_file() {
            return Err(Status::invalid_argument("Not a file!"));
        }

        // TODO: zip if it's a folder

        // creating chunks directory
        let file_path = PathBuf::from(request.file_path.clone());
        let containing_dir = file_path.parent().ok_or(Status::invalid_argument(
            "Cannot get file's parent directory!",
        ))?;
        let file_name = file_path
            .file_name()
            .ok_or(Status::invalid_argument("Cannot get file's name!"))?
            .to_string_lossy();

        let pieces_dir = containing_dir.join(format!("{}_chunks", file_name.replace(".", "_")));
        info!(target: LOG_TARGET, "Chunks dir: {:?}", pieces_dir.as_path());
        tokio::fs::remove_dir_all(pieces_dir.clone())
            .await
            .map_err(|error| {
                Status::internal(format!(
                    "Failed to delete chunks directory ({:?}): {error}",
                    pieces_dir.as_path()
                ))
            })?;
        tokio::fs::create_dir_all(pieces_dir.clone())
            .await
            .map_err(|error| {
                Status::internal(format!(
                    "Failed to create chunks directory ({:?}) for the file: {error}",
                    pieces_dir.as_path()
                ))
            })?;

        // reading file in chunks
        let file = File::open(request.file_path.clone())
            .await
            .map_err(|error| Status::internal(format!("Cannot open file: {error}")))?;
        let mut buffer = [0; 1024];
        let mut reader = BufReader::new(file);
        let mut chunk_number = 0;
        let mut merkle_tree = MerkleTree::<Sha256>::new();
        loop {
            let mut to_write = Vec::<u8>::with_capacity(CHUNK_SIZE);
            let mut n = 0;
            while to_write.len() < CHUNK_SIZE {
                n = reader
                    .read(&mut buffer)
                    .await
                    .map_err(|error| Status::internal(format!("Failed to read file: {error}")))?;
                to_write.append(&mut buffer.to_vec());
                buffer = [0; 1024];
                if n == 0 {
                    break;
                }
            }

            merkle_tree.insert(Sha256::hash(to_write.as_slice()));

            let target_dir = pieces_dir.join(format!("{}.chunk", chunk_number));
            tokio::fs::write(target_dir, to_write)
                .await
                .map_err(|error| {
                    Status::internal(format!("Failed to write file chunk: {error}"))
                })?;

            chunk_number += 1;

            if n == 0 {
                break;
            }
        }
        merkle_tree.commit();
        let merkle_root = merkle_tree.root().ok_or(Status::internal(
            "Failed to get merkle root hash for chunks!",
        ))?;

        let mut merkle_proofs = HashMap::with_capacity(chunk_number);
        for i in 0..chunk_number {
            let proof = merkle_tree.proof(&[i]);
            merkle_proofs.insert(i, proof.to_bytes());
        }

        Ok(FileProcessResult::new(
            file_name.to_string(),
            chunk_number,
            pieces_dir,
            merkle_root,
            merkle_proofs,
        ))
    }
}
