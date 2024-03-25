use crate::fraction::Fraction;

#[derive(Clone, Debug, PartialEq)]
pub struct DemuxStreamInfo {
    pub channels: Vec<StreamInfoChannel>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StreamChannelType {
    Video,
    Audio,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StreamInfoChannel {
    pub index: usize,
    pub channel_type: StreamChannelType,
    pub width: usize,
    pub height: usize,
    pub fps: f32,
}

impl TryInto<Fraction> for StreamInfoChannel {
    type Error = ();

    fn try_into(self) -> Result<Fraction, Self::Error> {
        if self.channel_type == StreamChannelType::Video {
            Ok(Fraction::from((self.width, self.height)))
        } else {
            Err(())
        }
    }
}
