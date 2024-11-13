use std::collections::HashMap;
use std::fmt::Display;
use std::path::PathBuf;

use anyhow::Result;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPacket;
use ffmpeg_rs_raw::{Encoder, Muxer};
use itertools::Itertools;
use log::info;
use uuid::Uuid;

use crate::egress::{Egress, EgressConfig};
use crate::variant::{StreamMapping, VariantStream};

pub struct HlsEgress {
    id: Uuid,
    config: EgressConfig,
    muxer: Muxer,
}

enum HlsMapEntry {
    Video(usize),
    Audio(usize),
    Subtitle(usize),
}

impl Display for HlsMapEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HlsMapEntry::Video(i) => write!(f, "v:{}", i),
            HlsMapEntry::Audio(i) => write!(f, "a:{}", i),
            HlsMapEntry::Subtitle(i) => write!(f, "s:{}", i),
        }
    }
}

struct HlsStream {
    name: String,
    entries: Vec<HlsMapEntry>,
}

impl Display for HlsStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{},name:{}", self.entries.iter().join(","), self.name)
    }
}

impl HlsEgress {
    pub fn new<'a>(
        config: EgressConfig,
        encoded: impl Iterator<Item = &'a Encoder>,
    ) -> Result<Self> {
        let id = Uuid::new_v4();
        let base = PathBuf::from(&config.out_dir).join(id.to_string());

        let mut opts = HashMap::new();
        opts.insert(
            "hls_segment_filename".to_string(),
            format!("{}/%v/%05d.ts", base.display()),
        );
        opts.insert("master_pl_name".to_string(), "live.m3u8".to_string());
        opts.insert("master_pl_publish_rate".to_string(), "10".to_string());
        opts.insert("hls_time".to_string(), "2".to_string());
        opts.insert("hls_flags".to_string(), "delete_segments".to_string());

        let muxer = unsafe {
            let mut m = Muxer::builder()
                .with_output_path(
                    base.join("%v/live.m3u8").to_str().unwrap(),
                    Some("hls"),
                    Some(opts),
                )?
                .build()?;
            for e in encoded {
                m.add_stream_encoder(e)?;
            }
            m.open()?;
            m
        };

        Ok(Self { id, config, muxer })
    }

    unsafe fn setup_hls_mapping<'a>(
        variants: impl Iterator<Item = &'a VariantStream>,
    ) -> Result<String> {
        // configure mapping
        let mut stream_map = Vec::new();
        for (g, vars) in &variants
            .sorted_by(|a, b| a.group_id().cmp(&b.group_id()))
            .group_by(|x| x.group_id())
        {
            let group = HlsStream {
                name: format!("stream_{}", g),
                entries: Vec::new(),
            };
            for var in vars {
                todo!("get nth stream");
                let n = 0;
                match var {
                    VariantStream::Video(_) => group.entries.push(HlsMapEntry::Video(n)),
                    VariantStream::Audio(_) => group.entries.push(HlsMapEntry::Audio(n)),
                    VariantStream::CopyVideo(_) => group.entries.push(HlsMapEntry::Video(n)),
                    VariantStream::CopyAudio(_) => group.entries.push(HlsMapEntry::Audio(n)),
                };
            }
            stream_map.push(group);
        }
        let stream_map = stream_map.iter().join(" ");

        info!("map_str={}", stream_map);

        Ok(stream_map)
    }
}

impl Egress for HlsEgress {
    unsafe fn process_pkt(&mut self, packet: *mut AVPacket, variant: &Uuid) -> Result<()> {
        if self.config.variants.contains(variant) {
            self.muxer.write_packet(packet)
        } else {
            Ok(())
        }
    }
}
