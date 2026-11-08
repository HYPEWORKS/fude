//! Development tasks. Run with `cargo xtask <task>`.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const USAGE: &str = "\
Usage: cargo xtask <task>

Tasks:
  bench-startup [OPTIONS] [-- FUDE_ARGS...]
      Launch the release build repeatedly and measure time to first presented frame.

      --runs <N>        Measured runs [default: 20]
      --warmup <N>      Unmeasured runs first, to warm caches [default: 3]
      --budget-ms <MS>  Median budget for launch to first frame [default: 40]
      --check           Exit with an error if the median is over budget
      --no-build        Use the existing target/release/fude";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("bench-startup") => bench_startup(&args[1..]),
        Some("-h" | "--help") | None => {
            println!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(format!("unknown task {other:?}\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask is in the workspace")
        .to_path_buf()
}

struct Run {
    /// Spawn to the `first-present` line, measured by this process.
    external_ms: f64,
    /// Internal milestones, ms since fude's `main`.
    marks: Vec<(String, f64)>,
    rss: Option<u64>,
    peak_rss: Option<u64>,
}

fn bench_startup(args: &[String]) -> Result<(), String> {
    let mut runs = 20usize;
    let mut warmup = 3usize;
    let mut budget = 40.0f64;
    let mut check = false;
    let mut build = true;
    let mut fude_args: Vec<String> = vec![];
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut num = |name: &str| -> Result<f64, String> {
            it.next()
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| format!("{name} needs a number"))
        };
        match a.as_str() {
            "--runs" => runs = num("--runs")?.max(1.0) as usize,
            "--warmup" => warmup = num("--warmup")? as usize,
            "--budget-ms" => budget = num("--budget-ms")?,
            "--check" => check = true,
            "--no-build" => build = false,
            "--" => fude_args.extend(it.by_ref().cloned()),
            other => return Err(format!("unknown option {other:?}\n\n{USAGE}")),
        }
    }

    let root = workspace_root();
    if build {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let status = Command::new(cargo)
            .args(["build", "--release", "--quiet", "-p", "fude"])
            .current_dir(&root)
            .status()
            .map_err(|e| format!("running cargo: {e}"))?;
        if !status.success() {
            return Err("release build failed".into());
        }
    }
    let exe = root
        .join("target/release")
        .join(format!("fude{}", std::env::consts::EXE_SUFFIX));
    if !exe.exists() {
        return Err(format!(
            "{} not found; build it or drop --no-build",
            exe.display()
        ));
    }

    for _ in 0..warmup {
        run_once(&exe, &fude_args)?;
    }
    let mut results = Vec::with_capacity(runs);
    for _ in 0..runs {
        results.push(run_once(&exe, &fude_args)?);
    }
    report(&results, warmup, budget);

    let median = percentile(results.iter().map(|r| r.external_ms).collect(), 0.5);
    if check && median > budget {
        return Err(format!(
            "median {median:.1} ms is over the {budget} ms budget"
        ));
    }
    Ok(())
}

fn run_once(exe: &PathBuf, extra: &[String]) -> Result<Run, String> {
    let start = Instant::now();
    let mut child = Command::new(exe)
        .arg("--bench-startup")
        .args(extra)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("launching {}: {e}", exe.display()))?;

    // Read output on a thread so a hung launch (e.g. no display) can time out.
    let stdout = child.stdout.take().expect("stdout is piped");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let at = start.elapsed();
            if tx.send((at, line)).is_err() {
                break;
            }
        }
    });

    let mut run = Run {
        external_ms: f64::NAN,
        marks: vec![],
        rss: None,
        peak_rss: None,
    };
    let deadline = start + Duration::from_secs(15);
    loop {
        let timeout = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(timeout) {
            Ok((at, line)) => {
                let parts: Vec<&str> = line.split_whitespace().collect();
                match parts.as_slice() {
                    ["fude-bench", "mark", name, ms] => {
                        if *name == "first-present" {
                            run.external_ms = at.as_secs_f64() * 1000.0;
                        }
                        run.marks
                            .push((name.to_string(), ms.parse().unwrap_or(f64::NAN)));
                    }
                    ["fude-bench", "rss-bytes", b] => run.rss = b.parse().ok(),
                    ["fude-bench", "peak-rss-bytes", b] => run.peak_rss = b.parse().ok(),
                    _ => {}
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                return Err(
                    "fude did not present a frame within 15 s (is a display available?)".into(),
                );
            }
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() || run.external_ms.is_nan() {
        return Err(format!(
            "fude exited with {status} before presenting a frame"
        ));
    }
    Ok(run)
}

fn percentile(mut v: Vec<f64>, p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    let i = ((v.len() - 1) as f64 * p).round() as usize;
    v[i]
}

fn report(runs: &[Run], warmup: usize, budget: f64) {
    let ext: Vec<f64> = runs.iter().map(|r| r.external_ms).collect();
    let median = percentile(ext.clone(), 0.5);
    let verdict = if median <= budget { "within" } else { "OVER" };
    println!(
        "fude startup: {} runs ({warmup} warm-up), release build",
        runs.len()
    );
    println!();
    println!("  launch → first frame presented (ms)");
    println!(
        "    min {:.1}   median {:.1}   p90 {:.1}   max {:.1}    {verdict} the {budget} ms budget",
        percentile(ext.clone(), 0.0),
        median,
        percentile(ext.clone(), 0.9),
        percentile(ext, 1.0)
    );
    println!();
    println!("  milestones inside fude (median ms since main)");
    let names: Vec<String> = runs[0].marks.iter().map(|(n, _)| n.clone()).collect();
    for name in names {
        let v: Vec<f64> = runs
            .iter()
            .filter_map(|r| r.marks.iter().find(|(n, _)| *n == name).map(|(_, ms)| *ms))
            .collect();
        println!("    {name:<14} {:>7.2}", percentile(v, 0.5));
    }
    let mb = |b: u64| b as f64 / (1024.0 * 1024.0);
    let rss: Vec<f64> = runs.iter().filter_map(|r| r.rss.map(mb)).collect();
    let peak: Vec<f64> = runs.iter().filter_map(|r| r.peak_rss.map(mb)).collect();
    if !rss.is_empty() {
        println!();
        println!(
            "  memory at first frame (median): RSS {:.1} MB, peak {:.1} MB",
            percentile(rss, 0.5),
            percentile(peak, 0.5)
        );
    }
}
