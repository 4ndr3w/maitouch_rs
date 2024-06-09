use anyhow::Result;
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;
use std::thread;
use std::time::Duration;
use structopt::StructOpt;

mod io;

const MAX_MESSAGE_SIZE: usize = 6;
const TOUCH_PACKET_SIZE: usize = 9;
const HALT_COMMAND: &[u8] = "{HALT}".as_bytes();
const RESET_COMMAND: &[u8] = "{RSET}".as_bytes();
const STAT_COMMAND: &[u8] = "{STAT}".as_bytes();

#[derive(PartialEq)]
enum CommandType {
    Halt,
    Stat,
    Reset,
    Config,
}

struct PacketDelimiter {
    pub open: char,
    pub close: char,
}

const ALLS_PACKET: PacketDelimiter = PacketDelimiter {
    open: '{',
    close: '}',
};
const ADX_PACKET: PacketDelimiter = PacketDelimiter {
    open: '(',
    close: ')',
};

fn read_packet(
    buffer: &mut Vec<u8>,
    reader: &mut dyn BufRead,
    packet: &PacketDelimiter,
) -> std::io::Result<()> {
    buffer.clear();
    buffer.push(packet.open as u8);
    tracing::trace!("skip_until");
    loop {
        match io::skip_until(reader, packet.open as u8) {
            Err(err) => {
                if err.kind() != std::io::ErrorKind::TimedOut {
                    return Err(err);
                }
            }
            Ok(_) => break,
        }
    }
    tracing::trace!("read_until");
    loop {
        match reader.read_until(packet.close as u8, buffer) {
            Err(err) => {
                if err.kind() != std::io::ErrorKind::TimedOut {
                    return Err(err);
                }
            }
            Ok(_) => break,
        }
    }
    Ok(())
}

fn get_command_type(buffer: &Vec<u8>) -> CommandType {
    match buffer.as_slice() {
        HALT_COMMAND => CommandType::Halt,
        STAT_COMMAND => CommandType::Stat,
        RESET_COMMAND => CommandType::Reset,
        _ => CommandType::Config,
    }
}

fn stat_mode(
    adx_reader: &mut (dyn BufRead + Send),
    adx_writer: &mut dyn Write,
    alls_reader: &mut (dyn BufRead + Send),
    alls_writer: &mut (dyn Write + Send),
) -> Result<()> {
    tracing::info!("Streaming mode");
    let run_flag = AtomicBool::new(true);
    let state_buffer = RwLock::new([0u8; TOUCH_PACKET_SIZE]);

    thread::scope(|scope| {
        // Read the latest touch update
        scope.spawn(|| {
            let mut local_buf = Vec::<u8>::with_capacity(TOUCH_PACKET_SIZE);
            while run_flag.load(Ordering::Relaxed) {
                read_packet(&mut local_buf, adx_reader, &ADX_PACKET).unwrap();
                if local_buf.len() != TOUCH_PACKET_SIZE {
                    tracing::info!(
                        "Couldn't forward touch packet, buf was {} expected {}",
                        local_buf.len(),
                        TOUCH_PACKET_SIZE
                    );
                    continue;
                }
                {
                    let mut locked_buf = state_buffer.write().unwrap();
                    locked_buf.as_mut().copy_from_slice(local_buf.as_slice());
                }
            }
        });

        // Write the latest touch update
        scope.spawn(|| {
            while run_flag.load(Ordering::Relaxed) {
                {
                    let locked_buf = state_buffer.read().unwrap();
                    alls_writer.write_all(locked_buf.as_ref()).unwrap();
                }
                alls_writer.flush().unwrap();
            }
        });

        // Watch for halt
        scope.spawn(|| {
            let mut command_buffer = Vec::<u8>::with_capacity(MAX_MESSAGE_SIZE);
            loop {
                match read_packet(&mut command_buffer, alls_reader, &ALLS_PACKET) {
                    Err(err) => {
                        if err.kind() != std::io::ErrorKind::TimedOut {
                            return;
                        }
                    }
                    Ok(_) => {
                        if get_command_type(&command_buffer) == CommandType::Halt {
                            tracing::info!("HALT command in streaming mode");
                            run_flag.store(false, Ordering::Relaxed);
                            break;
                        }
                    }
                }
            }
        });
    });

    drain_and_reset(adx_reader, adx_writer)?;

    Ok(())
}

fn drain_and_reset(
    adx_read: &mut (dyn BufRead),
    adx_write: &mut (dyn Write),
) -> std::io::Result<()> {
    tracing::info!("Halting and clearing ADX read buffer");

    adx_write.write_all(RESET_COMMAND)?;
    adx_write.write_all(HALT_COMMAND)?;
    let mut buf = Vec::<u8>::with_capacity(TOUCH_PACKET_SIZE);

    loop {
        match adx_read.read_until(ADX_PACKET.close as u8, &mut buf) {
            Ok(bytes) => {
                tracing::info!("read {}", bytes);
                buf.clear();
            }
            Err(err) => {
                if err.kind() == std::io::ErrorKind::TimedOut {
                    tracing::info!("timeout");
                    return Ok(());
                }
                return Err(err);
            }
        }
    }
}

fn run_touch_proxy(config: &Config) -> Result<()> {
    let mut alls_reader_port = serialport::new(&config.alls, 9600)
        .timeout(Duration::from_secs(1))
        .open()?;
    let mut alls_writer = alls_reader_port.try_clone()?;
    let mut alls_reader = BufReader::new(&mut alls_reader_port);

    let mut adx_reader_port = serialport::new(&config.adx, 9600)
        .timeout(Duration::from_secs(1))
        .open()?;
    let mut adx_writer = adx_reader_port.try_clone()?;
    let mut adx_reader = BufReader::new(&mut adx_reader_port);

    drain_and_reset(&mut adx_reader, &mut adx_writer)?;

    tracing::info!("Ports opened");

    let mut command_buffer = Vec::<u8>::with_capacity(MAX_MESSAGE_SIZE);

    // At startup, the ADX is in config mode.
    // ALLS will send message to it, ADX will responds until streaming is enabled.
    tracing::info!("Read loop started");
    loop {
        read_packet(&mut command_buffer, &mut alls_reader, &ALLS_PACKET)?;
        adx_writer.write_all(&command_buffer)?;

        let cmd_str = String::from_utf8_lossy(&command_buffer);
        tracing::info!("From ALLS: {}", cmd_str);

        match get_command_type(&command_buffer) {
            CommandType::Config => {
                read_packet(&mut command_buffer, &mut adx_reader, &ADX_PACKET)?;
                let resp_str = String::from_utf8_lossy(&command_buffer);
                tracing::info!("From ADX: {}", resp_str);
                alls_writer.write_all(&command_buffer)?;
                alls_writer.flush()?;
            }
            CommandType::Stat => {
                stat_mode(
                    &mut adx_reader,
                    &mut adx_writer,
                    &mut alls_reader,
                    &mut alls_writer,
                )?;
            }
            _ => (),
        };
    }
}

#[derive(Debug, StructOpt)]
struct Config {
    pub alls: String,
    pub adx: String,
}

fn main() {
    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(tracing::Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");
    let config = Config::from_args();
    tracing::info!("ALLS {} ADX {}", config.alls, config.adx);
    run_touch_proxy(&config).unwrap();
}
