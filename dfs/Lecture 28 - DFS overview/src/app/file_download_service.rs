use anyhow::anyhow;
use async_channel::{Receiver, TryRecvError};
use async_trait::async_trait;
use log::{error, info};
use rs_merkle::{algorithms::Sha256, Hasher, MerkleProof};
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};
use tokio::{fs::File, sync::RwLock};
use tokio_util::sync::CancellationToken;

use crate::{
    app::service::FileChunkRequest,
    file_processor::{
        FileProcessResult, FileProcessResultHash, Processor, CHUNK_FILES_EXTENSION,
        PROCESSING_RESULT_FILE_NAME,
    },
    file_store::{self, PendingDownloadRecord},
};

use super::{
    service::{P2pCommand, ProvideFileChunkRequest},
    ServerError, Service,
};

const LOG_TARGET: &str = "app::service::FileDownloadService";

#[derive(Debug)]
struct FileChunkDownload {
    pub file_id: u64,
    pub chunk_id: usize,
    pub merkle_root: [u8; 32],
    pub merkle_proof: Vec<u8>,
    pub number_of_chunks: usize,
}

impl FileChunkDownload {
    pub fn new(
        file_id: u64,
        chunk_id: usize,
        merkle_root: [u8; 32],
        merkle_proof: Vec<u8>,
        number_of_chunks: usize,
    ) -> Self {
        Self {
            file_id,
            chunk_id,
            merkle_root,
            merkle_proof,
            number_of_chunks,
        }
    }
}

pub struct FileDownloadService<F: file_store::Store + Send + Sync + 'static> {
    file_store: Arc<F>,
    worker_count_per_file: u64,
    downloads: Arc<RwLock<HashMap<FileProcessResultHash, ()>>>,
    p2p_commands_tx: tokio::sync::mpsc::Sender<P2pCommand>,
}

impl<F: file_store::Store + Send + Sync + 'static> FileDownloadService<F> {
    pub fn new(
        file_store: Arc<F>,
        p2p_commands_tx: tokio::sync::mpsc::Sender<P2pCommand>,
        worker_count_per_file: u64,
    ) -> Self {
        Self {
            file_store,
            worker_count_per_file,
            downloads: Arc::new(RwLock::new(HashMap::new())),
            p2p_commands_tx,
        }
    }

    async fn file_chunk_download_worker(
        worker_id: usize,
        chunk_download_rx: Receiver<FileChunkDownload>,
        p2p_commands_tx: tokio::sync::mpsc::Sender<P2pCommand>,
        download_dir: PathBuf,
        success_tx: tokio::sync::mpsc::UnboundedSender<usize>,
    ) {
        info!(target: LOG_TARGET, "File chunk download worker #{worker_id} has been started!");
        loop {
            let recv_result = chunk_download_rx.try_recv();
            if let Err(TryRecvError::Empty) = recv_result {
                continue;
            }
            if let Err(TryRecvError::Closed) = recv_result {
                break;
            }
            let chunk_download = recv_result.unwrap();
            info!(target: LOG_TARGET, "Received file chunk: {}:{}", chunk_download.file_id, chunk_download.chunk_id);
            let (result_tx, result_rx) = tokio::sync::oneshot::channel::<Option<Vec<u8>>>();
            if let Err(error) = p2p_commands_tx
                .send(P2pCommand::RequestFileChunk {
                    request: FileChunkRequest {
                        file_id: chunk_download.file_id,
                        chunk_id: chunk_download.chunk_id,
                    },
                    result: result_tx,
                })
                .await
            {
                error!(target: LOG_TARGET, "Failed to send chunk download request: {error:?}");
                continue;
            }
            match result_rx.await {
                Ok(result) => match result {
                    Some(raw_chunk) => {
                        if let Ok(proof) =
                            MerkleProof::<Sha256>::try_from(chunk_download.merkle_proof.as_slice())
                        {
                            let chunk_hash = Sha256::hash(raw_chunk.as_slice());
                            if proof.verify(
                                chunk_download.merkle_root,
                                &[chunk_download.chunk_id],
                                &[chunk_hash],
                                chunk_download.number_of_chunks,
                            ) {
                                info!(target: LOG_TARGET, "Chunk {} is valid!", chunk_download.chunk_id);
                                let chunk_file_path = download_dir.join(format!(
                                    "{}.{}",
                                    chunk_download.chunk_id, CHUNK_FILES_EXTENSION
                                ));
                                if let Err(error) =
                                    tokio::fs::write(chunk_file_path.clone(), raw_chunk).await
                                {
                                    error!(target: LOG_TARGET, "Failed to write chunk to file ({chunk_file_path:?}): {error:?}");
                                }
                            }
                        }
                    }
                    None => error!(target: LOG_TARGET, "No file chunk has been received"),
                },
                Err(error) => {
                    error!(target: LOG_TARGET, "Failed to receive file chunk: {error:?}");
                }
            }
            // TODO: Error case: log the error and retry the operation (so possibly other peer will be asked in P2P service)

            if let Err(error) = success_tx.send(chunk_download.chunk_id) {
                error!(target: LOG_TARGET, "Failed to send back downloaded and validated file chunk: {error:?}");
            }
        }
        info!(target: LOG_TARGET, "File chunk download worker #{worker_id} has been finished!");
    }

    async fn download_file(
        file_store: Arc<F>,
        number_of_workers: u64,
        record: PendingDownloadRecord,
        p2p_commands_tx: tokio::sync::mpsc::Sender<P2pCommand>,
    ) -> anyhow::Result<()> {
        let metadata_file =
            File::open(record.download_path.join(PROCESSING_RESULT_FILE_NAME)).await?;
        let metadata: FileProcessResult = serde_cbor::from_reader(metadata_file.into_std().await)?;
        let (download_channel_tx, download_channel_rx) =
            async_channel::bounded::<FileChunkDownload>(100);
        let (chunk_download_success_tx, mut chunk_download_success_rx) =
            tokio::sync::mpsc::unbounded_channel::<usize>();

        let downloaded_chunk_ids =
            file_store.already_downloaded_chunks_in_pending_download(&record.id)?;

        let rest_of_chunks = metadata.number_of_chunks - downloaded_chunk_ids.len();
        let number_of_workers = if rest_of_chunks < number_of_workers as usize {
            rest_of_chunks
        } else {
            number_of_workers as usize
        };

        info!(target: LOG_TARGET, "Number of workers: {:?}", number_of_workers);

        // spawning workers
        for i in 0..=number_of_workers {
            let dl_channel = download_channel_rx.clone();
            let success_tx = chunk_download_success_tx.clone();
            let p2p_commands_sender = p2p_commands_tx.clone();
            let download_path = record.download_path.clone();
            tokio::spawn(async move {
                Self::file_chunk_download_worker(
                    i,
                    dl_channel,
                    p2p_commands_sender,
                    download_path,
                    success_tx,
                )
                .await;
            });
        }

        // drop cloned channel portions to avoid deadlock
        drop(chunk_download_success_tx);
        drop(download_channel_rx);

        for i in 0..metadata.number_of_chunks {
            if downloaded_chunk_ids.contains(&(i as u64)) {
                continue;
            }
            if let Some(merkle_proof) = metadata.merkle_proofs.get(&i) {
                if let Err(error) = download_channel_tx
                    .send(FileChunkDownload::new(
                        metadata.hash_sha256().raw_hash(),
                        i,
                        metadata.merkle_root.clone(),
                        merkle_proof.clone(),
                        metadata.number_of_chunks,
                    ))
                    .await
                {
                    error!(target: LOG_TARGET, "Failed to send file chunk to download: {error:?}");
                }
            }
        }

        download_channel_tx.close();

        // waiting for results
        let record_id = record.id;
        while let Some(chunk_id) = chunk_download_success_rx.recv().await {
            // Adding pending download to local DB
            file_store.add_downloaded_chunk_to_pending_download(record_id.clone(), chunk_id)?;

            // provide chunk on DHT
            if let Some(chunk_hash) = metadata.merkle_proof_hash(chunk_id) {
                let (provide_command_tx, provide_command_rx) =
                    tokio::sync::oneshot::channel::<bool>();
                p2p_commands_tx
                    .send(P2pCommand::ProvideFileChunk {
                        request: ProvideFileChunkRequest {
                            chunk_id,
                            chunk_hash,
                        },
                        result: provide_command_tx,
                    })
                    .await?;
                if !provide_command_rx.await? {
                    return Err(anyhow!("Failed to provide file chunk!"));
                }
            }
        }

        // process downloaded file
        let processor = Processor::new();
        processor
            .process_downloaded_file(record.download_path.clone(), &metadata)
            .await?;

        // adding file as published
        let file_id = metadata.hash_sha256();
        file_store.add_published_file(file_store::PublishedFileRecord {
            id: file_id.clone(),
            original_file_name: metadata.original_file_name,
            chunks_directory: record.download_path,
            public: metadata.public,
        })?;
        file_store.remove_pending_download(file_id.clone())?;

        // start providing file on DHT
        let (provide_command_tx, provide_command_rx) = tokio::sync::oneshot::channel::<bool>();
        p2p_commands_tx
            .send(P2pCommand::ProvideFile {
                request: super::service::ProvideFileRequest {
                    file_id,
                    number_of_chunks: metadata.number_of_chunks,
                    merkle_root: metadata.merkle_root,
                },
                result: provide_command_tx,
            })
            .await?;
        if !provide_command_rx.await? {
            return Err(anyhow!("Failed to provide file!"));
        }

        Ok(())
    }

    pub async fn check_pending_downloads(&self) {
        info!(target: LOG_TARGET, "Checking pending downloads...");
        match self.file_store.fetch_all_pending_downloads() {
            Ok(pending_downloads) => {
                for pending_download in pending_downloads {
                    let downloads = self.downloads.read().await;
                    if downloads.contains_key(&pending_download.id) {
                        info!(target: LOG_TARGET, "Download is in progress for {}!", pending_download.original_file_name);
                    } else {
                        info!(target: LOG_TARGET, "Starting download of file {}...", pending_download.original_file_name);
                        drop(downloads);
                        // add file to downloading list
                        let mut downloads = self.downloads.write().await;
                        if !downloads.contains_key(&pending_download.id) {
                            downloads.insert(pending_download.id.clone(), ());
                        }
                        drop(downloads);
                        let pending_dl = pending_download.clone();
                        let downloads_lock = self.downloads.clone();
                        let worker_count = self.worker_count_per_file;
                        let file_store = self.file_store.clone();
                        let p2p_commands_sender = self.p2p_commands_tx.clone();
                        tokio::spawn(async move {
                            if let Err(error) = Self::download_file(
                                file_store,
                                worker_count,
                                pending_dl.clone(),
                                p2p_commands_sender,
                            )
                            .await
                            {
                                error!(target: LOG_TARGET, "Failed to download file ({}): {error:?}", pending_dl.original_file_name);
                            }
                            let mut downloads = downloads_lock.write().await;
                            downloads.remove(&pending_dl.id);
                        });
                    }
                }
            }
            Err(error) => error!(target: LOG_TARGET, "Failed to get pending downloads: {error}"),
        }
    }
}

#[async_trait]
impl<F: file_store::Store + Send + Sync + 'static> Service for FileDownloadService<F> {
    async fn start(&mut self, cancel_token: CancellationToken) -> Result<(), ServerError> {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    self.check_pending_downloads().await;
                }
                _ = cancel_token.cancelled() => {
                    info!(target: LOG_TARGET, "File download service shutting down...");
                    break;
                }
            }
        }

        Ok(())
    }
}
