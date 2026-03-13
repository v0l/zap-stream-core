use tokio::process::{Child, Command};

pub struct FfmpegStream {
    child: Child,
}

impl FfmpegStream {
    /// Start an RTMPS stream using ffmpeg with a test source.
    pub async fn start_rtmps(
        rtmp_url: &str,
        stream_key: &str,
        duration_secs: u32,
        sine_freq: u32,
    ) -> Self {
        let dest = format!("{}{}", rtmp_url, stream_key);
        let child = Command::new("ffmpeg")
            .args([
                "-re",
                "-t",
                &duration_secs.to_string(),
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=1280x720:rate=30",
                "-f",
                "lavfi",
                "-i",
                &format!("sine=frequency={}:sample_rate=44100", sine_freq),
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-tune",
                "zerolatency",
                "-b:v",
                "2000k",
                "-c:a",
                "aac",
                "-ar",
                "44100",
                "-b:a",
                "128k",
                "-f",
                "flv",
                &dest,
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("Failed to start ffmpeg");

        Self { child }
    }

    /// Check if the ffmpeg process is still running.
    pub fn is_running(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_none()
    }

    /// Kill the ffmpeg process and wait for it to exit.
    pub async fn stop(&mut self) {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }
}
