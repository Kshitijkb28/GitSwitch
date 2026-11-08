//! A speed test: how fast this connection downloads and uploads right now,
//! measured against Cloudflare's speed test (speed.cloudflare.com, the same
//! endpoints its own page uses). On demand only; it uses about 100 MB.

use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct TestState {
    /// idle | latency | download | upload | done | cancelled | failed
    pub phase: String,
    /// 0..1, the whole test.
    pub progress: f64,
    pub latency_ms: Option<f64>,
    /// Bytes per second.
    pub down_bps: Option<f64>,
    pub up_bps: Option<f64>,
    /// What the test downloaded and uploaded, in bytes.
    pub used_bytes: u64,
    pub error: Option<String>,
    /// When it finished (seconds since 1970).
    pub at: Option<i64>,
}

/// How much a test may take.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub pings: usize,
    pub down_workers: usize,
    pub down_chunk: u64,
    pub down_time: Duration,
    pub down_bytes: u64,
    pub up_workers: usize,
    pub up_chunk: usize,
    pub up_time: Duration,
    pub up_bytes: u64,
}

impl Limits {
    pub const FULL: Limits = Limits {
        pings: 8,
        down_workers: 4,
        down_chunk: 25_000_000,
        down_time: Duration::from_secs(10),
        down_bytes: 75_000_000,
        up_workers: 3,
        up_chunk: 5_000_000,
        up_time: Duration::from_secs(8),
        up_bytes: 25_000_000,
    };
    /// A few megabytes: is the service there at all.
    #[allow(dead_code)]
    pub const QUICK: Limits = Limits {
        pings: 3,
        down_workers: 2,
        down_chunk: 2_000_000,
        down_time: Duration::from_secs(5),
        down_bytes: 4_000_000,
        up_workers: 1,
        up_chunk: 500_000,
        up_time: Duration::from_secs(5),
        up_bytes: 1_000_000,
    };
}

fn state() -> &'static Mutex<(TestState, Option<Arc<AtomicBool>>)> {
    static S: OnceLock<Mutex<(TestState, Option<Arc<AtomicBool>>)>> = OnceLock::new();
    S.get_or_init(|| Mutex::new((TestState { phase: "idle".into(), ..Default::default() }, None)))
}

fn lock() -> std::sync::MutexGuard<'static, (TestState, Option<Arc<AtomicBool>>)> {
    state().lock().unwrap_or_else(|e| e.into_inner())
}

/// Cloudflare's speed test — debug builds can point the tests at a local server.
pub fn base_url() -> String {
    #[cfg(debug_assertions)]
    if let Some(u) = std::env::var("GITSWITCH_SPEEDTEST_URL").ok().filter(|u| !u.is_empty()) {
        return u.trim_end_matches('/').to_string();
    }
    "https://speed.cloudflare.com".into()
}

fn running(phase: &str) -> bool {
    matches!(phase, "latency" | "download" | "upload")
}

pub fn start() -> Result<(), String> {
    let cancel = {
        let mut st = lock();
        if running(&st.0.phase) {
            return Err("A speed test is already running.".into());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        st.0 = TestState { phase: "latency".into(), ..Default::default() };
        st.1 = Some(cancel.clone());
        cancel
    };
    tauri::async_runtime::spawn(async move {
        let result = measure(&base_url(), &cancel, Limits::FULL, |s| {
            let mut st = lock();
            if running(&st.0.phase) {
                st.0 = s;
            }
        })
        .await;
        let mut st = lock();
        st.1 = None;
        st.0 = match result {
            Ok(s) => s,
            Err(e) => TestState { phase: if cancel.load(Ordering::SeqCst) { "cancelled".into() } else { "failed".into() }, error: Some(e), at: Some(chrono::Utc::now().timestamp()), ..st.0.clone() },
        };
    });
    Ok(())
}

pub fn status() -> TestState {
    lock().0.clone()
}

pub fn cancel() {
    if let Some(c) = lock().1.as_ref() {
        c.store(true, Ordering::SeqCst);
    }
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some(v[v.len() / 2])
}

/// Latency, then download, then upload; `report` hears every step.
pub async fn measure(base: &str, cancel: &Arc<AtomicBool>, limits: Limits, report: impl Fn(TestState)) -> Result<TestState, String> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(30)).user_agent("GitSwitch speed test").build().map_err(|e| e.to_string())?;
    let stopped = || cancel.load(Ordering::SeqCst);
    let mut s = TestState { phase: "latency".into(), ..Default::default() };
    report(s.clone());

    // Latency: the time to an answer for nothing.
    let mut pings = Vec::new();
    let mut last_err = None;
    for i in 0..limits.pings {
        if stopped() {
            return Err("Cancelled.".into());
        }
        let t = Instant::now();
        match client.get(format!("{}/__down?bytes=0", base)).send().await {
            Ok(r) if r.status().is_success() => {
                pings.push(t.elapsed().as_secs_f64() * 1000.0);
                let _ = r.bytes().await;
            }
            Ok(r) => last_err = Some(format!("the speed test answered {}", r.status())),
            Err(e) => last_err = Some(e.to_string()),
        }
        s.progress = 0.1 * (i + 1) as f64 / limits.pings as f64;
        report(s.clone());
    }
    s.latency_ms = Some(median(pings).ok_or_else(|| format!("Cloudflare's speed test could not be reached: {}", last_err.unwrap_or_default()))?);

    // Download: several streams at once, for a while or up to a budget.
    s.phase = "download".into();
    report(s.clone());
    let got = Arc::new(AtomicU64::new(0));
    let first = Arc::new(Mutex::new(None::<Instant>));
    let started = Instant::now();
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..limits.down_workers {
        let (client, got, first, url, cancel) = (client.clone(), got.clone(), first.clone(), format!("{}/__down?bytes={}", base, limits.down_chunk), cancel.clone());
        tasks.spawn(async move {
            let stopped = || cancel.load(Ordering::SeqCst);
            while started.elapsed() < limits.down_time && got.load(Ordering::SeqCst) < limits.down_bytes && !stopped() {
                let Ok(mut r) = client.get(&url).send().await else { break };
                if !r.status().is_success() {
                    break;
                }
                while let Ok(Some(chunk)) = r.chunk().await {
                    first.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Instant::now);
                    got.fetch_add(chunk.len() as u64, Ordering::SeqCst);
                    if started.elapsed() >= limits.down_time || got.load(Ordering::SeqCst) >= limits.down_bytes || stopped() {
                        break;
                    }
                }
            }
        });
    }
    let watch = async {
        while started.elapsed() < limits.down_time + Duration::from_secs(2) && got.load(Ordering::SeqCst) < limits.down_bytes && !stopped() {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let mut t = s.clone();
            t.progress = 0.1 + 0.6 * (started.elapsed().as_secs_f64() / limits.down_time.as_secs_f64()).max(got.load(Ordering::SeqCst) as f64 / limits.down_bytes as f64).min(1.0);
            report(t);
        }
    };
    tokio::join!(async { while tasks.join_next().await.is_some() {} }, watch);
    if stopped() {
        return Err("Cancelled.".into());
    }
    let down = got.load(Ordering::SeqCst);
    let since = first.lock().unwrap_or_else(|e| e.into_inner()).unwrap_or(started);
    if down == 0 {
        return Err("Nothing could be downloaded from the speed test.".into());
    }
    s.down_bps = Some(down as f64 / since.elapsed().as_secs_f64().max(0.001));
    s.used_bytes += down;
    s.progress = 0.7;

    // Upload: what is sent, counted as each request is answered.
    s.phase = "upload".into();
    report(s.clone());
    let sent = Arc::new(AtomicU64::new(0));
    let body = Arc::new(bytes_of(limits.up_chunk));
    let started = Instant::now();
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..limits.up_workers {
        let (client, sent, body, url, cancel) = (client.clone(), sent.clone(), body.clone(), format!("{}/__up", base), cancel.clone());
        tasks.spawn(async move {
            let stopped = || cancel.load(Ordering::SeqCst);
            while started.elapsed() < limits.up_time && sent.load(Ordering::SeqCst) < limits.up_bytes && !stopped() {
                match client.post(&url).body(body.as_ref().clone()).send().await {
                    Ok(r) if r.status().is_success() => {
                        sent.fetch_add(body.len() as u64, Ordering::SeqCst);
                        let _ = r.bytes().await;
                    }
                    _ => break,
                }
            }
        });
    }
    let watch = async {
        while started.elapsed() < limits.up_time + Duration::from_secs(5) && sent.load(Ordering::SeqCst) < limits.up_bytes && !stopped() {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let mut t = s.clone();
            t.progress = 0.7 + 0.3 * (started.elapsed().as_secs_f64() / limits.up_time.as_secs_f64()).max(sent.load(Ordering::SeqCst) as f64 / limits.up_bytes as f64).min(1.0);
            report(t);
        }
    };
    tokio::join!(async { while tasks.join_next().await.is_some() {} }, watch);
    if stopped() {
        return Err("Cancelled.".into());
    }
    let up = sent.load(Ordering::SeqCst);
    s.up_bps = (up > 0).then(|| up as f64 / started.elapsed().as_secs_f64().max(0.001));
    s.used_bytes += up;
    s.progress = 1.0;
    s.phase = "done".into();
    s.at = Some(chrono::Utc::now().timestamp());
    if s.up_bps.is_none() {
        s.error = Some("The upload could not be measured.".into());
    }
    report(s.clone());
    Ok(s)
}

/// What is uploaded: bytes that don't compress away.
fn bytes_of(n: usize) -> Vec<u8> {
    let mut x: u32 = 0x9e37_79b9;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A tiny HTTP/1.1 server with the speed test's two endpoints.
    async fn server() -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = l.accept().await else { return };
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    loop {
                        // Read one request's head.
                        let head_end = loop {
                            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                                break i + 4;
                            }
                            let mut tmp = [0u8; 4096];
                            match sock.read(&mut tmp).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => buf.extend_from_slice(&tmp[..n]),
                            }
                        };
                        let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                        buf.drain(..head_end);
                        let first = head.lines().next().unwrap_or_default().to_string();
                        let len: usize = head.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0))).unwrap_or(0);
                        while buf.len() < len {
                            let mut tmp = [0u8; 65536];
                            match sock.read(&mut tmp).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => buf.extend_from_slice(&tmp[..n]),
                            }
                        }
                        buf.drain(..len);
                        let n: usize = first.split("bytes=").nth(1).and_then(|r| r.split(' ').next()).and_then(|v| v.parse().ok()).unwrap_or(0);
                        let body_len = if first.starts_with("GET") { n } else { 0 };
                        let _ = sock.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body_len).as_bytes()).await;
                        let chunk = vec![0u8; 65536];
                        let mut left = body_len;
                        while left > 0 {
                            let k = left.min(chunk.len());
                            if sock.write_all(&chunk[..k]).await.is_err() {
                                return;
                            }
                            left -= k;
                        }
                    }
                });
            }
        });
        format!("http://{}", addr)
    }

    #[tokio::test]
    async fn a_speed_test_measures_latency_download_and_upload() {
        let base = server().await;
        let seen = Mutex::new(Vec::<String>::new());
        let s = measure(&base, &Arc::new(AtomicBool::new(false)), Limits::QUICK, |t| seen.lock().unwrap().push(t.phase)).await.unwrap();
        assert_eq!(s.phase, "done");
        assert!(s.latency_ms.unwrap() >= 0.0);
        assert!(s.down_bps.unwrap() > 0.0 && s.up_bps.unwrap() > 0.0, "{s:?}");
        assert!(s.used_bytes >= Limits::QUICK.down_bytes / 2, "{}", s.used_bytes);
        assert!((s.progress - 1.0).abs() < 1e-9);
        let phases = seen.lock().unwrap().clone();
        for p in ["latency", "download", "upload", "done"] {
            assert!(phases.iter().any(|x| x == p), "{p} reported: {phases:?}");
        }
    }

    #[tokio::test]
    async fn a_cancelled_or_unreachable_test_says_so() {
        let base = server().await;
        let cancelled = measure(&base, &Arc::new(AtomicBool::new(true)), Limits::QUICK, |_| {}).await;
        assert_eq!(cancelled.unwrap_err(), "Cancelled.");
        let nowhere = measure("http://127.0.0.1:9", &Arc::new(AtomicBool::new(false)), Limits::QUICK, |_| {}).await;
        assert!(nowhere.unwrap_err().contains("could not be reached"));
        assert_eq!(median(vec![3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(bytes_of(4).len(), 4);
    }
}
