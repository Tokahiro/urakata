//! Host-test helper: adds one SudoVDA display without a Steam stream, keeps
//! it alive with a ping each second, and removes it when a stop file
//! appears or the time limit passes. The watcher sees it like a stream.
//!
//! Usage: fake-display --stop-file <path> [--seconds <n>]
//!
//! If this process dies, the driver's watchdog removes the display after
//! its timeout (3 s by default).

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{} error: {e}", now());
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("fake-display runs on Windows only");
}

/// Seconds since the Unix epoch. Wall clock, so time asleep counts.
#[cfg(windows)]
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(windows)]
fn run() -> Result<(), String> {
    use sudovda::{AddParams, Device, Guid, RemoveParams};

    let mut stop_file = None;
    let mut seconds: u64 = 600;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--stop-file" => stop_file = args.next().map(std::path::PathBuf::from),
            "--seconds" => {
                seconds = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--seconds needs a number")?;
            }
            _ => return Err(format!("unknown argument '{a}'")),
        }
    }
    let stop_file = stop_file.ok_or("--stop-file is required")?;

    // Fixed identity, so that every run is the same monitor for Windows.
    let guid = Guid {
        data1: 0x7572_616b,
        data2: 0x6174,
        data3: 0x0014,
        data4: *b"urk-test",
    };
    let mut name = [0u8; 14];
    name[..12].copy_from_slice(b"urakata-test");
    let mut serial = [0u8; 14];
    serial[..3].copy_from_slice(b"URK");
    let params = AddParams {
        width: 1920,
        height: 1080,
        refresh_rate: 60,
        monitor_guid: guid,
        device_name: name,
        serial_number: serial,
    };

    let device = Device::open().map_err(|e| format!("open: {e}"))?;
    let out = device
        .add_display(&params)
        .map_err(|e| format!("add: {e}"))?;
    println!(
        "{} added: adapter {:08x}-{:08x}, target {}",
        now(),
        out.adapter_luid.high_part,
        out.adapter_luid.low_part,
        out.target_id
    );

    let end = now() + seconds;
    let mut failed = false;
    while !stop_file.exists() && now() < end {
        std::thread::sleep(std::time::Duration::from_secs(1));
        match device.ping() {
            Ok(()) if failed => {
                println!("{} ping works again", now());
                failed = false;
            }
            Ok(()) => {}
            Err(e) if !failed => {
                println!("{} ping failed: {e}", now());
                failed = true;
            }
            Err(_) => {}
        }
    }
    let why = if stop_file.exists() {
        "stop file"
    } else {
        "time limit"
    };
    match device.remove_display(&RemoveParams { monitor_guid: guid }) {
        Ok(()) => println!("{} removed ({why})", now()),
        Err(e) => println!("{} remove failed ({why}): {e}", now()),
    }
    Ok(())
}
