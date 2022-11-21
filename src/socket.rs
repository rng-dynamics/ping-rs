use std::io;
use std::time::Duration;

use socket2::{Domain, Protocol, Type};

pub(crate) trait Socket: Send + Sync {
    fn send_to(&self, buf: &[u8], addr: &socket2::SockAddr) -> io::Result<usize>;

    fn recv_from(
        &self,
        buf: &mut [std::mem::MaybeUninit<u8>],
    ) -> io::Result<(usize, socket2::SockAddr)>;
}

impl Socket for socket2::Socket {
    fn send_to(&self, buf: &[u8], addr: &socket2::SockAddr) -> io::Result<usize> {
        self.send_to(buf, addr)
    }

    fn recv_from(
        &self,
        buf: &mut [std::mem::MaybeUninit<u8>],
    ) -> io::Result<(usize, socket2::SockAddr)> {
        socket2::Socket::recv_from(self, buf)
    }
}

pub(crate) fn create_socket2_dgram_socket(timeout: Duration) -> Result<socket2::Socket, io::Error> {
    let socket = socket2::Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::ICMPV4))?;
    socket
        .set_read_timeout(Some(timeout))
        .expect("could not set socket timeout");
    Ok(socket)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use std::net::SocketAddr;
    use std::sync::Mutex;

    use pnet_packet::icmp::checksum;
    use pnet_packet::icmp::echo_reply::EchoReplyPacket;
    use pnet_packet::icmp::echo_reply::MutableEchoReplyPacket;
    use pnet_packet::icmp::IcmpCode;
    use pnet_packet::icmp::IcmpPacket;
    use pnet_packet::icmp::IcmpType;
    use pnet_packet::Packet;
    use pnet_packet::PacketSize;

    #[derive(PartialEq, Eq)]
    pub(crate) enum OnSend {
        ReturnErr,
        ReturnDefault,
    }

    #[derive(PartialEq, Eq, Clone, Copy)]
    pub(crate) enum OnReceive {
        ReturnWouldBlock,
        ReturnDefault(usize),
    }

    pub(crate) struct SocketMock {
        on_send: OnSend,
        on_receive: Mutex<OnReceive>,
        sent: Mutex<Vec<(Vec<u8>, socket2::SockAddr)>>,
        received_cnt: Mutex<usize>,
    }

    impl SocketMock {
        pub(crate) fn new(on_send: OnSend, on_receive: OnReceive) -> Self {
            Self {
                on_send,
                on_receive: Mutex::new(on_receive),
                sent: Mutex::new(vec![]),
                received_cnt: Mutex::new(0),
            }
        }

        pub(crate) fn should_send_number_of_messages(&self, n: usize) -> &Self {
            assert!(n == self.sent.lock().unwrap().len());
            self
        }

        pub(crate) fn should_send_to_address(&self, addr: &socket2::SockAddr) -> &Self {
            assert!(self
                .sent
                .lock()
                .unwrap()
                .iter()
                .any(|e| addr.as_socket() == e.1.as_socket()));
            self
        }

        pub(crate) fn should_receive_number_of_messages(&self, n: usize) -> &Self {
            assert!(n == *self.received_cnt.lock().unwrap());
            self
        }
    }

    impl crate::Socket for SocketMock {
        fn send_to(&self, buf: &[u8], addr: &socket2::SockAddr) -> io::Result<usize> {
            if self.on_send == OnSend::ReturnErr {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "simulating error in mock",
                ));
            }

            self.sent.lock().unwrap().push((buf.to_vec(), addr.clone()));
            Ok(buf.len())
        }

        fn recv_from(
            &self,
            buf: &mut [std::mem::MaybeUninit<u8>],
        ) -> io::Result<(usize, socket2::SockAddr)> {
            let on_receive: OnReceive = *self.on_receive.lock().unwrap();
            match on_receive {
                OnReceive::ReturnWouldBlock => {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "simulating would-block in mock",
                    ));
                }
                OnReceive::ReturnDefault(cnt) => {
                    *self.on_receive.lock().unwrap() = if cnt <= 1 {
                        OnReceive::ReturnWouldBlock
                    } else {
                        OnReceive::ReturnDefault(cnt - 1)
                    };
                }
            };

            let payload: Vec<u8> = vec![0xFF, 0xFF, 0xFF, 0xFF];
            if buf.len() < EchoReplyPacket::minimum_packet_size() + payload.len() {
                return Err(io::Error::new(io::ErrorKind::Other, "buffer too small"));
            }

            let mut received_cnt = self.received_cnt.lock().unwrap();
            *received_cnt += 1;

            let buf2 = vec![0u8; EchoReplyPacket::minimum_packet_size() + payload.len()];
            let mut packet: MutableEchoReplyPacket<'_> =
                MutableEchoReplyPacket::owned(buf2).unwrap();
            packet.set_icmp_type(IcmpType::new(0)); // echo reply
            packet.set_icmp_code(IcmpCode::new(0)); // echo reply
            packet.set_identifier(0xABCD_u16);
            packet.set_sequence_number(0);
            packet.set_payload(&payload);
            packet.set_checksum(0_u16);
            packet.set_checksum(checksum(&IcmpPacket::new(packet.packet()).unwrap()));
            for (i, b) in packet.packet().iter().enumerate() {
                buf[i].write(*b);
            }

            Ok((
                packet.packet_size(),
                "127.0.0.1:12345".parse::<SocketAddr>().unwrap().into(),
            ))
        }
    }
}
