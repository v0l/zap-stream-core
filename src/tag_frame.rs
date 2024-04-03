use anyhow::Error;
use ffmpeg_sys_next::{av_buffer_ref, AVBufferRef};
use tokio::sync::mpsc::UnboundedSender;

use crate::ipc::Rx;
use crate::pipeline::{PipelinePayload, PipelineProcessor};
use crate::utils::variant_id_ref;
use crate::variant::{VariantStream, VariantStreamType};

pub struct TagFrame<TRecv> {
    variant: VariantStream,
    chan_in: TRecv,
    chan_out: UnboundedSender<PipelinePayload>,
    var_id_ref: *mut AVBufferRef,
}

unsafe impl<T> Send for TagFrame<T> {}

unsafe impl<T> Sync for TagFrame<T> {}

impl<TRecv> TagFrame<TRecv>
where
    TRecv: Rx<PipelinePayload>,
{
    pub fn new(
        var: VariantStream,
        chan_in: TRecv,
        chan_out: UnboundedSender<PipelinePayload>,
    ) -> Self {
        let id_ref = variant_id_ref(&var).unwrap();
        Self {
            variant: var,
            var_id_ref: id_ref,
            chan_in,
            chan_out,
        }
    }
}

impl<TRecv> PipelineProcessor for TagFrame<TRecv>
where
    TRecv: Rx<PipelinePayload>,
{
    fn process(&mut self) -> Result<(), Error> {
        while let Ok(pkg) = self.chan_in.try_recv_next() {
            if let PipelinePayload::AvFrame(_, pkt, idx) = &pkg {
                if *idx == self.variant.src_index() {
                    unsafe {
                        (**pkt).opaque_ref = av_buffer_ref(self.var_id_ref);
                    }
                    self.chan_out.send(pkg)?;
                }
            }
        }
        Ok(())
    }
}
