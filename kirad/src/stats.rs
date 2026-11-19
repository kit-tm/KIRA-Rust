//! Output of the R²/KAD message statistics ([MessageStats]) as JSON.

use std::{
    io::Write,
    path::Path,
    sync::Arc,
    time::Duration,
};

use kira_lib::io::stats::{
    KindSnapshot,
    MessageStats,
    StatsSnapshot,
};
use serde_json::{
    Map,
    Value,
    json,
};

fn rate(count: u64, secs: f64) -> f64 {
    if secs > 0.0 { count as f64 / secs } else { 0.0 }
}

fn counters_to_json(c: &KindSnapshot, secs: f64) -> Value {
    json!({
        "tx_msgs": c.tx_msgs,
        "tx_payload_bytes": c.tx_bytes,
        "tx_wire_bytes": c.tx_wire_bytes(),
        "tx_msgs_per_s": rate(c.tx_msgs, secs),
        "tx_payload_bytes_per_s": rate(c.tx_bytes, secs),
        "tx_wire_bytes_per_s": rate(c.tx_wire_bytes(), secs),
        "rx_msgs": c.rx_msgs,
        "rx_payload_bytes": c.rx_bytes,
        "rx_wire_bytes": c.rx_wire_bytes(),
        "rx_msgs_per_s": rate(c.rx_msgs, secs),
        "rx_payload_bytes_per_s": rate(c.rx_bytes, secs),
        "rx_wire_bytes_per_s": rate(c.rx_wire_bytes(), secs),
    })
}

/// Converts a [StatsSnapshot] to JSON with totals, per-kind counters and averages per second
/// since the start of the node.
pub fn stats_to_json(snapshot: &StatsSnapshot, node_id: &str, format: &str) -> Value {
    let secs = snapshot.elapsed.as_secs_f64();
    let per_kind: Map<String, Value> = snapshot
        .per_kind
        .iter()
        .map(|(kind, c)| (format!("{kind:?}"), counters_to_json(c, secs)))
        .collect();
    json!({
        "node_id": node_id,
        "format": format,
        "unix_time_s": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or_default(),
        "duration_s": secs,
        "ipv6_udp_header_bytes": kira_lib::io::stats::IPV6_UDP_HEADER_BYTES,
        "total": counters_to_json(&snapshot.total(), secs),
        "per_kind": per_kind,
        "rx_invalid_msgs": snapshot.rx_invalid_msgs,
        "rx_invalid_bytes": snapshot.rx_invalid_bytes,
        "tx_errors": snapshot.tx_errors,
    })
}

/// Writes the current statistics to `path`, replacing it atomically.
pub fn write_summary(
    path: &Path,
    stats: &MessageStats,
    node_id: &str,
    format: &str,
) -> std::io::Result<()> {
    let json = stats_to_json(&stats.snapshot(), node_id, format);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&json)?)?;
    std::fs::rename(&tmp, path)
}

/// Appends a snapshot of the cumulative statistics as one JSON line to `path` every `interval`.
pub async fn write_periodically(
    path: std::path::PathBuf,
    interval: Duration,
    stats: Arc<MessageStats>,
    node_id: String,
    format: String,
) {
    let mut file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(err) => {
            tracing::error!(path = %path.display(), %err, "Failed to open statistics file");
            return;
        }
    };
    let mut ticker = tokio::time::interval(interval);
    ticker.tick().await; // the first tick completes immediately
    loop {
        ticker.tick().await;
        let json = stats_to_json(&stats.snapshot(), &node_id, &format);
        if let Err(err) = writeln!(file, "{json}").and_then(|_| file.flush()) {
            tracing::error!(path = %path.display(), %err, "Failed to write statistics");
            return;
        }
    }
}
