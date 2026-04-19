use std::{
    env,
    error::Error,
    process,
    time::{Duration, Instant},
};
use vlfd_rs::{Board, IoConfig, TransferStageProfile, TransportConfig};

const WORDS_PER_CYCLE: usize = 4;

fn main() {
    if let Err(err) = real_main() {
        eprintln!("error: {err}");
        process::exit(1);
    }
}

fn real_main() -> Result<(), Box<dyn Error>> {
    let options = Options::parse(env::args().skip(1))?;
    match options.mode {
        RunMode::Cpu => run_cpu_bench(&options),
        RunMode::Device => run_device_bench(&options),
    }
}

#[derive(Debug, Clone, Copy)]
enum RunMode {
    Cpu,
    Device,
}

#[derive(Debug, Clone, Copy)]
struct Options {
    mode: RunMode,
    iterations: usize,
    words: usize,
    window: usize,
    profile_stages: bool,
    clock_high_delay: u16,
    clock_low_delay: u16,
    transport: TransportConfig,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            mode: RunMode::Cpu,
            iterations: 100_000,
            words: 512,
            window: 16,
            profile_stages: false,
            clock_high_delay: 11,
            clock_low_delay: 11,
            transport: TransportConfig::default(),
        }
    }
}

impl Options {
    fn parse<I>(args: I) -> Result<Self, Box<dyn Error>>
    where
        I: IntoIterator<Item = String>,
    {
        let mut options = Self::default();
        let mut args = args.into_iter();

        let Some(mode) = args.next() else {
            print_usage();
            return Err("missing mode (cpu|device)".into());
        };
        options.mode = match mode.as_str() {
            "cpu" => RunMode::Cpu,
            "device" => RunMode::Device,
            _ => {
                print_usage();
                return Err(format!("unknown mode `{mode}`").into());
            }
        };

        while let Some(flag) = args.next() {
            match flag.as_str() {
                "--iterations" => {
                    options.iterations = next_value(&mut args, "--iterations")?.parse()?
                }
                "--words" => options.words = next_value(&mut args, "--words")?.parse()?,
                "--window" => options.window = next_value(&mut args, "--window")?.parse()?,
                "--profile-stages" => options.profile_stages = true,
                "--clock-high" => {
                    options.clock_high_delay = next_value(&mut args, "--clock-high")?.parse()?
                }
                "--clock-low" => {
                    options.clock_low_delay = next_value(&mut args, "--clock-low")?.parse()?
                }
                "--usb-timeout-ms" => {
                    let value: u64 = next_value(&mut args, "--usb-timeout-ms")?.parse()?;
                    options.transport.usb_timeout = Duration::from_millis(value);
                }
                "--sync-timeout-ms" => {
                    let value: u64 = next_value(&mut args, "--sync-timeout-ms")?.parse()?;
                    options.transport.sync_timeout = Duration::from_millis(value);
                }
                "--reset-on-open" => options.transport.reset_on_open = true,
                "--no-clear-halt" => options.transport.clear_halt_on_open = false,
                "--help" | "-h" => {
                    print_usage();
                    process::exit(0);
                }
                other => return Err(format!("unknown flag `{other}`").into()),
            }
        }

        Ok(options)
    }
}

fn next_value<I>(args: &mut I, flag: &str) -> Result<String, Box<dyn Error>>
where
    I: Iterator<Item = String>,
{
    args.next()
        .ok_or_else(|| format!("missing value for `{flag}`").into())
}

fn print_usage() {
    eprintln!(
        "Usage:\n  cargo run --example bench_transfer -- cpu [--words N] [--iterations N]\n  cargo run --example bench_transfer -- device [--words N] [--iterations N] [--window N] [--profile-stages] [--clock-high N] [--clock-low N] [--usb-timeout-ms N] [--sync-timeout-ms N] [--reset-on-open] [--no-clear-halt]"
    );
}

fn run_cpu_bench(options: &Options) -> Result<(), Box<dyn Error>> {
    let template = vec![0x1234u16; options.words];
    let mut scratch = Vec::with_capacity(options.words);
    let key = [0x55aau16; 16];

    let started = Instant::now();
    for _ in 0..options.iterations {
        scratch.clear();
        scratch.extend_from_slice(&template);
        xor_words(&mut scratch, &key);
    }
    let elapsed = started.elapsed();

    print_summary("cpu", options.words, options.iterations, elapsed, None);
    Ok(())
}

fn run_device_bench(options: &Options) -> Result<(), Box<dyn Error>> {
    let mut board = Board::open_with_transport(options.transport)?;
    let max_cycles_per_transfer = usize::from(board.config().fifo_size_words()) / WORDS_PER_CYCLE;
    let mut io = board.configure_io(&IoConfig {
        clock_high_delay: options.clock_high_delay,
        clock_low_delay: options.clock_low_delay,
        ..IoConfig::default()
    })?;

    if options.window == 0 {
        return Err("window must be at least 1".into());
    }

    let template = vec![0x1234u16; options.words];
    let mut rx = vec![0u16; options.words];
    let mut outputs = vec![vec![0u16; options.words]; options.window];
    let mut stage_profile = TransferStageProfile::default();

    let started = Instant::now();
    if options.window == 1 {
        if options.profile_stages {
            for _ in 0..options.iterations {
                let profile = io.transfer_profiled_into(&template, &mut rx)?;
                stage_profile.merge(&profile);
            }
        } else {
            for _ in 0..options.iterations {
                io.transfer(&template, &mut rx)?;
            }
        }
    } else {
        let mut window = io.transfer_window(options.words, options.window)?;
        let initial = options.iterations.min(options.window);
        for _ in 0..initial {
            if options.profile_stages {
                let profile = window.submit_profiled(&template)?;
                stage_profile.merge(&profile);
            } else {
                window.submit(&template)?;
            }
        }

        let mut submitted = initial;
        let mut completed = 0usize;
        while completed < options.iterations {
            let output = outputs[completed % options.window].as_mut_slice();
            if options.profile_stages {
                let profile = window.receive_into_profiled(output)?;
                stage_profile.merge(&profile);
            } else {
                window.receive_into(output)?;
            }
            completed += 1;

            if submitted < options.iterations {
                if options.profile_stages {
                    let profile = window.submit_profiled(&template)?;
                    stage_profile.merge(&profile);
                } else {
                    window.submit(&template)?;
                }
                submitted += 1;
            }
        }
    }
    let elapsed = started.elapsed();
    io.finish()?;

    print_summary(
        "device",
        options.words,
        options.iterations,
        elapsed,
        Some(max_cycles_per_transfer),
    );
    if options.profile_stages {
        print_stage_profile(&stage_profile, elapsed);
    }
    Ok(())
}

fn xor_words(buffer: &mut [u16], key: &[u16; 16]) {
    let mut index = 0usize;
    for word in buffer {
        *word ^= key[index];
        index = (index + 1) & 0x0f;
    }
}

fn print_summary(
    mode: &str,
    words: usize,
    iterations: usize,
    elapsed: Duration,
    max_cycles_per_transfer: Option<usize>,
) {
    let seconds = elapsed.as_secs_f64();
    let transfers_per_sec = iterations as f64 / seconds.max(f64::MIN_POSITIVE);
    let words_per_sec = (iterations * words) as f64 / seconds.max(f64::MIN_POSITIVE);
    let cycles_per_transfer = words / WORDS_PER_CYCLE;
    let cycles_per_sec = words_per_sec / WORDS_PER_CYCLE as f64;
    let max_cycles_per_transfer = max_cycles_per_transfer
        .map(|value| value.to_string())
        .unwrap_or_else(|| "n/a".to_string());

    println!(
        "mode={mode} words={words} cycles_per_transfer={cycles_per_transfer} max_cycles_per_transfer={max_cycles_per_transfer} iterations={iterations} elapsed={elapsed:?} transfers_per_sec={transfers_per_sec:.3} words_per_sec={words_per_sec:.3} cycles_per_sec={cycles_per_sec:.3}"
    );
}

fn print_stage_profile(profile: &TransferStageProfile, elapsed: Duration) {
    let accounted = profile.total_duration();
    let elapsed_secs = elapsed.as_secs_f64().max(f64::MIN_POSITIVE);
    let accounted_secs = accounted.as_secs_f64().max(f64::MIN_POSITIVE);
    let transfers = profile.transfers.max(1);

    println!(
        "profile calls={} transfers={} accounted={:?} wall={:?} unaccounted={:?}",
        profile.calls,
        profile.transfers,
        accounted,
        elapsed,
        elapsed.saturating_sub(accounted),
    );

    for (stage, duration) in [
        ("validation", profile.validation),
        ("setup", profile.setup),
        ("submit", profile.submit),
        ("wait_write", profile.wait_write),
        ("wait_read", profile.wait_read),
        ("decode_copy", profile.decode_copy),
    ] {
        let pct_of_accounted = duration.as_secs_f64() * 100.0 / accounted_secs;
        let pct_of_wall = duration.as_secs_f64() * 100.0 / elapsed_secs;
        let avg_us_per_transfer = duration.as_secs_f64() * 1_000_000.0 / transfers as f64;
        println!(
            "stage={stage} duration={duration:?} pct_accounted={pct_of_accounted:.2} pct_wall={pct_of_wall:.2} avg_us_per_transfer={avg_us_per_transfer:.3}"
        );
    }
}
