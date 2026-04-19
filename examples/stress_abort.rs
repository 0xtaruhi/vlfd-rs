use std::{env, error::Error, process, thread, time::Duration};

use vlfd_rs::{Board, IoConfig};

fn main() {
    if let Err(err) = real_main() {
        eprintln!("error: {err}");
        process::exit(1);
    }
}

fn real_main() -> Result<(), Box<dyn Error>> {
    let mut iterations = 10usize;
    let mut words = 512usize;
    let mut window = 4usize;
    let mut clock_high = 4u16;
    let mut clock_low = 4u16;
    let mut settle_ms = 0u64;

    let mut args = env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--iterations" => {
                iterations = args
                    .next()
                    .ok_or("missing value for --iterations")?
                    .parse()?
            }
            "--words" => words = args.next().ok_or("missing value for --words")?.parse()?,
            "--window" => window = args.next().ok_or("missing value for --window")?.parse()?,
            "--clock-high" => {
                clock_high = args
                    .next()
                    .ok_or("missing value for --clock-high")?
                    .parse()?
            }
            "--clock-low" => {
                clock_low = args
                    .next()
                    .ok_or("missing value for --clock-low")?
                    .parse()?
            }
            "--settle-ms" => {
                settle_ms = args
                    .next()
                    .ok_or("missing value for --settle-ms")?
                    .parse()?
            }
            other => return Err(format!("unknown flag `{other}`").into()),
        }
    }

    let tx = vec![0x1234u16; words];

    for iteration in 0..iterations {
        println!("iter={iteration} phase=open");
        let mut board = Board::open()?;
        println!(
            "iter={iteration} config programmed={} fifo_words={} version=0x{:04x}",
            board.config().is_programmed(),
            board.config().fifo_size_words(),
            board.config().smims_version_raw()
        );

        let mut io = board.configure_io(&IoConfig {
            clock_high_delay: clock_high,
            clock_low_delay: clock_low,
            ..IoConfig::default()
        })?;

        {
            let mut rolling = io.transfer_window(words, window)?;
            for submit_index in 0..window {
                rolling.submit(&tx)?;
                println!("iter={iteration} submit={submit_index}");
            }
            if settle_ms > 0 {
                thread::sleep(Duration::from_millis(settle_ms));
            }
        }

        io.finish()?;
        board.close()?;

        println!("iter={iteration} phase=reopen");
        let reopened = Board::open()?;
        println!(
            "iter={iteration} reopened programmed={} fifo_words={} version=0x{:04x}",
            reopened.config().is_programmed(),
            reopened.config().fifo_size_words(),
            reopened.config().smims_version_raw()
        );
        reopened.close()?;
    }

    Ok(())
}
