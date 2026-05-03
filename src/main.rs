use std::{
    path::Path,
    sync::mpsc::{self, Sender},
};

use anyhow::Context;
use ffmpeg_next as ffmpeg;

#[derive(Debug)]
struct Frame {
    width: u32,
    height: u32,
}

const WALLPAPER_PATH: &str = "/home/plo/dotfiles/wallpapers/animated/blue-porsche.mp4";

fn main() -> anyhow::Result<()> {
    env_logger::init();

    ffmpeg::init()?;
    ffmpeg::log::set_level(ffmpeg::log::Level::Warning);

    let (tx, rx) = mpsc::channel();

    std::thread::Builder::new()
        .name("decoder".into())
        .spawn(move || {
            if let Err(e) = run(Path::new(&WALLPAPER_PATH), tx) {
                log::error!("decoder error: {e:#}");
            }
        })?;

    loop {
        let frame = rx.recv().context("receive decoded frame")?;
        println!("{:?}", frame);
    }
}

fn run(path: &Path, tx: Sender<Frame>) -> anyhow::Result<()> {
    loop {
        match decode_file(path, &tx) {
            Ok(()) => log::debug!("Video EOF — looping"),
            Err(e) => return Err(e),
        }
    }
}

fn decode_file(path: &Path, tx: &Sender<Frame>) -> anyhow::Result<()> {
    let mut input =
        ffmpeg::format::input(path).with_context(|| format!("Open {}", path.display()))?;

    let stream = input
        .streams()
        .best(ffmpeg::media::Type::Video)
        .context("No video stream in file")?;

    let stream_idx = stream.index();

    let context_decoder =
        ffmpeg::codec::Context::from_parameters(stream.parameters()).context("Codec context")?;

    let mut decoder = context_decoder.decoder().video().context("Open decoder")?;

    let width = decoder.width();
    let height = decoder.height();

    for (stream, packet) in input.packets() {
        if stream.index() != stream_idx {
            continue;
        }

        decoder.send_packet(&packet).context("send_packet")?;
        let mut decoded = ffmpeg::frame::Video::empty();

        while decoder.receive_frame(&mut decoded).is_ok() {
            tx.send(Frame { width, height })?;
        }
    }

    decoder.send_eof()?;
    Ok(())
}
