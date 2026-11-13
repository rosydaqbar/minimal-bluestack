use std::{io, net::TcpStream};

// A quiet display may stop producing images. Detect disconnected receivers even
// then, so a dead client cannot monopolize the single frame-broker accept loop.
pub fn check_receiver(socket: &TcpStream) -> io::Result<()> {
    socket.set_nonblocking(true)?;
    let result = socket.peek(&mut [0u8; 1]);
    socket.set_nonblocking(false)?;
    match result {
        Ok(0) => Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "Frame receiver disconnected",
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::{Shutdown, TcpListener},
        time::{Duration, Instant},
    };
    #[test]
    fn quiet_display_releases_a_disconnected_receiver() -> io::Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let client = TcpStream::connect(listener.local_addr()?)?;
        let (server, _) = listener.accept()?;
        check_receiver(&server)?;
        client.shutdown(Shutdown::Both)?;
        let deadline = Instant::now() + Duration::from_secs(1);
        while check_receiver(&server).is_ok() {
            assert!(
                Instant::now() < deadline,
                "Idle broker retained a dead receiver"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }
    #[test]
    fn probing_does_not_consume_the_frame_acknowledgement() -> io::Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let mut client = TcpStream::connect(listener.local_addr()?)?;
        let (mut server, _) = listener.accept()?;
        client.write_all(b"ack")?;
        check_receiver(&server)?;
        server.set_read_timeout(Some(Duration::from_secs(1)))?;
        let mut bytes = [0; 3];
        server.read_exact(&mut bytes)?;
        assert_eq!(&bytes, b"ack");
        Ok(())
    }
}
