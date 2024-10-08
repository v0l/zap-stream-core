use crate::fraction::Fraction;
use ffmpeg_sys_next::AVFormatContext;
use std::fmt::{Display, Formatter};

#[derive(Clone, Debug, PartialEq)]
pub struct DemuxerInfo {
    pub channels: Vec<StreamInfoChannel>,
    pub ctx: *const AVFormatContext,
}

unsafe impl Send for DemuxerInfo {}
unsafe impl Sync for DemuxerInfo {}

impl Display for DemuxerInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Demuxer Info:")?;
        for c in &self.channels {
            write!(f, "\n{}", c)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum StreamChannelType {
    Video,
    Audio,
}

impl Display for StreamChannelType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                StreamChannelType::Video => "video",
                StreamChannelType::Audio => "audio",
            }
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StreamInfoChannel {
    pub index: usize,
    pub channel_type: StreamChannelType,
    pub width: usize,
    pub height: usize,
    pub fps: f32,
    pub format: usize,
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

impl Display for StreamInfoChannel {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} #{}: size={}x{},fps={}",
            self.channel_type, self.index, self.width, self.height, self.fps
        )
    }
}
