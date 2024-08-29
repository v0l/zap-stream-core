use anyhow::Error;
use tokio::sync::mpsc::UnboundedSender;

use crate::ipc::Rx;
use crate::pipeline::{AVFrameSource, PipelinePayload, PipelineProcessor};
use crate::variant::{VariantStream, VariantStreamType};

pub struct TagFrame<TRecv> {
    variant: VariantStream,
    chan_in: TRecv,
    chan_out: UnboundedSender<PipelinePayload>,
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
        Self {
            variant: var,
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
            if let PipelinePayload::AvFrame(_, src) = &pkg {
                let idx = match &src {
                    AVFrameSource::Decoder(s) => unsafe { (**s).index },
                    _ => return Err(Error::msg(format!("Cannot process frame from: {:?}", src))),
                };
                if self.variant.src_index() == idx as usize {
                    self.chan_out.send(pkg)?;
                }
            }
        }
        Ok(())
    }
}
