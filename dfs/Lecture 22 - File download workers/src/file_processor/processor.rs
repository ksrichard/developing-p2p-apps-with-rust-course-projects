use rs_merkle::algorithms::Sha256;
use rs_merkle::{Hasher, MerkleTree};
use rs_sha256::Sha256Hasher;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{Hash, Hasher as _};
use std::path::PathBuf;
use tokio::io::AsyncReadExt;
use tokio::{fs::File, io::BufReader};
use tonic::Status;

use crate::app::dfs_grpc::PublishFileRequest;

const CHUNK_SIZE: usize = 1024 * 1024; // 1 MB
pub const PROCESSING_RESULT_FILE_NAME: &str = "metadata.cbor";
pub const CHUNK_FILES_EXTENSION: &str = "chunk";

const LOG_TARGET: &str = "file_processor::processor";

#[derive(Debug, Serialize, Deserialize)]
pub struct FileProcessResult {
    pub original_file_name: String,
    pub number_of_chunks: usize,
    pub chunks_directory: PathBuf,
    pub merkle_root: [u8; 32],
    pub merkle_proofs: HashMap<usize, Vec<u8>>,
    pub public: bool,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct FileProcessResultHash(u64);

impl FileProcessResultHash {
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    pub fn raw_hash(&self) -> u64 {
        self.0
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let result: [u8; 8] = self.into();
        result.to_vec()
    }

    pub fn to_array(&self) -> [u8; 8] {
        self.into()
    }
}

impl TryFrom<Vec<u8>> for FileProcessResultHash {
    type Error = anyhow::Error;

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        let be_bytes: [u8; 8] = value
            .try_into()
            .map_err(|_| anyhow::anyhow!("Failed to convert big endian u64 from bytes!"))?;
        Ok(Self(u64::from_be_bytes(be_bytes)))
    }
}

impl From<&FileProcessResultHash> for [u8; 8] {
    fn from(value: &FileProcessResultHash) -> Self {
        value.0.to_be_bytes()
    }
}

impl FileProcessResult {
    pub fn new(
        original_file_name: String,
        number_of_chunks: usize,
        chunks_directory: PathBuf,
        merkle_root: [u8; 32],
        merkle_proofs: HashMap<usize, Vec<u8>>,
        public: bool,
    ) -> Self {
        Self {
            original_file_name,
            number_of_chunks,
            chunks_directory,
            merkle_root,
            merkle_proofs,
            public,
        }
    }

    pub fn hash_sha256(&self) -> FileProcessResultHash {
        let mut hasher = Sha256Hasher::default();
        self.hash(&mut hasher);
        FileProcessResultHash(hasher.finish())
    }
}

impl Hash for FileProcessResult {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.original_file_name.hash(state);
        self.number_of_chunks.hash(state);
        self.merkle_root.hash(state);
        self.public.hash(state);
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
        let _ = tokio::fs::remove_dir_all(pieces_dir.clone()).await;
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

            let target_dir = pieces_dir.join(format!("{}.{CHUNK_FILES_EXTENSION}", chunk_number));
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

        let result = FileProcessResult::new(
            file_name.to_string(),
            chunk_number,
            pieces_dir.clone(),
            merkle_root,
            merkle_proofs,
            request.public,
        );

        // save result to chunks dir
        let result_file_path = pieces_dir.join(PROCESSING_RESULT_FILE_NAME);
        let result_file = std::fs::File::create(result_file_path.clone()).map_err(|error| {
            Status::internal(format!(
                "Failed to create file ({:?}): {error}",
                result_file_path.as_path()
            ))
        })?;
        serde_cbor::to_writer(result_file, &result).map_err(|error| {
            Status::internal(format!(
                "Failed to write to file processing result file: {error}"
            ))
        })?;

        Ok(result)
    }
}
