use std::{
    env,
    error::Error,
    process,
    time::{Duration, Instant},
};
use vlfd_rs::{Board, IoConfig, TransportConfig};

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
        "Usage:\n  cargo run --example bench_transfer -- cpu [--words N] [--iterations N]\n  cargo run --example bench_transfer -- device [--words N] [--iterations N] [--clock-high N] [--clock-low N] [--usb-timeout-ms N] [--sync-timeout-ms N] [--reset-on-open] [--no-clear-halt]"
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

    print_summary("cpu", options.words, options.iterations, elapsed);
    Ok(())
}

fn run_device_bench(options: &Options) -> Result<(), Box<dyn Error>> {
    let mut board = Board::open_with_transport(options.transport)?;
    let mut io = board.configure_io(&IoConfig {
        clock_high_delay: options.clock_high_delay,
        clock_low_delay: options.clock_low_delay,
        ..IoConfig::default()
    })?;

    let template = vec![0x1234u16; options.words];
    let mut rx = vec![0u16; options.words];

    let started = Instant::now();
    for _ in 0..options.iterations {
        io.transfer(&template, &mut rx)?;
    }
    let elapsed = started.elapsed();
    io.finish()?;

    print_summary("device", options.words, options.iterations, elapsed);
    Ok(())
}

fn xor_words(buffer: &mut [u16], key: &[u16; 16]) {
    let mut index = 0usize;
    for word in buffer {
        *word ^= key[index];
        index = (index + 1) & 0x0f;
    }
}

fn print_summary(mode: &str, words: usize, iterations: usize, elapsed: Duration) {
    let seconds = elapsed.as_secs_f64();
    let transfers_per_sec = iterations as f64 / seconds.max(f64::MIN_POSITIVE);
    let words_per_sec = (iterations * words) as f64 / seconds.max(f64::MIN_POSITIVE);

    println!(
        "mode={mode} words={words} iterations={iterations} elapsed={elapsed:?} transfers_per_sec={transfers_per_sec:.3} words_per_sec={words_per_sec:.3}"
    );
}
