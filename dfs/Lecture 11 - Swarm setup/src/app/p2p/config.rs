use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct P2pServiceConfig {
    pub keypair_file: PathBuf,
}

impl P2pServiceConfig {
    pub fn builder() -> P2pServiceConfigBuilder {
        P2pServiceConfigBuilder::new()
    }
}

#[derive(Debug, Clone)]
pub struct P2pServiceConfigBuilder {
    config: P2pServiceConfig,
}

impl P2pServiceConfigBuilder {
    pub fn new() -> Self {
        Self {
            config: P2pServiceConfig::default(),
        }
    }

    pub fn with_keypair_file<P: AsRef<Path>>(&mut self, keypair_file: P) -> &mut Self {
        self.config.keypair_file = keypair_file.as_ref().into();
        self
    }

    pub fn build(&self) -> P2pServiceConfig {
        self.config.clone()
    }
}