use std::path::Path;
use std::sync::mpsc::SyncSender;

use anyhow::Context;
use ffmpeg_next as ffmpeg;
use ffmpeg_next::software::scaling::{Context as Scaler, Flags};
use ffmpeg_next::util::format::Pixel;

#[derive(Debug)]
pub struct Frame {
    pub data: Vec<u8>, // RGBA, tightly packed, width * height * 4 bytes
    pub width: u32,
    pub height: u32,
}

pub fn run(
    path: &Path,
    tx: SyncSender<Frame>,
    target_width: u32,
    target_height: u32,
) -> anyhow::Result<()> {
    ffmpeg::init()?;
    ffmpeg::log::set_level(ffmpeg::log::Level::Warning);

    loop {
        match decode_file(path, &tx, target_width, target_height) {
            Ok(()) => log::debug!("Video EOF — looping"),
            Err(e) => return Err(e),
        }
    }
}

fn decode_file(
    path: &Path,
    tx: &SyncSender<Frame>,
    target_width: u32,
    target_height: u32,
) -> anyhow::Result<()> {
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

    let mut scaler = Scaler::get(
        decoder.format(),
        decoder.width(),
        decoder.height(),
        Pixel::RGBA,
        target_width,
        target_height,
        Flags::BILINEAR,
    )
    .context("Create scaler")?;

    for (stream, packet) in input.packets() {
        if stream.index() != stream_idx {
            continue;
        }
        decoder.send_packet(&packet).context("send_packet")?;
        let mut decoded = ffmpeg::frame::Video::empty();

        while decoder.receive_frame(&mut decoded).is_ok() {
            let mut scaled = ffmpeg::frame::Video::empty();
            scaler.run(&decoded, &mut scaled).context("scale frame")?;

            // Copy row-by-row to strip any stride padding
            let linesize = scaled.stride(0);
            let row_bytes = target_width as usize * 4;
            let src = scaled.data(0);
            let mut data = vec![0u8; row_bytes * target_height as usize];
            for row in 0..target_height as usize {
                let src_row = &src[row * linesize..row * linesize + row_bytes];
                data[row * row_bytes..row * row_bytes + row_bytes].copy_from_slice(src_row);
            }

            tx.send(Frame {
                data,
                width: target_width,
                height: target_height,
            })?;
        }
    }

    decoder.send_eof()?;
    Ok(())
}
