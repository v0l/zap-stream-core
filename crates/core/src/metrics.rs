use prometheus::{
    Encoder, Gauge, Histogram, HistogramOpts, HistogramVec, Opts, Registry, TextEncoder,
};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, warn};

/// Global metrics registry
static METRICS: OnceLock<PipelineMetrics> = OnceLock::new();

/// Pipeline metrics collection
pub struct PipelineMetrics {
    /// Histogram tracking thumbnail generation time in seconds
    pub thumbnail_generation_time: Histogram,
    /// Histogram tracking playback rate as fraction of target FPS (with pipeline_id label)
    pub playback_rate: HistogramVec,
    /// Histogram tracking block_on duration for on_thumbnail calls
    pub block_on_thumbnail: Histogram,
    /// Histogram tracking block_on duration for handle_egress_results calls
    pub block_on_egress_results: Histogram,
    /// Histogram tracking block_on duration for start_stream calls
    pub block_on_start_stream: Histogram,
    /// Histogram tracking block_on duration for RTMP forwarder connect calls
    pub block_on_rtmp_connect: Histogram,
    /// Histogram tracking block_on duration for MoQ origin calls
    pub block_on_moq_origin: Histogram,
    /// Gauge tracking total number of viewers across all streams
    pub total_viewers: Gauge,
}

impl PipelineMetrics {
    /// Create new pipeline metrics and register with the given registry
    pub fn new(registry: &Registry) -> prometheus::Result<Self> {
        let thumbnail_generation_time = Histogram::with_opts(
            HistogramOpts::new(
                "thumbnail_generation_seconds",
                "Time taken to generate thumbnails in seconds",
            )
            .buckets(vec![0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0]),
        )?;

        // Playback rate histogram - buckets from 0 to 2 (0% to 200% of target FPS)
        let playback_rate = HistogramVec::new(
            HistogramOpts::new(
                "pipeline_playback_rate",
                "Playback rate as fraction of target FPS (1.0 = 100%)",
            )
            .buckets(vec![
                0.0, 0.25, 0.5, 0.75, 0.9, 0.95, 1.0, 1.05, 1.1, 1.25, 1.5, 2.0,
            ]),
            &["pipeline_id"],
        )?;

        // Block-on duration buckets (in seconds) - from 1ms to 10s
        let block_on_buckets = vec![
            0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
        ];

        let block_on_thumbnail = Histogram::with_opts(
            HistogramOpts::new(
                "pipeline_block_on_thumbnail_seconds",
                "Duration of block_on calls for on_thumbnail",
            )
            .buckets(block_on_buckets.clone()),
        )?;

        let block_on_egress_results = Histogram::with_opts(
            HistogramOpts::new(
                "pipeline_block_on_egress_results_seconds",
                "Duration of block_on calls for handle_egress_results",
            )
            .buckets(block_on_buckets.clone()),
        )?;

        let block_on_start_stream = Histogram::with_opts(
            HistogramOpts::new(
                "pipeline_block_on_start_stream_seconds",
                "Duration of block_on calls for start_stream",
            )
            .buckets(block_on_buckets.clone()),
        )?;

        let block_on_rtmp_connect = Histogram::with_opts(
            HistogramOpts::new(
                "pipeline_block_on_rtmp_connect_seconds",
                "Duration of block_on calls for RTMP forwarder connect",
            )
            .buckets(block_on_buckets.clone()),
        )?;

        let block_on_moq_origin = Histogram::with_opts(
            HistogramOpts::new(
                "pipeline_block_on_moq_origin_seconds",
                "Duration of block_on calls for MoQ origin",
            )
            .buckets(block_on_buckets),
        )?;

        let total_viewers = Gauge::with_opts(Opts::new(
            "total_viewers",
            "Total number of viewers across all streams",
        ))?;

        registry.register(Box::new(thumbnail_generation_time.clone()))?;
        registry.register(Box::new(playback_rate.clone()))?;
        registry.register(Box::new(block_on_thumbnail.clone()))?;
        registry.register(Box::new(block_on_egress_results.clone()))?;
        registry.register(Box::new(block_on_start_stream.clone()))?;
        registry.register(Box::new(block_on_rtmp_connect.clone()))?;
        registry.register(Box::new(block_on_moq_origin.clone()))?;
        registry.register(Box::new(total_viewers.clone()))?;

        Ok(Self {
            thumbnail_generation_time,
            playback_rate,
            block_on_thumbnail,
            block_on_egress_results,
            block_on_start_stream,
            block_on_rtmp_connect,
            block_on_moq_origin,
            total_viewers,
        })
    }

    /// Initialize global metrics with the default registry
    pub fn init_global() -> prometheus::Result<()> {
        let metrics = Self::new(prometheus::default_registry())?;
        METRICS.set(metrics).map_err(|_| {
            prometheus::Error::Msg("PipelineMetrics already initialized".to_string())
        })?;
        Ok(())
    }

    /// Get the global metrics instance
    pub fn global() -> Option<&'static PipelineMetrics> {
        METRICS.get()
    }

    /// Export all metrics in prometheus text format
    pub fn export_text() -> Result<String, prometheus::Error> {
        let encoder = TextEncoder::new();
        let metric_families = prometheus::gather();
        let mut buffer = Vec::new();
        encoder.encode(&metric_families, &mut buffer)?;
        String::from_utf8(buffer).map_err(|e| prometheus::Error::Msg(e.to_string()))
    }

    /// Get the prometheus content type for HTTP responses
    pub fn content_type() -> &'static str {
        prometheus::TEXT_FORMAT
    }
}

/// Record a thumbnail generation duration
pub fn record_thumbnail_generation_time(duration: Duration) {
    if let Some(metrics) = PipelineMetrics::global() {
        metrics
            .thumbnail_generation_time
            .observe(duration.as_secs_f64());
    }
}

/// Record playback rate as fraction of target FPS
pub fn record_playback_rate(pipeline_id: &str, average_fps: f32, target_fps: f32) {
    if target_fps > 0.0
        && let Some(metrics) = PipelineMetrics::global()
    {
        let rate = average_fps as f64 / target_fps as f64;
        metrics
            .playback_rate
            .with_label_values(&[pipeline_id])
            .observe(rate);
    }
}

/// Remove playback rate metrics for a pipeline when it ends
pub fn remove_playback_rate(pipeline_id: &str) {
    if let Some(metrics) = PipelineMetrics::global()
        && let Err(e) = metrics.playback_rate.remove_label_values(&[pipeline_id])
    {
        debug!(
            "Failed to remove playback rate metrics for {}: {}",
            pipeline_id, e
        );
    }
}

/// Record block_on duration for on_thumbnail calls
pub fn record_block_on_thumbnail(duration: Duration) {
    if let Some(metrics) = PipelineMetrics::global() {
        metrics.block_on_thumbnail.observe(duration.as_secs_f64());
    }
}

/// Record block_on duration for handle_egress_results calls
pub fn record_block_on_egress_results(duration: Duration) {
    if let Some(metrics) = PipelineMetrics::global() {
        metrics
            .block_on_egress_results
            .observe(duration.as_secs_f64());
    }
}

/// Record block_on duration for start_stream calls
pub fn record_block_on_start_stream(duration: Duration) {
    if let Some(metrics) = PipelineMetrics::global() {
        metrics
            .block_on_start_stream
            .observe(duration.as_secs_f64());
    }
}

/// Record block_on duration for RTMP forwarder connect calls
pub fn record_block_on_rtmp_connect(duration: Duration) {
    if let Some(metrics) = PipelineMetrics::global() {
        metrics
            .block_on_rtmp_connect
            .observe(duration.as_secs_f64());
    }
}

/// Record block_on duration for MoQ origin calls
pub fn record_block_on_moq_origin(duration: Duration) {
    if let Some(metrics) = PipelineMetrics::global() {
        metrics.block_on_moq_origin.observe(duration.as_secs_f64());
    }
}

/// Set the total number of viewers across all streams
pub fn set_total_viewers(count: u64) {
    if let Some(metrics) = PipelineMetrics::global() {
        metrics.total_viewers.set(count as f64);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EndpointStats {
    pub name: String,
    pub bitrate: usize,
}

/// Generic packet metrics collection for ingress and egress components
#[derive(Debug, Clone)]
pub struct PacketMetrics {
    pub bytes_processed: u64,
    pub packets_processed: u64,
    pub last_metrics_update: Instant,
    pub source_name: String,
    pub reporting_interval: Duration,
    sender: Option<UnboundedSender<EndpointStats>>,
}

impl PacketMetrics {
    /// Create new packet metrics instance with default 2-second reporting interval
    pub fn new(source_name: &str, sender: Option<UnboundedSender<EndpointStats>>) -> Self {
        Self::new_with_interval(source_name, sender, Duration::from_secs(2))
    }

    /// Create new packet metrics instance with custom reporting interval
    pub fn new_with_interval(
        source_name: &str,
        sender: Option<UnboundedSender<EndpointStats>>,
        reporting_interval: Duration,
    ) -> Self {
        Self {
            bytes_processed: 0,
            packets_processed: 0,
            last_metrics_update: Instant::now(),
            source_name: source_name.to_string(),
            reporting_interval,
            sender,
        }
    }

    /// Update metrics with processed packet data and auto-report if interval elapsed
    pub fn update(&mut self, bytes: usize) {
        self.bytes_processed += bytes as u64;
        self.packets_processed += 1;

        // Auto-report if interval has elapsed
        if self.should_report() {
            self.report_and_reset();
        }
    }

    /// Update metrics with processed packet data and auto-report with extra info if interval elapsed
    pub fn update_with_extra(&mut self, bytes: usize, extra_info: Option<&str>) {
        self.bytes_processed += bytes as u64;
        self.packets_processed += 1;

        // Auto-report if interval has elapsed
        if self.should_report() {
            self.report_and_reset_with_extra(extra_info);
        }
    }

    /// Calculate current bitrate in bits per second
    pub fn calculate_bitrate(&self) -> f64 {
        let elapsed = self.last_metrics_update.elapsed().as_secs_f64();
        if elapsed > 0.0 {
            (self.bytes_processed as f64 * 8.0) / elapsed
        } else {
            0.0
        }
    }

    /// Calculate packet rate in packets per second
    pub fn calculate_packet_rate(&self) -> f64 {
        let elapsed = self.last_metrics_update.elapsed().as_secs_f64();
        if elapsed > 0.0 {
            self.packets_processed as f64 / elapsed
        } else {
            0.0
        }
    }

    /// Check if metrics should be reported based on configured interval
    pub fn should_report(&self) -> bool {
        self.last_metrics_update.elapsed() >= self.reporting_interval
    }

    /// Report metrics and reset counters
    pub fn report_and_reset(&mut self) {
        self.report_and_reset_with_extra(None);
    }

    /// Report metrics with optional extra information and reset counters
    pub fn report_and_reset_with_extra(&mut self, extra_info: Option<&str>) {
        if !self.should_report() {
            return;
        }

        let bitrate_mbps = self.calculate_bitrate() / 1_000_000.0;
        let packet_rate = self.calculate_packet_rate();

        // Debug print with optional extra information
        match extra_info {
            Some(extra) => {
                debug!(
                    "{}: {:.1} Mbps, {:.1} pps, {} packets, {} bytes, {}",
                    self.source_name,
                    bitrate_mbps,
                    packet_rate,
                    self.packets_processed,
                    self.bytes_processed,
                    extra
                );
            }
            None => {
                debug!(
                    "{}: {:.1} Mbps, {:.1} pps, {} packets, {} bytes",
                    self.source_name,
                    bitrate_mbps,
                    packet_rate,
                    self.packets_processed,
                    self.bytes_processed
                );
            }
        }

        // Send metrics to pipeline if sender is available
        if let Some(sender) = &self.sender {
            let bitrate_bps = self.calculate_bitrate() as usize;
            if bitrate_bps > 0 {
                let stats = EndpointStats {
                    name: self.source_name.to_string(),
                    bitrate: bitrate_bps,
                };

                if let Err(e) = sender.send(stats) {
                    warn!("Failed to send {} metrics: {}", self.source_name, e);
                }
            }
        }

        // Reset counters for next interval
        self.bytes_processed = 0;
        self.packets_processed = 0;
        self.last_metrics_update = Instant::now();
    }

    /// Get current metrics without resetting
    pub fn get_current_metrics(&self) -> PacketMetricsSnapshot {
        PacketMetricsSnapshot {
            bytes_processed: self.bytes_processed,
            packets_processed: self.packets_processed,
            bitrate_bps: self.calculate_bitrate(),
            packet_rate_pps: self.calculate_packet_rate(),
            elapsed_seconds: self.last_metrics_update.elapsed().as_secs_f64(),
        }
    }

    /// Reset all metrics
    pub fn reset(&mut self) {
        self.bytes_processed = 0;
        self.packets_processed = 0;
        self.last_metrics_update = Instant::now();
    }
}

/// Snapshot of packet metrics at a point in time
#[derive(Debug, Clone)]
pub struct PacketMetricsSnapshot {
    pub bytes_processed: u64,
    pub packets_processed: u64,
    pub bitrate_bps: f64,
    pub packet_rate_pps: f64,
    pub elapsed_seconds: f64,
}
