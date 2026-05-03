use std::{path::Path, sync::mpsc::Sender};

use anyhow::Context;
use ffmpeg_next as ffmpeg;

#[derive(Debug)]
pub struct Frame {
    width: u32,
    height: u32,
}

pub fn run(path: &Path, tx: Sender<Frame>) -> anyhow::Result<()> {
    ffmpeg::init()?;
    ffmpeg::log::set_level(ffmpeg::log::Level::Warning);

    loop {
        match decode_file(path, &tx) {
            Ok(()) => log::debug!("Video EOF — looping"),
            Err(e) => return Err(e),
        }
    }
}

pub fn decode_file(path: &Path, tx: &Sender<Frame>) -> anyhow::Result<()> {
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
