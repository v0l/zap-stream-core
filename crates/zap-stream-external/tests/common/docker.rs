use tokio::process::Command;

/// Check if Docker is available and running.
pub async fn check_docker_available() -> bool {
    Command::new("docker")
        .args(["ps"])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Detect a running container whose name contains `pattern`.
pub async fn detect_container(pattern: &str) -> Option<String> {
    let output = Command::new("docker")
        .args(["ps", "--format", "{{.Names}}"])
        .output()
        .await
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find(|line| line.contains(pattern))
        .map(String::from)
}

/// Fetch the last `tail` lines of logs from `container`.
pub async fn get_docker_logs(container: &str, tail: u32) -> String {
    let output = Command::new("docker")
        .args(["logs", "--tail", &tail.to_string(), container])
        .output()
        .await
        .expect("docker logs failed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("{}{}", stdout, stderr)
}

/// Check if a command is available on PATH.
pub async fn command_exists(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}
