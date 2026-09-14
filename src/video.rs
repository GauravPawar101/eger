//! Video → ASCII rendering with bounded memory frame streaming.

use crate::config::Config;
use crate::error::{EgerError, Result};
use crate::render;
use iascii::convert::convert_image;
use iascii::render::ColorDepth;
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufWriter};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

/// Basic video stream metadata needed to drive frame extraction.
#[derive(Debug, Clone, Copy)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub frame_count: Option<u64>,
}

/// Probes a video file with `ffprobe` for its resolution and frame rate.
pub async fn probe(path: &Path) -> Result<VideoInfo> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,r_frame_rate,nb_frames",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .await
        .map_err(|_| EgerError::FfmpegNotFound)?;

    if !output.status.success() {
        return Err(EgerError::Ffmpeg(
            output.status,
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut fields = stdout.trim().split(',');

    let width: u32 = fields
        .next()
        .and_then(|s| s.trim().parse().ok())
        .ok_or_else(|| EgerError::Video("ffprobe: could not parse width".into()))?;
    let height: u32 = fields
        .next()
        .and_then(|s| s.trim().parse().ok())
        .ok_or_else(|| EgerError::Video("ffprobe: could not parse height".into()))?;
    let fps = fields.next().map(parse_frame_rate).unwrap_or(30.0);
    let frame_count = fields.next().and_then(|s| s.trim().parse().ok());

    Ok(VideoInfo {
        width,
        height,
        fps,
        frame_count,
    })
}

fn parse_frame_rate(raw: &str) -> f64 {
    let raw = raw.trim();
    if let Some((num, den)) = raw.split_once('/') {
        match (num.trim().parse::<f64>(), den.trim().parse::<f64>()) {
            (Ok(n), Ok(d)) if d != 0.0 => n / d,
            _ => 30.0,
        }
    } else {
        raw.parse().unwrap_or(30.0)
    }
}

async fn spawn_frame_stream(path: &Path) -> Result<Child> {
    Command::new("ffmpeg")
        .args(["-loglevel", "error", "-i"])
        .arg(path)
        .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| EgerError::FfmpegNotFound)
}

/// Streams converted ASCII frame batches over a bounded `mpsc` channel.
///
/// Limits memory usage to active worker chunk buffers rather than holding the entire
/// video's rendered frames in memory.
pub async fn stream_video_frames(
    path: &Path,
    config: Arc<Config>,
    depth_override: Option<ColorDepth>,
    tx: mpsc::Sender<Result<Vec<String>>>,
) -> Result<()> {
    let info = probe(path).await?;
    let frame_bytes = (info.width as usize) * (info.height as usize) * 3;
    if frame_bytes == 0 {
        return Err(EgerError::Video("video has a zero-sized frame".into()));
    }

    let mut child = spawn_frame_stream(path).await?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| EgerError::Video("failed to capture ffmpeg stdout".into()))?;

    let color_depth = depth_override.unwrap_or(config.color_depth);
    let chunk_size = (config.num_threads * 4).max(1);

    let mut pool_builder = rayon::ThreadPoolBuilder::new();
    if config.num_threads > 0 {
        pool_builder = pool_builder.num_threads(config.num_threads);
    }
    let pool = Arc::new(
        pool_builder
            .build()
            .map_err(|e| EgerError::Video(e.to_string()))?,
    );

    type ConversionHandle = tokio::task::JoinHandle<Result<Vec<String>>>;
    let mut pending: Option<ConversionHandle> = None;

    loop {
        let mut chunk = Vec::with_capacity(chunk_size);
        for _ in 0..chunk_size {
            let mut buf = vec![0u8; frame_bytes];
            match stdout.read_exact(&mut buf).await {
                Ok(_) => chunk.push(buf),
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e.into()),
            }
        }
        let is_last = chunk.len() < chunk_size;

        if let Some(handle) = pending.take() {
            let frames = handle.await??;
            if tx.send(Ok(frames)).await.is_err() {
                break; // Receiver was dropped by consumer
            }
        }

        if chunk.is_empty() {
            break;
        }

        let width = info.width;
        let height = info.height;
        let config = Arc::clone(&config);
        let pool = Arc::clone(&pool);
        pending = Some(tokio::task::spawn_blocking(
            move || -> Result<Vec<String>> {
                pool.install(|| {
                    chunk
                        .into_par_iter()
                        .map(|raw| {
                            let grid = convert_image(width, height, &raw, &config.ascii)?;
                            Ok(render::grid_to_string(&grid, Some(color_depth)))
                        })
                        .collect::<Result<Vec<String>>>()
                })
            },
        ));

        if is_last {
            break;
        }
    }

    if let Some(handle) = pending.take() {
        let frames = handle.await??;
        let _ = tx.send(Ok(frames)).await;
    }

    let status = child.wait().await?;
    if !status.success() {
        return Err(EgerError::Ffmpeg(
            status,
            "ffmpeg frame extraction failed".into(),
        ));
    }

    Ok(())
}

/// Renders all video frames into memory.
///
/// **Warning:** Prefer [`video_to_file`], [`play_video`], or [`stream_video_frames`]
/// for large files to prevent OOM errors.
pub async fn render_video_frames(
    path: &Path,
    config: Arc<Config>,
    depth_override: Option<ColorDepth>,
) -> Result<Vec<String>> {
    let (tx, mut rx) = mpsc::channel(4);
    let path_buf = path.to_path_buf();
    let stream_task =
        tokio::spawn(
            async move { stream_video_frames(&path_buf, config, depth_override, tx).await },
        );

    let mut rendered = Vec::new();
    while let Some(chunk_res) = rx.recv().await {
        let chunk = chunk_res?;
        rendered.extend(chunk);
    }

    stream_task.await??;
    Ok(rendered)
}

pub async fn video_to_lines(path: &Path, config: Arc<Config>) -> Result<Vec<String>> {
    render_video_frames(path, config, None).await
}

/// Streams and writes ASCII frame chunks directly to an output file using a buffered writer.
pub async fn video_to_file(
    path: &Path,
    config: Arc<Config>,
    output: Option<&Path>,
) -> Result<PathBuf> {
    let output = output
        .map(Path::to_path_buf)
        .or_else(|| config.output.clone())
        .ok_or_else(|| EgerError::Render("no output path provided or configured".into()))?;

    let file = tokio::fs::File::create(&output).await?;
    let mut writer = BufWriter::new(file);

    let (tx, mut rx) = mpsc::channel(4);
    let path_buf = path.to_path_buf();
    let stream_task =
        tokio::spawn(async move { stream_video_frames(&path_buf, config, None, tx).await });

    let mut first = true;
    while let Some(chunk_res) = rx.recv().await {
        let chunk = chunk_res?;
        for frame in chunk {
            if !first {
                writer.write_all(b"\n\x1E\n").await?;
            }
            first = false;
            writer.write_all(frame.as_bytes()).await?;
        }
    }
    writer.flush().await?;

    stream_task.await??;
    Ok(output)
}

/// Streams frames directly to stdout for terminal playback without buffering the whole file in RAM.
pub async fn play_video(path: &Path, config: Arc<Config>) -> Result<()> {
    let info = probe(path).await?;
    if !(info.fps > 0.0) {
        return Err(EgerError::Render("fps must be positive".into()));
    }
    let frame_delay = std::time::Duration::from_secs_f64(1.0 / info.fps);
    let mut stdout = tokio::io::stdout();

    let (tx, mut rx) = mpsc::channel(4);
    let path_buf = path.to_path_buf();
    let stream_task =
        tokio::spawn(async move { stream_video_frames(&path_buf, config, None, tx).await });

    while let Some(chunk_res) = rx.recv().await {
        let chunk = chunk_res?;
        for frame in chunk {
            stdout.write_all(b"\x1B[H\x1B[2J").await?;
            stdout.write_all(frame.as_bytes()).await?;
            stdout.flush().await?;
            tokio::time::sleep(frame_delay).await;
        }
    }

    stream_task.await??;
    Ok(())
}
