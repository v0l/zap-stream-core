use crate::cors::CORS;
use anyhow::{Result, bail};
use clap::Parser;
use itertools::Itertools;
use m3u8_rs::{
    MasterPlaylist, MediaPlaylist, MediaSegment, MediaSegmentType, Resolution, VariantStream,
};
use nostr_sdk::{
    Alphabet, Client, Event, EventId, Filter, Kind, RelayPoolNotification, SingleLetterTag,
    SubscribeAutoCloseOptions, SubscribeOptions, TagKind, Timestamp,
};
use rocket::http::Status;
use rocket::shield::Shield;
use rocket::{State, routes};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt::{Display, Formatter};
use std::net::SocketAddr;
use std::ops::Sub;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

mod cors;

type StreamList = Arc<RwLock<HashMap<EventId, Stream>>>;

#[derive(Parser)]
struct Args {
    /// Relays to load stream events from
    #[clap(short, long, default_values_t = [
        "wss://relay.snort.social".to_string(),
        "wss://relay.damus.io".to_string(),
        "wss://relay.primal.net".to_string(),
        "wss://nos.lol".to_string()
    ])]
    pub relay: Vec<String>,

    /// Listen address for HTTP server
    #[clap(short, long, default_value = "0.0.0.0:8000")]
    pub listen: String,
}

#[derive(Debug, Clone)]
struct Stream {
    /// The stream event from nostr
    pub event: Event,
    /// Last time the playlist was updated
    pub last_hit: u64,
    /// List of all stream variants
    pub variants: HashMap<String, StreamVariant>,
}

#[derive(Debug, Clone, Default)]
struct StreamVariant {
    pub id: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bitrate: Option<u32>,
    pub mime: Option<String>,
    pub segments: Vec<StreamSegment>,
}

impl Display for StreamVariant {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "id={}, width={}, height={}, bitrate={}",
            self.id,
            self.width.unwrap_or(0),
            self.height.unwrap_or(0),
            self.bitrate.unwrap_or(0)
        )
    }
}

#[derive(Debug, Clone)]
struct StreamSegment {
    pub index: u64,
    pub duration: f32,
    pub url: String,
    pub expires: Option<u64>,
}

#[rocket::get("/")]
async fn index(
    streams: &State<StreamList>,
) -> Result<rocket::response::content::RawHtml<String>, Status> {
    let streams_guard = streams.read().await;
    let mut html = String::from(
        r#"<!DOCTYPE html>
<html>
<head>
    <title>N94 Bridge - Stream List</title>
    <style>
        body { 
            font-family: Arial, sans-serif; 
            margin: 40px; 
            background-color: #1a1a1a; 
            color: #e0e0e0; 
        }
        h1 { color: #ffffff; }
        h2 { color: #ffffff; margin-top: 30px; }
        .description { 
            background: #2d2d2d; 
            padding: 20px; 
            border-radius: 8px; 
            margin: 20px 0; 
            border-left: 4px solid #4a9eff; 
        }
        .stream { 
            border: 1px solid #444; 
            margin: 10px 0; 
            padding: 15px; 
            border-radius: 5px; 
            background: #2d2d2d; 
        }
        .stream-id { 
            font-family: monospace; 
            color: #888; 
            font-size: 0.9em; 
        }
        .variant { 
            margin: 5px 0 5px 20px; 
            padding: 8px; 
            background: #3a3a3a; 
            border-radius: 3px; 
        }
        .playlist-link { 
            color: #4a9eff; 
            text-decoration: none; 
        }
        .playlist-link:hover { 
            text-decoration: underline; 
            color: #66b3ff; 
        }
        .no-streams { 
            color: #888; 
            font-style: italic; 
        }
    </style>
</head>
<body>
    <h1>N94 Bridge - Active Streams</h1>
    <div class="description">
        <h2>About N94 Bridge</h2>
        <p>N94 Bridge is a Nostr-based streaming bridge service that aggregates live stream events from multiple Nostr relays and provides HLS (HTTP Live Streaming) playlist access. The service:</p>
        <ul>
            <li>Monitors Nostr relays for stream events (kind 1053) and segment metadata (kind 1063)</li>
            <li>Automatically tracks stream variants with different bitrates and resolutions</li>
            <li>Generates master and variant HLS playlists for seamless video playback</li>
            <li>Provides a real-time web interface to browse active streams</li>
            <li>Handles stream cleanup and expiration automatically</li>
        </ul>
        <p>Connect your media player to the playlist URLs below to start watching live streams from the Nostr network.</p>
    </div>
    <h2>Active Streams</h2>"#,
    );

    if streams_guard.is_empty() {
        html.push_str(r#"    <p class="no-streams">No active streams found.</p>"#);
    } else {
        html.push_str(&format!(
            "<p>Found {} active stream(s):</p>",
            streams_guard.len()
        ));

        for (event_id, stream) in streams_guard.iter() {
            html.push_str(&format!(
                r#"    <div class="stream">
        <div class="stream-id">Stream ID: {}</div>
        <p><strong>Last Hit:</strong> {}</p>
        <p><strong>Variants:</strong></p>"#,
                event_id, stream.last_hit
            ));

            if stream.variants.is_empty() {
                html.push_str(r#"        <div class="variant">No variants available</div>"#);
            } else {
                html.push_str(&format!(
                    r#"        <div class="variant">
            <a class="playlist-link" href="/{}.m3u8" target="_blank">📺 Master Playlist</a>
            <br><strong>Available Resolutions:</strong> {}</div>"#,
                    event_id,
                    stream
                        .variants
                        .values()
                        .map(|v| format!(
                            "{}x{} @ {}kbps",
                            v.width.unwrap_or(0),
                            v.height.unwrap_or(0),
                            v.bitrate.unwrap_or(0) / 1000
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }

            html.push_str("    </div>");
        }
    }

    html.push_str(
        r#"
</body>
</html>"#,
    );

    Ok(rocket::response::content::RawHtml(html))
}

#[rocket::get("/<event>/<variant>")]
async fn variant_playlist(
    event: &str,
    variant: &str,
    streams: &State<StreamList>,
) -> Result<Vec<u8>, Status> {
    let id = if let Some(x) = event.split('.').next().and_then(|i| EventId::parse(i).ok()) {
        x
    } else {
        return Err(Status::BadRequest);
    };
    let variant = if let Some(x) = variant.split('.').next() {
        x
    } else {
        return Err(Status::BadRequest);
    };
    if let Some(stream) = streams.read().await.get(&id) {
        let variant = if let Some(v) = stream.variants.values().find(|v| v.id == variant) {
            v
        } else {
            return Err(Status::NotFound);
        };

        let mut var_pl = MediaPlaylist::default();
        let active_segments = variant
            .segments
            .iter()
            .filter(|a| {
                if let Some(e) = a.expires {
                    e > Timestamp::now().as_u64()
                } else {
                    true
                }
            })
            .collect::<Vec<_>>();
        var_pl.version = Some(3);
        var_pl.media_sequence = active_segments
            .iter()
            .min_by_key(|a| a.index)
            .map(|a| a.index)
            .unwrap_or(0);
        var_pl.target_duration = active_segments
            .iter()
            .last()
            .map(|a| a.duration)
            .unwrap_or(1.0); // TODO: maybe average the duration
        for seg in active_segments.into_iter().sorted_by_key(|a| a.index) {
            var_pl.segments.push(MediaSegmentType::Full(MediaSegment {
                uri: seg.url.clone(),
                duration: seg.duration,
                title: None,
                byte_range: None,
                discontinuity: false,
                key: None,
                map: None,
                program_date_time: None,
                daterange: None,
                unknown_tags: vec![],
            }));
        }

        let mut ret = Vec::new();
        if let Err(e) = var_pl.write_to(&mut ret) {
            error!("Failed to write media playlist {} {}", variant.id, e);
            return Err(Status::InternalServerError);
        }
        return Ok(ret);
    }

    Err(Status::NotFound)
}

#[rocket::get("/<event>")]
async fn master_playlist(
    event: &str,
    _client: &State<Client>,
    streams: &State<StreamList>,
) -> Result<Vec<u8>, Status> {
    let id = if let Some(x) = event.split('.').next().and_then(|i| EventId::parse(i).ok()) {
        x
    } else {
        return Err(Status::BadRequest);
    };

    if let Some(stream) = streams.read().await.get(&id) {
        // return master playlist
        let mut pl = MasterPlaylist::default();
        pl.version = Some(3);
        for var in stream.variants.values() {
            pl.variants.push(VariantStream {
                uri: format!("{}/{}.m3u8", id, var.id),
                bandwidth: var.bitrate.unwrap_or(0) as _,
                resolution: if var.width.is_some() && var.height.is_some() {
                    Some(Resolution {
                        width: var.width.unwrap() as _,
                        height: var.height.unwrap() as _,
                    })
                } else {
                    None
                },
                ..Default::default()
            });
        }

        let mut ret = Vec::new();
        if let Err(e) = pl.write_to(&mut ret) {
            error!("Failed to write master playlist: {:?}", e);
            return Err(Status::InternalServerError);
        }
        return Ok(ret);
    } else {
        // try to fetch if not already in cache
    }
    Err(Status::NotFound)
}

// process stream / segment event
async fn process_event(event: Event, client: Client, streams: StreamList) -> Result<()> {
    match event.kind {
        // stream event
        Kind::Custom(1053) => {
            let id = event.id;
            match streams.write().await.entry(id) {
                Entry::Occupied(_) => {
                    // skip
                }
                Entry::Vacant(k) => {
                    info!("Tracking stream: {}", k.key());
                    let mut vars = HashMap::new();
                    for var_tag in event.tags.filter(TagKind::Custom("variant".into())) {
                        let mut var = StreamVariant::default();
                        for (k, v) in var_tag
                            .as_slice()
                            .iter()
                            .skip(1)
                            .map_while(|s| s.split_once(" "))
                        {
                            match k {
                                "d" => var.id = v.to_string(),
                                "m" => var.mime = Some(v.to_string()),
                                "bitrate" => var.bitrate = v.parse().ok(),
                                "dim" => {
                                    let (w, h) = v.split_once("x").unwrap();
                                    var.width = Some(w.parse()?);
                                    var.height = Some(h.parse()?);
                                }
                                _ => {}
                            }
                        }
                        if !var.id.is_empty() {
                            info!("  - {}", var);
                            vars.insert(var.id.clone(), var);
                        } else {
                            warn!(
                                "Variant tag has no identifier, skipping: {}",
                                var_tag.content().unwrap_or("")
                            );
                        }
                    }
                    // try preload segments with subscription
                    client
                        .subscribe(
                            Filter::new().kind(Kind::FileMetadata).event(event.id),
                            Some(SubscribeAutoCloseOptions::default()),
                        )
                        .await?;

                    k.insert(Stream {
                        event,
                        last_hit: Timestamp::now().as_u64(),
                        variants: vars,
                    });
                }
            }
        }
        // segment
        Kind::FileMetadata => {
            let stream_id = if let Some(i) = event.tags.event_ids().next() {
                i
            } else {
                warn!("Segment event {} had no e tag!", event.id);
                return Ok(());
            };
            let var_id = if let Some(i) = event
                .tags
                .find(TagKind::SingleLetter(SingleLetterTag::lowercase(
                    Alphabet::D,
                )))
                .and_then(|t| t.content())
            {
                i.to_string()
            } else {
                warn!("Segment event {} had no d tag!", event.id);
                return Ok(());
            };
            let index = if let Some(i) = event
                .tags
                .find(TagKind::Custom("index".into()))
                .and_then(|t| t.content())
                .and_then(|i| i.parse::<u64>().ok())
            {
                i
            } else {
                warn!("Segment event {} had no index tag!", event.id);
                return Ok(());
            };
            let duration = if let Some(i) = event
                .tags
                .find(TagKind::Custom("duration".into()))
                .and_then(|t| t.content())
                .and_then(|i| i.parse::<f32>().ok())
            {
                i
            } else {
                warn!("Segment event {} had no index tag!", event.id);
                return Ok(());
            };
            let url = if let Some(i) = event.tags.find(TagKind::Url).and_then(|t| t.content()) {
                i.to_string()
            } else {
                warn!("Segment event {} had no url tag!", event.id);
                return Ok(());
            };
            let expires = event
                .tags
                .find(TagKind::Expiration)
                .and_then(|t| t.content())
                .and_then(|t| t.parse::<u64>().ok());

            if let Some(stream) = streams.write().await.get_mut(stream_id) {
                stream.last_hit = Timestamp::now().as_u64();
                match stream.variants.entry(var_id.clone()) {
                    Entry::Occupied(e) => {
                        debug!(
                            "Inserting segment {} into variant {} in stream {}",
                            url, var_id, stream.event.id
                        );
                        e.into_mut().segments.push(StreamSegment {
                            index,
                            url,
                            duration,
                            expires,
                        });
                    }
                    Entry::Vacant(_) => {
                        warn!("Unknown variant {} in stream {}", var_id, stream_id);
                    }
                }
            }
        }
        _ => warn!("Unsupported event kind: {}", event.kind.as_u16()),
    }
    Ok(())
}

#[rocket::main]
async fn main() -> Result<()> {
    if std::env::var("RUST_LOG").is_err() {
        unsafe {
            std::env::set_var("RUST_LOG", "info");
        }
    }
    pretty_env_logger::init();

    let args: Args = Args::parse();

    let mut config = rocket::Config::default();
    let ip: SocketAddr = match args.listen.parse() {
        Ok(i) => i,
        Err(e) => bail!(e),
    };
    config.address = ip.ip();
    config.port = ip.port();

    let client = Client::builder().build();
    for r in &args.relay {
        client.add_relay(r).await?;
        info!("Connected to relay: {}", r);
    }
    client.connect().await;

    let events: StreamList = Default::default();

    // listen to all stream events
    let listener = client.clone();
    let l_events = events.clone();
    tokio::spawn(async move {
        let mut n = listener.notifications();
        while let Ok(msg) = n.recv().await {
            match msg {
                RelayPoolNotification::Event { event, .. } => {
                    if let Err(e) = process_event(*event, listener.clone(), l_events.clone()).await
                    {
                        error!("Failed to process event: {}", e);
                    }
                }
                RelayPoolNotification::Message { .. } => {}
                RelayPoolNotification::Shutdown => {}
            }
        }
    });

    // spawn stream cleanup
    let c_events = events.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            let to_remove = c_events
                .read()
                .await
                .values()
                .filter(|s| s.last_hit < Timestamp::now().sub(Duration::from_secs(60)).as_u64())
                .map(|v| v.event.id)
                .collect::<Vec<_>>();
            if !to_remove.is_empty() {
                info!("Cleaning up {} expired events", to_remove.len());
                let mut w_lock = c_events.write().await;
                for rem in to_remove {
                    w_lock.remove(&rem);
                }
            }
        }
    });

    // listen for all stream events
    client
        .pool()
        .subscribe(
            Filter::new().kind(Kind::Custom(1053)).limit(10),
            SubscribeOptions::default(),
        )
        .await?;
    // listen for all stream segments
    client
        .pool()
        .subscribe(
            Filter::new()
                .kind(Kind::FileMetadata)
                .custom_tag(SingleLetterTag::lowercase(Alphabet::K), "1053")
                .limit(10),
            SubscribeOptions::default(),
        )
        .await?;
    let rocket = rocket::Rocket::custom(config)
        .manage(client)
        .manage(events)
        .attach(CORS)
        .attach(Shield::new()) // disable
        .mount("/", routes![index, master_playlist, variant_playlist]);

    rocket.launch().await?;
    Ok(())
}
