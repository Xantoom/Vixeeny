// SPDX-License-Identifier: GPL-3.0-or-later
//! `cargo xtask bench-idle [--seconds N] [--daemon PATH]`: measures NF-1 (idle CPU, wake-ups)
//! and NF-2 (idle memory) of the daemon. Defaults to the 10 minutes of the plan.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

const SETTLE: Duration = Duration::from_secs(5);
const LIMIT_PRIVATE_BYTES: f64 = 15.0 * 1024.0 * 1024.0;
const LIMIT_WAKEUPS_PER_MIN: f64 = 10.0;
const LIMIT_CPU_PERCENT: f64 = 0.05;

#[derive(Debug, Clone, Copy)]
struct Sample {
    cpu_seconds: f64,
    private_bytes: f64,
    threads: u64,
    context_switches: u64,
}

pub fn run(args: &[String]) -> Result<()> {
    let mut seconds = 600u64;
    let mut daemon = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--seconds" => {
                seconds = it
                    .next()
                    .context("--seconds needs a value")?
                    .parse()
                    .context("--seconds must be a number")?;
            }
            "--daemon" => daemon = Some(PathBuf::from(it.next().context("--daemon needs a path")?)),
            other => bail!("unknown option `{other}`"),
        }
    }
    let daemon = match daemon {
        Some(path) => path,
        None => default_daemon()?,
    };

    // Isolated instance: own endpoint, config and logs, no autostart entry.
    let scratch = std::env::temp_dir().join(format!("vixeeny-bench-{}", std::process::id()));
    std::fs::create_dir_all(&scratch)?;
    let mut child = Command::new(&daemon)
        .env("VIXEENY_NO_AUTOSTART", "1")
        .env(
            "VIXEENY_IPC_NAME",
            format!("vixeeny-bench-{}", std::process::id()),
        )
        .env("VIXEENY_CONFIG_DIR", scratch.join("config"))
        .env("VIXEENY_LOG_DIR", scratch.join("logs"))
        .stdin(Stdio::null())
        .spawn()
        .with_context(|| format!("starting {}", daemon.display()))?;
    let pid = child.id();
    println!("daemon pid {pid}; settling {SETTLE:?}, then measuring {seconds} s…");

    let result = measure(pid, seconds, &mut child);
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

fn measure(pid: u32, seconds: u64, child: &mut std::process::Child) -> Result<()> {
    std::thread::sleep(SETTLE);
    if let Some(status) = child.try_wait()? {
        bail!("the daemon exited early ({status})");
    }
    let start = sample(pid)?;
    let t0 = Instant::now();
    std::thread::sleep(Duration::from_secs(seconds));
    if let Some(status) = child.try_wait()? {
        bail!("the daemon exited during the measurement ({status})");
    }
    let end = sample(pid)?;
    let elapsed = t0.elapsed().as_secs_f64();

    let cpu_percent = (end.cpu_seconds - start.cpu_seconds) / elapsed * 100.0;
    let wakeups =
        end.context_switches.saturating_sub(start.context_switches) as f64 / elapsed * 60.0;
    let mib = |b: f64| b / 1024.0 / 1024.0;
    let verdict = |ok: bool| if ok { "OK  " } else { "FAIL" };

    println!("\nover {elapsed:.0} s:");
    println!(
        "  NF-1 CPU        {cpu_percent:.3} % (limit {LIMIT_CPU_PERCENT} %)        {}",
        verdict(cpu_percent <= LIMIT_CPU_PERCENT)
    );
    println!(
        "  NF-1 wake-ups   {wakeups:.1} /min (limit {LIMIT_WAKEUPS_PER_MIN})        {}",
        verdict(wakeups < LIMIT_WAKEUPS_PER_MIN)
    );
    println!(
        "  NF-2 private    {:.1} MiB (limit {:.0})        {}",
        mib(end.private_bytes),
        mib(LIMIT_PRIVATE_BYTES),
        verdict(end.private_bytes < LIMIT_PRIVATE_BYTES)
    );
    println!(
        "  threads         {} (start {})",
        end.threads, start.threads
    );
    println!(
        "  context switches {} → {}",
        start.context_switches, end.context_switches
    );
    let pass = cpu_percent <= LIMIT_CPU_PERCENT
        && wakeups < LIMIT_WAKEUPS_PER_MIN
        && end.private_bytes < LIMIT_PRIVATE_BYTES;
    if !pass {
        bail!("idle targets not met");
    }
    Ok(())
}

fn default_daemon() -> Result<PathBuf> {
    let mut path = std::env::current_exe()?;
    path.set_file_name(format!("vixeeny-daemon{}", std::env::consts::EXE_SUFFIX));
    if !path.exists() {
        bail!(
            "{} not found; build it (`cargo build --release -p vixeeny-daemon`) or pass --daemon",
            path.display()
        );
    }
    Ok(path)
}

#[cfg(windows)]
fn sample(pid: u32) -> Result<Sample> {
    // WMI raw counters are locale-independent, unlike `Get-Counter` paths. The thread counters
    // are cumulative, so summing them gives the process's total context switches.
    let script = format!(
        "$p = Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -Filter 'IDProcess={pid}'; \
         $c = (Get-Process -Id {pid}).TotalProcessorTime.TotalSeconds; \
         $s = (Get-CimInstance Win32_PerfRawData_PerfProc_Thread -Filter 'IDProcess={pid}' | \
               Measure-Object ContextSwitchesPersec -Sum).Sum; \
         \"$c $($p.WorkingSetPrivate) $($p.ThreadCount) $s\""
    );
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .context("running powershell")?;
    if !out.status.success() {
        bail!(
            "powershell failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let f: Vec<&str> = text.split_whitespace().collect();
    let [cpu, private, threads, switches] = f[..] else {
        bail!("unexpected powershell output: {text:?}");
    };
    Ok(Sample {
        cpu_seconds: cpu.replace(',', ".").parse()?,
        private_bytes: private.parse()?,
        threads: threads.parse()?,
        context_switches: switches.parse()?,
    })
}

#[cfg(not(windows))]
fn sample(pid: u32) -> Result<Sample> {
    // Linux (development aid): /proc.
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    // Fields after the parenthesised command name; utime and stime are fields 14 and 15.
    let after = stat.rsplit_once(')').context("bad /proc stat")?.1;
    let fields: Vec<&str> = after.split_whitespace().collect();
    let ticks =
        |i: usize| -> Result<f64> { Ok(fields.get(i).context("short /proc stat")?.parse()?) };
    let cpu_seconds = (ticks(11)? + ticks(12)?) / 100.0;
    let status = std::fs::read_to_string(format!("/proc/{pid}/status"))?;
    let kb = |key: &str| -> u64 {
        status
            .lines()
            .find_map(|l| l.strip_prefix(key))
            .and_then(|v| v.split_whitespace().next()?.parse().ok())
            .unwrap_or(0)
    };
    Ok(Sample {
        cpu_seconds,
        private_bytes: (kb("RssAnon:") * 1024) as f64,
        threads: kb("Threads:"),
        context_switches: kb("voluntary_ctxt_switches:") + kb("nonvoluntary_ctxt_switches:"),
    })
}
