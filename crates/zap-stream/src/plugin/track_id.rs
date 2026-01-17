use crate::plugin::TrackIdMatch;
use anyhow::Result;
use chromaprint_sys_next::{
    ChromaprintContext, chromaprint_feed, chromaprint_free, chromaprint_get_fingerprint,
    chromaprint_new, chromaprint_start,
};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVSampleFormat;
use ffmpeg_rs_raw::{AvFrameRef, Resample};
use libc::strlen;
use std::ptr;
use std::sync::Mutex;
use std::time::Instant;
use tokio::sync::mpsc::UnboundedSender;
use tracing::warn;
use uuid::Uuid;
use zap_stream_core::pipeline::{
    ConfigurableEgress, PipelinePlugin, PipelinePluginConfigurationResult,
};

pub struct TrackIdPlugin {
    /// Unique plugin id
    id: Uuid,
    /// Chromaprint context
    ctx: *mut ChromaprintContext,
    /// How much audio data to accumulate before checking for a match
    sample_time: f32,
    /// Software resampler to get S16 samples for chromaprint
    resample: Mutex<Resample>,
    /// Channel to send track id matches to
    submissions: UnboundedSender<TrackIdMatch>,
    /// Last time a submission was sent
    last_submission: Instant,
}

unsafe impl Sync for TrackIdPlugin {}
unsafe impl Send for TrackIdPlugin {}

impl Drop for TrackIdPlugin {
    fn drop(&mut self) {
        if !self.ctx.is_null() {
            unsafe {
                chromaprint_free(self.ctx);
            }
        }
    }
}
impl TrackIdPlugin {
    pub const NAME: &'static str = "chromaprint";

    pub fn new(
        id: Uuid,
        sample_rate: u32,
        channels: u8,
        sample_time: f32,
        sender: UnboundedSender<TrackIdMatch>,
    ) -> Result<Self> {
        let ctx = unsafe { chromaprint_new(1) };
        Ok(Self {
            id,
            ctx,
            sample_time,
            resample: Mutex::new(Resample::new(
                AVSampleFormat::AV_SAMPLE_FMT_S16,
                sample_rate,
                channels as _,
            )),
            submissions: sender,
            last_submission: Instant::now(),
        })
    }
}
impl PipelinePlugin for TrackIdPlugin {
    fn id(&self) -> Uuid {
        self.id
    }

    fn process_frame(&self, frame: AvFrameRef) {
        let mut re = self.resample.lock().unwrap();
        let f = match re.process_frame(&frame) {
            Ok(f) => f,
            Err(e) => {
                warn!("Error processing frame: {}", e);
                return;
            }
        };

        if self.last_submission.elapsed().as_secs() > 10 {}

        let ret = unsafe { chromaprint_feed(self.ctx, f.data[0] as *const i16, f.nb_samples as _) };
        if ret == 0 {
            warn!("Error feeding frame");
            return;
        }

        let mut fingerprint = ptr::null_mut();
        let ret = unsafe { chromaprint_get_fingerprint(self.ctx, &mut fingerprint) };
        if ret == 0 {
            warn!("Error getting fingerprint");
            return;
        }

        unsafe {
            if !fingerprint.is_null() {
                let len = strlen(fingerprint);
                if let Err(e) = self.submissions.send(TrackIdMatch {
                    id: String::from_raw_parts(fingerprint as _, len, len),
                }) {
                    warn!("Error sending track id match: {}", e);
                }
            }
        }
    }

    fn get_frame(&self) -> Option<AvFrameRef> {
        // no output for this plugin
        None
    }

    fn configure_egress(
        &self,
        _e: ConfigurableEgress,
    ) -> Result<PipelinePluginConfigurationResult> {
        // doest modify egress'
        Ok(PipelinePluginConfigurationResult::default())
    }
}
