#![allow(clippy::unwrap_used)]
use std::time::Instant;

fn ws() -> String {
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", &format!("$p=Get-Process -Id {}; '{{0:N1}} MB WS, {{1:N1}} MB private, {{2}} threads' -f ($p.WorkingSet64/1MB), ($p.PrivateMemorySize64/1MB), $p.Threads.Count", std::process::id())])
        .output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

#[test]
fn gpu_cost() {
    eprintln!("before: {}", ws());
    let t = Instant::now();
    let gpu = vixeeny_capture::freeze::Gpu::new().unwrap();
    let first = t.elapsed();
    eprintln!("with device: {} (created in {first:?})", ws());
    drop(gpu);
    eprintln!("dropped: {}", ws());
    let t = Instant::now();
    let gpu = vixeeny_capture::freeze::Gpu::new().unwrap();
    eprintln!("second creation {:?}", t.elapsed());
    drop(gpu);
}
