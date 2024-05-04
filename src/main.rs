use std::{io::Write, net::IpAddr, time::{Duration, Instant}};

use flexi_logger::{style, Cleanup, Criterion, DeferredNow, FileSpec, LogSpecification, Naming};
use futures_util::{FutureExt, StreamExt};
use log::{debug, error, info, trace, warn, Record};
use once_cell::sync::Lazy;
use tokio::select;

use crate::cli::ARGS;

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

#[cfg(unix)]
async fn watch_sigs(tx: tokio::sync::watch::Sender<bool>) {
    use tokio::signal::unix::SignalKind;

    let mut sigint = tokio::signal::unix::signal(SignalKind::interrupt()).unwrap();
    let mut sigterm = tokio::signal::unix::signal(SignalKind::terminate()).unwrap();
    let mut sigquit = tokio::signal::unix::signal(SignalKind::quit()).unwrap();
    let sig = futures_util::future::select_all([
        Box::pin(sigint.recv()),
        Box::pin(sigterm.recv()),
        Box::pin(sigquit.recv()),
    ]).await;
    match sig.1 {
        0 => {info!("received SIGINT, terminating")},
        1 => {info!("received SIGTERM, terminating")},
        2 => {info!("received SIGQUIT, terminating")},
        _ => unreachable!()
    };
    tx.send(true).unwrap();
    trace!("sent cancellation signal");
}

#[cfg(windows)]
async fn watch_sigs(tx: tokio::sync::watch::Sender<bool>) {
    tokio::signal::windows::ctrl_c().unwrap().recv().await;
    info!("received CTRL+C, terminating");
    tx.send(true).unwrap();
    trace!("sent cancellation signal");
}

async fn monitor_ip(addr: IpAddr, is_error: tokio::sync::mpsc::Sender<bool>) {
    let payload: [u8; 256] = core::array::from_fn(|i| i as u8);
    let mut errors: u32 = 0;

    let mut interval = tokio::time::interval(Duration::from_secs(ARGS.interval));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        let ping = surge_ping::ping(addr, &payload).await;

        match ping {
            Ok(ping) => {
                errors = 0;
                debug!("ping to {addr} successful");
                trace!("{ping:?}");
            },
            Err(e) => {
                errors += 1;
                warn!("ping to {addr} failed {errors} times");
                debug!("{e:?}");
            },
        }

        if errors >= ARGS.hysteresis {
            is_error.send(true).await.expect("receiver died");
        } else {
            is_error.send(false).await.expect("receiver died");
        }

        interval.tick().await;
    }

}

#[tokio::main]
async fn main() {
    // gotta init this outside of the async runtime
    tokio::task::spawn_blocking(|| Lazy::force(&ARGS)).await.unwrap();

    let level = match ARGS.verbosity {
        0 => LogSpecification::info(),
        1 => LogSpecification::debug(),
        _ => LogSpecification::trace()
    };

    let logger = flexi_logger::Logger::with(level)
        .duplicate_to_stderr(flexi_logger::Duplicate::All)
        .print_message()
        .rotate(Criterion::Size(1024 * 1024 * 5), Naming::Numbers, Cleanup::KeepCompressedFiles(30))
        .format_for_stderr(formatter_stderr)
        .format_for_files(formatter_file);

    if let Some(dir) = &ARGS.out_dir {
        logger.log_to_file(
            FileSpec::default()
                .directory(dir)
        ).start().unwrap();
    } else {
        logger.start().unwrap();
    }

    info!("logging started");

    debug!("{:#?}", *ARGS);

    info!("monitoring started, pinging {} and {} every {}s", ARGS.hostname.0, ARGS.hostname.1, ARGS.interval);

    let (tx, mut rx) = tokio::sync::watch::channel(false);

    // watches for termination signals and sends a signal to main thread to
    // gracefully shutdown
    tokio::spawn(watch_sigs(tx));

    let (v4_tx, mut v4_rx) = tokio::sync::mpsc::channel(16);
    let v4_thread = tokio::spawn(monitor_ip(ARGS.hostname.0.into(), v4_tx));

    let (v6_tx, mut v6_rx) = tokio::sync::mpsc::channel(16);
    let v6_thread = tokio::spawn(monitor_ip(ARGS.hostname.1.into(), v6_tx));

    let mut v4_down = false;
    let mut v6_down = false;

    let mut v4_error_active = false;
    let mut v6_error_active = false;

    let mut v4_error_start: Option<Instant> = None;
    let mut v6_error_start: Option<Instant> = None;

    // if this stream ever yields anything then quit
    let mut should_end = futures_util::future::select(
        futures_util::future::select_all([v4_thread, v6_thread]),
        Box::pin(rx.changed())
    ).into_stream();

    // TODO: make this not as stupid
    loop {
        select! {
            v4 = v4_rx.recv() => {
                v4_down = v4.unwrap();
            },
            v6 = v6_rx.recv() => {
                v6_down = v6.unwrap();
            },
            _ = should_end.next() => { break }
        }

        // this is an abomination
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
                if v6_error_active {
                    if let Some(start) = v6_error_start {
                        let secs = start.elapsed().as_secs();
                        info!(
                            "IPv6 is back online, and was down for {:02}:{:02}:{:02}",
                            secs / 3600,
                            (secs / 60) % 60,
                            secs % 60
                        );
                        v6_error_start = None;
                    } else {
                        info!("IPv6 is back online;")
                    }
                }
                (v4_error_active, v6_error_active) = (true, false);
            },
            (false, true) => {
                if !v6_error_active {
                    error!("IPv6 is down!");
                }
                if v4_error_active {
                    if let Some(start) = v4_error_start {
                        let secs = start.elapsed().as_secs();
                        info!(
                            "IPv4 is back online, and was down for {:02}:{:02}:{:02}",
                            secs / 3600,
                            (secs / 60) % 60,
                            secs % 60
                        );
                        v4_error_start = None;
                    } else {
                        info!("IPv4 is back online")
                    }
                }
                (v4_error_active, v6_error_active) = (false, true);
            },
            (false, false) => {
                if v4_error_active && v6_error_active {
                    if let Some(start) = v4_error_start {
                        let secs = start.elapsed().as_secs();
                        info!(
                            "network is back online, and was down for {:02}:{:02}:{:02}",
                            secs / 3600,
                            (secs / 60) % 60,
                            secs % 60
                        );
                        v4_error_start = None;
                    } else {
                        info!("network is back online")
                    }
                } else if v4_error_active {
                    if let Some(start) = v4_error_start {
                        let secs = start.elapsed().as_secs();
                        info!(
                            "IPv4 is back online, and was down for {:02}:{:02}:{:02}",
                            secs / 3600,
                            (secs / 60) % 60,
                            secs % 60
                        );
                        v4_error_start = None;
                    } else {
                        info!("IPv4 is back online")
                    }
                } else if v6_error_active {
                    if let Some(start) = v6_error_start {
                        let secs = start.elapsed().as_secs();
                        info!(
                            "IPv6 is back online, and was down for {:02}:{:02}:{:02}",
                            secs / 3600,
                            (secs / 60) % 60,
                            secs % 60
                        );
                        v6_error_start = None;
                    } else {
                        info!("IPv6 is back online")
                    }
                }
                (v4_error_active, v6_error_active) = (false, false);
            },
        };
    }
    info!("logging stopped");
}
