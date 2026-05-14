use gst::MessageView::*;
use std::path::Path;
use std::sync::mpsc::SyncSender;

use anyhow::{Context, anyhow};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_gl as gst_gl;
use gstreamer_gl::prelude::*;
use gstreamer_gl_egl as gst_gl_egl;
use gstreamer_video as gst_video;

pub struct Frame {
    pub texture_id: u32,
}

pub fn run(
    path: &Path,
    gl_display: gst_gl::GLDisplay,
    gl_context: gst_gl::GLContext,
    tx: SyncSender<gst::Sample>,
) -> anyhow::Result<()> {
    let pipeline_desc = format!(
        "filesrc location=\"{}\" ! decodebin3 ! glupload ! glcolorconvert ! \
         appsink name=sink caps=video/x-raw(memory:GLMemory),format=RGBA,texture-target=2D \
         sync=true max-buffers=1",
        path.display()
    );

    let pipeline = gst::parse::launch(&pipeline_desc)
        .context("parse pipeline")?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow!("parsed element is not a Pipeline"))?;

    let mut display_ctx = gst::Context::new("gst.gl.GLDisplay", true);
    display_ctx
        .get_mut()
        .expect("freshly created Context has a unique reference")
        .set_gl_display(&gl_display);

    pipeline.set_context(&display_ctx);

    let mut app_ctx = gst::Context::new("gst.gl.app_context", true);
    app_ctx
        .get_mut()
        .expect("freshly created Context has a unique reference")
        .structure_mut()
        .set("context", &gl_context);

    pipeline.set_context(&app_ctx);

    let appsink = pipeline
        .by_name("sink")
        .context("appsink not found in pipeline")?
        .downcast::<gst_app::AppSink>()
        .map_err(|_| anyhow!("`sink` is not an AppSink"))?;

    appsink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                tx.send(sample).map_err(|_| gst::FlowError::Eos)?;
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );

    pipeline
        .set_state(gst::State::Playing)
        .context("set pipeline state to Playing")?;

    let bus = pipeline.bus().context("pipeline has no bus")?;
    for msg in bus.iter_timed(gst::ClockTime::NONE) {
        match msg.view() {
            Eos(_) => {
                pipeline
                    .seek_simple(
                        gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
                        gst::ClockTime::ZERO,
                    )
                    .context("seek to loop")?;
            }
            Error(err) => {
                pipeline.set_state(gst::State::Null).ok();
                return Err(anyhow!(
                    "pipeline error from {:?}: {} ({:?})",
                    err.src().map(|s| s.path_string()),
                    err.error(),
                    err.debug()
                ));
            }
            _ => {}
        }
    }

    pipeline.set_state(gst::State::Null).ok();
    Ok(())
}

pub fn sample_to_frame(
    sample: gst::Sample,
    gl_context: &gst_gl::GLContext,
) -> anyhow::Result<Frame> {
    let buffer = sample.buffer_owned().context("sample has no buffer")?;
    let caps = sample.caps().context("sample has no caps")?;
    let info = gst_video::VideoInfo::from_caps(caps).context("VideoInfo from caps")?;

    if let Some(sync_meta) = buffer.meta::<gst_gl::GLSyncMeta>() {
        sync_meta.wait(gl_context);
    }

    let gl_frame = gst_gl::GLVideoFrame::from_buffer_readable(buffer, &info)
        .map_err(|_| anyhow!("GLVideoFrame::from_buffer_readable failed"))?;

    let texture_id = gl_frame
        .texture_id(0)
        .context("GLVideoFrame texture id missing")?;

    Ok(Frame { texture_id })
}

pub fn wrap_gl(
    egl_display: usize,
    egl_context: usize,
) -> anyhow::Result<(gst_gl::GLDisplay, gst_gl::GLContext)> {
    gst::init().context("gst::init")?;

    let gl_display = unsafe { gst_gl_egl::GLDisplayEGL::with_egl_display(egl_display) }
        .context("wrap EGL display for GStreamer")?;

    let gl_display = gl_display.upcast::<gst_gl::GLDisplay>();

    let gl_context = unsafe {
        gst_gl::GLContext::new_wrapped(
            &gl_display,
            egl_context,
            gst_gl::GLPlatform::EGL,
            gst_gl::GLAPI::GLES2,
        )
    }
    .context("wrap EGL context for GStreamer")?;

    gl_context
        .activate(true)
        .map_err(|_| anyhow!("GLContext::activate failed"))?;

    gl_context
        .fill_info()
        .map_err(|e| anyhow!("GLContext::fill_info: {e}"))?;

    Ok((gl_display, gl_context))
}
