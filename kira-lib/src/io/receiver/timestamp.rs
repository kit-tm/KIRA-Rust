//! Receiving datagrams together with the time the kernel received them.
//!
//! A message can wait in the socket's receive buffer (and then in the channel to the protocol
//! instance) before it is processed, e.g., while the node is overloaded. The time of receipt
//! lets the protocol instance handle the message before timers that became due only while it
//! waited, and measure how far processing lags behind.

use std::{
    io,
    net::SocketAddr,
    time::{
        Duration,
        Instant,
    },
};

use tokio::net::UdpSocket;

/// Upper bound of the age of a datagram derived from its kernel timestamp.
///
/// The kernel stamps datagrams with the realtime clock, which may jump.
const MAX_AGE: Duration = Duration::from_secs(60);

/// Lets the kernel record the time of receipt of the datagrams of the socket.
pub(super) fn enable(socket: &UdpSocket) -> io::Result<()> {
    imp::enable(socket)
}

/// Receives a datagram like [UdpSocket::recv_from] and returns when the kernel received it.
///
/// Without a kernel timestamp the time of reading it is returned.
pub(super) async fn recv_from(
    socket: &UdpSocket,
    buffer: &mut [u8],
) -> io::Result<(usize, SocketAddr, Instant)> {
    let (len, from, age) = imp::recv_from(socket, buffer).await?;
    let now = Instant::now();
    let received_at = age
        .and_then(|age| now.checked_sub(age.min(MAX_AGE)))
        .unwrap_or(now);
    Ok((len, from, received_at))
}

#[cfg(target_os = "linux")]
mod imp {
    use std::{
        io::{
            self,
            IoSliceMut,
        },
        net::{
            SocketAddr,
            SocketAddrV6,
        },
        time::{
            Duration,
            SystemTime,
            UNIX_EPOCH,
        },
    };

    use nix::{
        cmsg_space,
        sys::{
            socket::{
                ControlMessageOwned,
                MsgFlags,
                SockaddrIn6,
                recvmsg,
                setsockopt,
                sockopt::ReceiveTimestampns,
            },
            time::TimeSpec,
        },
    };
    use tokio::{
        io::Interest,
        net::UdpSocket,
    };

    pub(super) fn enable(socket: &UdpSocket) -> io::Result<()> {
        setsockopt(socket, ReceiveTimestampns, &true).map_err(io::Error::from)
    }

    /// Returns the length, sender and age of the datagram (None without kernel timestamp).
    pub(super) async fn recv_from(
        socket: &UdpSocket,
        buffer: &mut [u8],
    ) -> io::Result<(usize, SocketAddr, Option<Duration>)> {
        let (len, from, stamp) = socket
            .async_io(Interest::READABLE, || recv_with_stamp(socket, buffer))
            .await?;
        let age = stamp.map(|stamp| SystemTime::now().duration_since(stamp).unwrap_or_default());
        Ok((len, from, age))
    }

    fn recv_with_stamp(
        socket: &UdpSocket,
        buffer: &mut [u8],
    ) -> io::Result<(usize, SocketAddr, Option<SystemTime>)> {
        use std::os::fd::AsRawFd;

        let mut iov = [IoSliceMut::new(buffer)];
        let mut control = cmsg_space!(TimeSpec);
        let msg = recvmsg::<SockaddrIn6>(
            socket.as_raw_fd(),
            &mut iov,
            Some(&mut control),
            MsgFlags::empty(),
        )
        .map_err(io::Error::from)?;

        let from = msg.address.map(SocketAddrV6::from).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "received datagram from a non-IPv6 address",
            )
        })?;
        let mut stamp = None;
        for cmsg in msg.cmsgs().map_err(io::Error::from)? {
            if let ControlMessageOwned::ScmTimestampns(ts) = cmsg {
                stamp = u64::try_from(ts.tv_sec()).ok().and_then(|secs| {
                    UNIX_EPOCH.checked_add(Duration::new(secs, ts.tv_nsec() as u32))
                });
            }
        }
        Ok((msg.bytes, SocketAddr::V6(from), stamp))
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use std::{
        io,
        net::SocketAddr,
        time::Duration,
    };

    use tokio::net::UdpSocket;

    pub(super) fn enable(_socket: &UdpSocket) -> io::Result<()> {
        Ok(())
    }

    pub(super) async fn recv_from(
        socket: &UdpSocket,
        buffer: &mut [u8],
    ) -> io::Result<(usize, SocketAddr, Option<Duration>)> {
        let (len, from) = socket.recv_from(buffer).await?;
        Ok((len, from, None))
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::time::Instant;

    use super::*;

    #[tokio::test]
    async fn receives_datagram_with_time_of_receipt() {
        let receiver = UdpSocket::bind("[::1]:0").await.unwrap();
        enable(&receiver).unwrap();
        let sender = UdpSocket::bind("[::1]:0").await.unwrap();

        let sent = Instant::now();
        sender
            .send_to(b"hello", receiver.local_addr().unwrap())
            .await
            .unwrap();
        // the datagram waits in the receive buffer
        std::thread::sleep(Duration::from_millis(50));

        let mut buffer = [0u8; 64];
        let (len, from, received_at) = recv_from(&receiver, &mut buffer).await.unwrap();
        assert_eq!(&buffer[..len], b"hello");
        assert_eq!(from, sender.local_addr().unwrap());
        // stamped on arrival, not when read 50ms later
        let waited = Instant::now() - received_at;
        assert!(waited >= Duration::from_millis(40), "{waited:?}");
        assert!(
            received_at >= sent - Duration::from_millis(5),
            "{:?}",
            received_at - sent
        );
    }
}
