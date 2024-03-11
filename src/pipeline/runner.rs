use crate::pipeline::{PipelinePayload, PipelineStep};

pub struct PipelineRunner {
    steps: Vec<Box<dyn PipelineStep + Sync + Send>>,
}

impl PipelineRunner {
    pub fn new(steps: Vec<Box<dyn PipelineStep + Sync + Send>>) -> Self {
        Self { steps }
    }

    pub async fn push(&mut self, bytes: bytes::Bytes) -> Result<(), anyhow::Error> {
        let mut output = PipelinePayload::Bytes(bytes);
        for step in &mut self.steps {
            let output2 = step.process(output).await?;
            //info!("{} result: {:?}", step.name(), output2);
            output = output2;
        }
        Ok(())
    }
}
