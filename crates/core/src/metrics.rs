use crate::ingress::EndpointStats;
use crate::pipeline::runner::PipelineCommand;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, warn};

/// Generic packet metrics collection for ingress and egress components
#[derive(Debug, Clone)]
pub struct PacketMetrics {
    pub bytes_processed: u64,
    pub packets_processed: u64,
    pub last_metrics_update: Instant,
    pub source_name: &'static str,
    pub reporting_interval: Duration,
    sender: Option<UnboundedSender<PipelineCommand>>,
}

impl PacketMetrics {
    /// Create new packet metrics instance with default 2-second reporting interval
    pub fn new(
        source_name: &'static str,
        sender: Option<UnboundedSender<PipelineCommand>>,
    ) -> Self {
        Self::new_with_interval(source_name, sender, Duration::from_secs(2))
    }

    /// Create new packet metrics instance with custom reporting interval
    pub fn new_with_interval(
        source_name: &'static str,
        sender: Option<UnboundedSender<PipelineCommand>>,
        reporting_interval: Duration,
    ) -> Self {
        Self {
            bytes_processed: 0,
            packets_processed: 0,
            last_metrics_update: Instant::now(),
            source_name,
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
                let stats = match self.source_name.contains("Egress") {
                    true => PipelineCommand::EgressMetrics(EndpointStats {
                        name: self.source_name.to_string(),
                        bitrate: bitrate_bps,
                    }),
                    false => PipelineCommand::IngressMetrics(EndpointStats {
                        name: self.source_name.to_string(),
                        bitrate: bitrate_bps,
                    }),
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
