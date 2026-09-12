// SPDX-License-Identifier: GPL-3.0-or-later
use airpods_gnome_daemon::ipc::socket_path;
use std::{
    env,
    io::{Read, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
    time::Duration,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let args: Vec<_> = env::args().skip(1).collect();
    anyhow::ensure!(
        args.len() == 1,
        "Usage: airpods-gnome-ctl <status|noise:off|noise:anc|noise:transparency|noise:adaptive|ear:off|ear:one|ear:both|ca:on|ca:off|adaptive:0..100|connect|disconnect>"
    );
    let command = &args[0];
    anyhow::ensure!(
        command.len() <= 128 && !command.contains(['\n', '\r', '\0']),
        "Invalid command"
    );
    let mut socket = UnixStream::connect(socket_path()?)?;
    socket.set_read_timeout(Some(Duration::from_secs(4)))?;
    socket.set_write_timeout(Some(Duration::from_secs(1)))?;
    socket.write_all(command.as_bytes())?;
    socket.write_all(b"\n")?;
    socket.shutdown(Shutdown::Write)?;
    let mut response = String::new();
    socket.take(65_537).read_to_string(&mut response)?;
    anyhow::ensure!(response.len() <= 65_536, "Oversized daemon reply");
    anyhow::ensure!(
        !response.is_empty(),
        "Daemon closed without acknowledging {command}"
    );
    if response.starts_with("error:") {
        anyhow::bail!("{}", response.trim());
    }
    if command == "status" {
        print!("{response}");
    } else {
        anyhow::ensure!(response == "ok\n", "Unexpected daemon reply");
    }
    Ok(())
}
