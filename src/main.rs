use std::{
    error::Error,
    net::{IpAddr, Ipv4Addr, SocketAddr},
};

use base64::{decode, encode};
use clap::Parser;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpListener, TcpStream,
    },
};

#[derive(Parser, Debug)]
pub struct Args {
    /// The port this proxy works on
    proxy_port: u16,
    /// The IP address of the Pi
    pi_ip: IpAddr,
    /// The port to that the Pi speaks katcp over
    #[clap(long, default_value_t = 7147)]
    pi_port: u16,
}

// We need to intercept !read and !write messages
fn bad_to_good(line: &[u8]) -> String {
    if &line[0..9] == b"!read ok " {
        // Everything after this point is raw binary, escape to base64
        format!("!read ok {}", encode(&line[9..]))
    } else if &line[0..7] == b"?write " {
        // Keep grabbing bytes, breaking on spaces, for the name and offset
        let mut register_name = String::new();
        let mut offset = String::new();

        let mut byte_ptr = 7usize;

        // First get the register name
        loop {
            let next_char = line[byte_ptr] as char;
            byte_ptr += 1;
            if next_char == ' ' {
                break;
            } else {
                register_name.push(next_char);
            }
        }

        // Then get the offset
        loop {
            let next_char = line[byte_ptr] as char;
            byte_ptr += 1;
            if next_char == ' ' {
                break;
            } else {
                offset.push(next_char);
            }
        }

        // Then deal with bytes
        format!(
            "?write {} {} {}",
            register_name,
            offset,
            encode(&line[byte_ptr..])
        )
    } else {
        std::str::from_utf8(line).unwrap().to_owned()
    }
}

fn good_to_bad(line: &str) -> Vec<u8> {
    let components: Vec<&str> = line.split(' ').collect();
    if components.first().unwrap() == &"!read" {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"!read ok ");
        bytes.extend(decode(components.get(2).unwrap()).unwrap());
        bytes
    } else if components.first().unwrap() == &"?write" {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"?write ");
        bytes.extend_from_slice(components.get(1).unwrap().as_bytes());
        bytes.push(b' ');
        bytes.extend_from_slice(components.get(2).unwrap().as_bytes());
        bytes.push(b' ');
        bytes.extend(decode(components.get(3).unwrap()).unwrap());
        bytes
    } else {
        line.as_bytes().to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bad_to_good() {
        let deadbeef = [0xde, 0xad, 0xbe, 0xef];
        let mut bad_read = [0u8; 13];
        bad_read[0..9].clone_from_slice(b"!read ok ");
        bad_read[9..].clone_from_slice(&deadbeef);
        assert_eq!(bad_to_good(&bad_read), "!read ok 3q2+7w==".to_owned());
        let mut bad_write = [0u8; 28];
        bad_write[0..24].clone_from_slice(b"?write sys_scratchpad 0 ");
        bad_write[24..].clone_from_slice(&deadbeef);
        assert_eq!(bad_to_good(&bad_write), "?write sys_scratchpad 0 3q2+7w==");
        let normal_command = b"?help my_device";
        assert_eq!(
            bad_to_good(normal_command),
            std::str::from_utf8(normal_command).unwrap()
        );
    }

    #[test]
    fn test_good_to_bad() {
        let good_write = "?write sys 0 3q2+7w==";
        let good_read = "!read ok 3q2+7w==";
        let normal_command = "?help my_device";
        assert_eq!(good_to_bad(good_read), vec![
            b'!', b'r', b'e', b'a', b'd', b' ', b'o', b'k', b' ', 0xde, 0xad, 0xbe, 0xef
        ]);
        assert_eq!(good_to_bad(good_write), vec![
            b'?', b'w', b'r', b'i', b't', b'e', b' ', b's', b'y', b's', b' ', b'0', b' ', 0xde,
            0xad, 0xbe, 0xef
        ]);
        assert_eq!(good_to_bad(normal_command), normal_command.as_bytes());
    }

    #[test]
    fn test_roundtrip() {
        let good_write = "?write sys 0 3q2+7w==";
        let good_read = "!read ok 3q2+7w==";
        let normal_command = "?help my_device";

        assert_eq!(bad_to_good(&good_to_bad(good_write)), good_write);
        assert_eq!(bad_to_good(&good_to_bad(good_read)), good_read);
        assert_eq!(bad_to_good(&good_to_bad(normal_command)), normal_command);
    }
}

async fn transform_from_pi(mut writer: OwnedWriteHalf, reader: OwnedReadHalf) {
    let mut lines = BufReader::new(reader);
    let mut buf = Vec::new();
    loop {
        lines
            .read_until(b'\n', &mut buf)
            .await
            .expect("Error awaiting for an incoming line. This was probably a socket error?");
        // Process (removing the newline) and send to the proxy
        buf.pop();
        if buf.last().unwrap() == &b'\r' {
            buf.pop();
        }
        writer
            .write_all(bad_to_good(&buf).as_bytes())
            .await
            .unwrap();
        writer.write_all(&[b'\n']).await.unwrap();
        buf.clear();
    }
}

async fn transform_from_proxy(mut writer: OwnedWriteHalf, reader: OwnedReadHalf) {
    let mut lines = BufReader::new(reader);
    let mut buf = Vec::new();
    loop {
        lines
            .read_until(b'\n', &mut buf)
            .await
            .expect("Error awaiting for an incoming line. This was probably a socket error?");
        // Process (removing the newline) and send to the proxy
        buf.pop();
        if buf.last().unwrap() == &b'\r' {
            buf.pop();
        }
        writer
            .write_all(&good_to_bad(std::str::from_utf8(&buf).unwrap()))
            .await
            .unwrap();
        writer.write_all(&[b'\n']).await.unwrap();
        buf.clear();
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    // Construct socket addrs
    let pi_addr = SocketAddr::new(args.pi_ip, args.pi_port);
    let proxy_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), args.proxy_port);
    // Connect to streams
    println!("Connected to Pi");
    let (pi_read, pi_write) = TcpStream::connect(pi_addr).await?.into_split();
    let listener = TcpListener::bind(proxy_addr).await?;
    println!("Waiting for proxy connection");
    let (proxy_read, proxy_write) = listener.accept().await?.0.into_split();
    println!("Connection made, proxy running");

    tokio::spawn(transform_from_pi(proxy_write, pi_read));
    transform_from_proxy(pi_write, proxy_read).await;
    Ok(())
}
