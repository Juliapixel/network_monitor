use std::{io::Write, net::IpAddr, time::Duration};

use clap::Parser;
use cli::Args;
use flexi_logger::{style, Age, Cleanup, Criterion, DeferredNow, FileSpec, LogSpecification, Naming};
use log::{debug, error, info, trace, Record};

mod cli;

fn formatter_stderr(write: &mut dyn Write, now: &mut DeferredNow, record: &Record) -> std::io::Result<()>{
    write!(
        write,
        "[{} {} {}] {}",
        now.now(),
        record.target(),
        style(record.level()).paint(record.level().to_string()),
        record.args()
    )
}

fn formatter_file(write: &mut dyn Write, now: &mut DeferredNow, record: &Record) -> std::io::Result<()>{
    write!(
        write,
        "[{} {} {}] {}",
        now.now(),
        record.target(),
        record.level(),
        record.args()
    )
}

#[tokio::main]
async fn main() {
    let args = tokio::task::spawn_blocking(|| { Args::parse() }).await.unwrap();

    let level = match args.verbosity {
        0 => LogSpecification::info(),
        1 => LogSpecification::debug(),
        _ => LogSpecification::trace()
    };

    let logger = flexi_logger::Logger::with(level)
        .duplicate_to_stderr(flexi_logger::Duplicate::All)
        .print_message()
        .rotate(Criterion::Age(Age::Day), Naming::Numbers, Cleanup::KeepCompressedFiles(30))
        .format_for_stderr(formatter_stderr)
        .format_for_files(formatter_file);

    if let Some(dir) = args.out_dir {
        logger.log_to_file(
            FileSpec::default()
                .directory(dir)
        ).start().unwrap();
    } else {
        logger.start().unwrap();
    }

    info!("logging started");
    info!("pinging {} for IPv4", args.hostname.0);
    info!("pinging {} for IPv6", args.hostname.1);

    let (tx, mut rx) = tokio::sync::watch::channel(false);

    // watches for CTRL+C and sends a signal to main thread to gracefully
    // shutdown
    tokio::spawn({
        async move {
                tokio::signal::ctrl_c().await.unwrap();
                trace!("received CTRL+C");
                tx.send(true).unwrap();
                trace!("sent cancellation signal");
            }
        }
    );

    // errors in a row
    let mut v4_errors: usize = 0;
    let mut v6_errors: usize = 0;
    // successes in a row
    let mut v4_successes: usize = 0;
    let mut v6_successes: usize = 0;
    // if there was an error during the last iteration
    let mut v4_error_active = false;
    let mut v6_error_active = false;

    let mut interval = tokio::time::interval(Duration::from_secs(args.interval as u64));

    let v4_ip = IpAddr::V4(args.hostname.0);
    let v6_ip = IpAddr::V6(args.hostname.1);

    loop {
        let payload: [u8; 256] = core::array::from_fn(|i| i as u8);

        let ipv4_ping = surge_ping::ping(v4_ip, &payload);
        let ipv6_ping = surge_ping::ping(v6_ip, &payload);

        let (v4_res, v6_res) = tokio::join!(
            ipv4_ping,
            ipv6_ping
        );

        match v4_res {
            Ok(o) => {
                trace!("{:?}", o.0);
                v4_errors = 0;
                if v4_successes % 10 == 0 {
                    debug!("{} responded in {}ms", v4_ip, o.1.as_millis());
                }
                v4_successes += 1;
            },
            Err(e) => {
                debug!("v4 failed: {e:?}");
                v4_successes = 0;
                v4_errors += 1;
            },
        }

        match v6_res {
            Ok(o) => {
                trace!("{:?}", o.0);
                v6_errors = 0;
                if v6_successes % 10 == 0 {
                    debug!("{} responded in {}ms", v6_ip, o.1.as_millis());
                }
                v6_successes += 1;
            },
            Err(e) => {
                debug!("v6 failed: {e:?}");
                v6_successes = 0;
                v6_errors += 1;
            },
        }

        // small hysteresis to account for random missed pings
        let v4_down = v4_errors >= args.hysteresis as usize;
        let v6_down = v6_errors >= args.hysteresis as usize;

        match (v4_down, v6_down) {
            (true, true) => {
                if !(v4_error_active && v6_error_active) {
                    error!("network is down!");
                }
                (v4_error_active, v6_error_active) = (true, true);
            },
            (true, false) => {
                if !v4_error_active {
                    error!("IPv4 is down!");
                }
                (v4_error_active, v6_error_active) = (true, false);
            },
            (false, true) => {
                if !v6_error_active {
                    error!("IPv6 is down!");
                }
                (v4_error_active, v6_error_active) = (false, true);
            },
            (false, false) => {
                if v4_error_active && v6_error_active {
                    info!("network is back online");
                } else if v4_error_active {
                    info!("IPv4 is back online")
                } else if v6_error_active {
                    info!("IPv6 is back online")
                }
                (v4_error_active, v6_error_active) = (false, false);
            },
        };

        tokio::select! {
            _ = rx.changed() => { break }
            _ = interval.tick() => (),
        }
    }
    info!("logging stopped");
}
