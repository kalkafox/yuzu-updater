use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressState, ProgressStyle};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    ffi::{OsStr, OsString},
    fmt::Write,
    ops::Add,
    path::PathBuf,
    str::FromStr,
    sync::{
        atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};
use tokio::io::AsyncWriteExt;
use walkdir::WalkDir;

use clap::Parser;
use reqwest::header::{HeaderMap, HeaderValue};

static GITHUB_URL: &str = "https://api.github.com/repos/";

#[derive(Clone, Debug)]
enum UpdateType {
    AppImage,
    Standalone,
}

#[derive(Serialize, Deserialize, Debug)]
struct Release {
    url: String,
    assets_url: String,
    upload_url: String,
    html_url: String,
    id: u64,
    author: Author,
    node_id: String,
    tag_name: String,
    target_commitish: String,
    name: String,
    draft: bool,
    prerelease: bool,
    created_at: String,
    published_at: String,
    assets: Vec<Asset>,
    tarball_url: String,
    zipball_url: String,
    body: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct Author {
    login: String,
    id: u64,
    node_id: String,
    avatar_url: String,
    gravatar_id: String,
    url: String,
    html_url: String,
    followers_url: String,
    following_url: String,
    gists_url: String,
    starred_url: String,
    subscriptions_url: String,
    organizations_url: String,
    repos_url: String,
    events_url: String,
    received_events_url: String,
    #[serde(rename = "type")]
    author_type: String,
    site_admin: bool,
}

#[derive(Serialize, Deserialize, Debug)]
struct Asset {
    url: String,
    id: u64,
    node_id: String,
    name: String,
    label: String,
    uploader: Author,
    content_type: String,
    state: String,
    size: u64,
    download_count: u64,
    created_at: String,
    updated_at: String,
    browser_download_url: String,
}

impl UpdateType {
    const APP_IMAGE: &'static str = "appimage";
    const STANDALONE: &'static str = "standalone";
}

impl FromStr for UpdateType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            UpdateType::APP_IMAGE => Ok(UpdateType::AppImage),
            UpdateType::STANDALONE => Ok(UpdateType::Standalone),
            _ => Err(format!("Unknown update type: {}", s)),
        }
    }
}

/// Checks the latest yuzu version and downloads it if one is available
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Directory to extract DiscordCanary
    #[arg(short, long)]
    download_dir: Option<String>,

    /// Choose update type
    #[arg(short, long)]
    update_type: Option<UpdateType>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    let client = reqwest::Client::new();

    let mut headers = HeaderMap::new();

    headers.insert(
        reqwest::header::USER_AGENT,
        HeaderValue::from_static("yuzu-updater 0.1.0"),
    );

    let download_dir = args.download_dir.unwrap_or_else(|| {
        let home_dir = std::env::var("HOME").unwrap_or_else(|_| {
            panic!("HOME environment variable not found");
        });
        format!("{}/Downloads", home_dir)
    });

    let res = client
        .get(format!(
            "{}{}/{}/releases/latest",
            GITHUB_URL, "yuzu-emu", "yuzu-mainline"
        ))
        .headers(headers.clone())
        .send()
        .await?;

    let yuzu_release_data = res.json::<Release>().await?;

    let mut chosen_asset: Option<&Asset> = None;

    for asset in yuzu_release_data.assets.iter() {
        if asset.name.ends_with(".AppImage") {
            chosen_asset = Some(asset);
        }
    }

    if chosen_asset.is_none() {
        std::process::exit(1);
    }

    let asset = chosen_asset.unwrap();

    let file_args = asset.name.split("-").collect::<Vec<&str>>();

    let latest_commit = file_args[3].replace(".AppImage", "");

    println!("Latest commit: {}", latest_commit);

    let now = SystemTime::now();

    let mut matching_files: Vec<(OsString, SystemTime)> = Vec::new();

    for entry in WalkDir::new(&download_dir) {
        if entry.is_err() {
            continue;
        }

        let entry = entry?;

        let file_name = entry.file_name().to_str().unwrap();

        if file_name.starts_with("yuzu") && file_name.ends_with(".AppImage") {
            let date_created = entry.metadata()?.created()?;
            matching_files.push((entry.file_name().to_os_string(), date_created));
        }
    }

    let mut latest_file: Option<OsString> = None;

    if let Some((l_file, _)) = matching_files
        .iter()
        .max_by(|(_, time1), (_, time2)| time1.cmp(time2))
    {
        latest_file = Some(l_file.to_os_string());
    } else {
        println!("No matching files found.");
    }

    let file = latest_file.unwrap();

    let file_args = file.to_str().unwrap().split("-").collect::<Vec<&str>>();

    let latest_local_commit = file_args[3].replace(".AppImage", "");

    println!("Latest local commit: {}", latest_local_commit);

    if latest_commit == latest_local_commit {
        println!("Versions match.");
        return Ok(());
    }

    println!("Beginning download...");

    let download_url = asset.browser_download_url.to_owned();

    let file_name = asset.name.clone();

    let download_task = tokio::spawn(async move {
        let res = client
            .get(download_url)
            .headers(headers.clone())
            .send()
            .await
            .unwrap();

        let mut stream = res.bytes_stream();

        println!("{:}", file_name);

        let mut new_yuzu_file = tokio::fs::File::create(format!("{}/{}", download_dir, file_name))
            .await
            .unwrap();

        while let Some(item) = stream.next().await {
            let item = item.unwrap();

            new_yuzu_file.write_all(&item).await.unwrap();
        }
    });

    download_task.await?;

    Ok(())
}
