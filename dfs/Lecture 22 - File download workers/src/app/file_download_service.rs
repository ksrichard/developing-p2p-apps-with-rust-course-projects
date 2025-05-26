use async_trait::async_trait;
use log::{error, info};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::{
    file_processor::FileProcessResultHash,
    file_store::{self, PendingDownloadRecord},
};

use super::{ServerError, Service};

const LOG_TARGET: &str = "app::service::FileDownloadService";

pub struct FileDownloadService<F: file_store::Store + Send + Sync + 'static> {
    file_store: Arc<F>,
    downloads: Arc<RwLock<Vec<FileProcessResultHash>>>,
}

impl<F: file_store::Store + Send + Sync + 'static> FileDownloadService<F> {
    pub fn new(file_store: Arc<F>) -> Self {
        Self {
            file_store,
            downloads: Arc::new(RwLock::new(vec![])),
        }
    }

    async fn start_file_download(record: PendingDownloadRecord) {
        // TODO: load metadata to get the number of chunks
        // TODO: create a channel that workers can use to download chunks
        // TODO: start N number of workers for this file download
        // TODO: iterate over chunks and send to channel (send the corresponding merkle proof and merkle root for validation)
    }

    pub async fn check_pending_downloads(&self) {
        info!(target: LOG_TARGET, "Looking for new pending downloads...");
        match self.file_store.fetch_all_pending_downloads() {
            Ok(pending_downloads) => {
                for pending_download in pending_downloads {
                    let downloads = self.downloads.read().await;
                    if downloads.contains(&pending_download.id) {
                        info!(target: LOG_TARGET, "Download is in progress for {}!", pending_download.original_file_name);
                    } else {
                        info!(target: LOG_TARGET, "Starting download of file {}...", pending_download.original_file_name);
                        drop(downloads);
                        // add file to downloading list
                        let mut downloads = self.downloads.write().await;
                        if !downloads.contains(&pending_download.id) {
                            downloads.push(pending_download.id.clone());
                        }
                        drop(downloads);
                        let pending_dl = pending_download.clone();
                        tokio::spawn(async move {
                            Self::start_file_download(pending_dl).await;
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