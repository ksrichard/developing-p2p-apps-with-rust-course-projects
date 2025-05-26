use std::{env, path::PathBuf};

use clap::{command, Parser, Subcommand};
use homedir::my_home;

const DEFAULT_BASE_DIR: &str = ".dfs";

fn default_base_path() -> PathBuf {
    let home_dir_result = my_home();
    if let Ok(Some(mut home_dir)) = home_dir_result {
        home_dir.push(DEFAULT_BASE_DIR);
        return home_dir;
    }
    let mut path = env::current_dir().expect("Can't get current directory");
    path.push(DEFAULT_BASE_DIR);
    path
}

#[derive(Subcommand)]
pub enum Commands {
    Start,
}

#[derive(Parser)]
#[command(version, about)]
pub struct Cli {
    #[arg(short, long, env = "DFS_BASE_PATH", default_value = default_base_path().into_os_string())]
    pub base_path: PathBuf,

    #[arg(short, long, default_value_t = 9999, env = "DFS_GRPC_PORT")]
    pub grpc_port: u16,

    #[command(subcommand)]
    pub command: Commands,
}
