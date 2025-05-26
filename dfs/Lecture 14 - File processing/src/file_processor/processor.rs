use libp2p::futures::SinkExt;
use log::info;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncReadExt};
use tokio::{fs::File, io::BufReader};
use tonic::Status;

use crate::app::publish::PublishFileRequest;

const CHUNK_SIZE: usize = 1024 * 1024; // 1 MB

const LOG_TARGET: &str = "file_processor::processor";

#[derive(Debug)]
pub struct FileProcessResult {
    original_file_name: String,
    number_of_chunks: u64,
    chunks_directory: PathBuf,
    // TODO: add merkle tree root hash
}

impl FileProcessResult {
    pub fn new(
        original_file_name: String,
        number_of_chunks: u64,
        chunks_directory: PathBuf,
    ) -> Self {
        Self {
            original_file_name,
            number_of_chunks,
            chunks_directory,
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
        // TODO: delete dir before create
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
        let mut chunk_number = 1;
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

        Ok(FileProcessResult::new(
            file_name.to_string(),
            chunk_number-1,
            pieces_dir,
        ))
    }
}
