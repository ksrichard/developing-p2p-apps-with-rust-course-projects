use async_channel::Receiver;
use async_trait::async_trait;
use log::{error, info};
use std::{
    collections::HashMap,
    sync::{mpsc, Arc},
    time::Duration,
};
use tokio::{
    fs::File,
    sync::{broadcast, RwLock},
};
use tokio_util::sync::CancellationToken;

use crate::{
    file_processor::{FileProcessResult, FileProcessResultHash, PROCESSING_RESULT_FILE_NAME},
    file_store::{self, PendingDownloadRecord},
};

use super::{ServerError, Service};

const LOG_TARGET: &str = "app::service::FileDownloadService";

#[derive(Debug)]
struct FileChunkDownload {
    pub chunk_id: usize,
    pub merkle_root: [u8; 32],
    pub merkle_proof: Vec<u8>,
}

impl FileChunkDownload {
    pub fn new(chunk_id: usize, merkle_root: [u8; 32], merkle_proof: Vec<u8>) -> Self {
        Self {
            chunk_id,
            merkle_root,
            merkle_proof,
        }
    }
}

pub struct FileDownloadService<F: file_store::Store + Send + Sync + 'static> {
    file_store: Arc<F>,
    worker_count_per_file: u64,
    downloads: Arc<RwLock<HashMap<FileProcessResultHash, ()>>>,
}

impl<F: file_store::Store + Send + Sync + 'static> FileDownloadService<F> {
    pub fn new(file_store: Arc<F>, worker_count_per_file: u64) -> Self {
        Self {
            file_store,
            worker_count_per_file,
            downloads: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    async fn file_chunk_download_worker(
        worker_id: usize,
        chunk_download_rx: Receiver<FileChunkDownload>,
        success_tx: mpsc::Sender<usize>,
    ) {
        info!(target: LOG_TARGET, "File chunk download worker #{worker_id} has been started!");
        while let Ok(chunk_download) = chunk_download_rx.recv().await {
            info!(target: LOG_TARGET, "Received file chunk: {:?}", chunk_download);
            // TODO: implement chunk download process (by calling P2P command)
            // TODO: handle P2P command response:
            // TODO: Error case: log the error and retry the operation (so possibly other peer will be asked in P2P service)
            // TODO: Success case: save the file to downloads folder and send back success
            success_tx.send(chunk_download.chunk_id);
        }
        info!(target: LOG_TARGET, "File chunk download worker #{worker_id} has been finished!");
    }

    async fn start_file_download(
        file_store: Arc<F>,
        number_of_workers: u64,
        record: PendingDownloadRecord,
    ) -> anyhow::Result<()> {
        let metadata_file =
            File::open(record.download_path.join(PROCESSING_RESULT_FILE_NAME)).await?;
        let metadata: FileProcessResult = serde_cbor::from_reader(metadata_file.into_std().await)?;
        let (download_channel_tx, download_channel_rx) =
            async_channel::bounded::<FileChunkDownload>(100);
        let (chunk_download_success_tx, chunk_download_success_rx) = mpsc::channel::<usize>();

        let downloaded_chunk_ids =
            file_store.already_downloaded_chunks_in_pending_download(&record.id)?;

        let rest_of_chunks = metadata.number_of_chunks - downloaded_chunk_ids.len();
        let number_of_workers = if rest_of_chunks < number_of_workers as usize {
            rest_of_chunks
        } else {
            number_of_workers as usize
        };

        // spwaning workers
        for i in 0..number_of_workers {
            let dl_channel = download_channel_rx.clone();
            let success_tx = chunk_download_success_tx.clone();
            tokio::spawn(async move {
                Self::file_chunk_download_worker(i, dl_channel, success_tx).await;
            });
        }

        for i in 0..metadata.number_of_chunks {
            if downloaded_chunk_ids.contains(&(i as u64)) {
                continue;
            }
            if let Some(merkle_proof) = metadata.merkle_proofs.get(&i) {
                if let Err(error) = download_channel_tx
                    .send(FileChunkDownload::new(
                        i,
                        metadata.merkle_root.clone(),
                        merkle_proof.clone(),
                    ))
                    .await
                {
                    error!(target: LOG_TARGET, "Failed to send file chunk to download: {error:?}");
                }
            }
        }

        // waiting for results
        let record_id = record.id;
        while let Ok(chunk_id) = chunk_download_success_rx.recv() {
            file_store.add_downloaded_chunk_to_pending_download(record_id.clone(), chunk_id)?;
            // TODO: trigger here to provide chunk on DHT (via P2P command)
        }

        // TODO: and add new record to normal providers column family

        Ok(())
    }

    pub async fn check_pending_downloads(&self) {
        info!(target: LOG_TARGET, "Looking for new pending downloads...");
        match self.file_store.fetch_all_pending_downloads() {
            Ok(pending_downloads) => {
                for pending_download in pending_downloads {
                    let downloads = self.downloads.read().await;
                    if downloads.contains_key(&pending_download.id) {
                        info!(target: LOG_TARGET, "Download is in progress for {}!", pending_download.original_file_name);
                        // TODO: somehow check how many chunks we have been downloaded, so if it is successful, we can remove from pending list
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
                        tokio::spawn(async move {
                            if let Err(error) = Self::start_file_download(
                                file_store,
                                worker_count,
                                pending_dl.clone(),
                            )
                            .await
                            {
                                error!(target: LOG_TARGET, "Failed to start downloading file ({}): {error:?}", pending_dl.original_file_name);
                                let mut downloads = downloads_lock.write().await;
                                downloads.remove(&pending_dl.id);
                            }
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
