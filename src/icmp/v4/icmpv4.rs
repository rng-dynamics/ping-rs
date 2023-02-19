use crate::PingError;
use pnet_packet::icmp::{
    echo_reply::EchoReplyPacket,
    echo_request::{
        EchoRequestPacket as EchoRequestPacketV4,
        MutableEchoRequestPacket as MutableEchoRequestPacketV4,
    },
    IcmpPacket, IcmpTypes,
};
use pnet_packet::Packet;
use rand::Rng;
use std::io;
use std::net::{IpAddr, Ipv4Addr};
use std::result::Result;
use std::time::Instant;

pub(crate) const PAYLOAD_SIZE: usize = 56;

pub(crate) struct IcmpV4 {
    payload: [u8; PAYLOAD_SIZE],
}

impl IcmpV4 {
    pub(crate) fn create() -> IcmpV4 {
        let mut payload = [0u8; PAYLOAD_SIZE];
        rand::thread_rng().fill(&mut payload[..]);
        IcmpV4 { payload }
    }

    pub(crate) fn send_one_ping<S>(
        &self,
        socket: &S,
        ipv4: Ipv4Addr,
        sequence_number: u16,
    ) -> Result<(usize, IpAddr, u16, Instant), PingError>
    where
        S: crate::icmp::v4::socket::Socket,
    {
        let ip_addr = IpAddr::V4(ipv4);
        let addr = std::net::SocketAddr::new(ip_addr, 0);

        let package = self.new_icmpv4_package(sequence_number).ok_or(PingError {
            message: "could not create ICMP package".to_owned(),
        })?;

        // TODO(as): do not use Instant::now() directly.
        let start_time: Instant = Instant::now();
        socket.send_to(pnet_packet::Packet::packet(&package), &addr.into())?;

        Ok((PAYLOAD_SIZE, ip_addr, sequence_number, start_time))
    }

    pub(crate) fn try_receive<S>(
        socket: &S,
    ) -> std::result::Result<Option<(usize, IpAddr, u16, Instant)>, io::Error>
    where
        S: crate::icmp::v4::socket::Socket,
    {
        let mut buf1 = [0u8; 256];
        match socket.recv_from(&mut buf1) {
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(e),
            Ok((n, addr, ttl)) => {
                let receive_time: Instant = Instant::now();
                let echo_reply_package =
                    EchoReplyPacket::new(&buf1).expect("could not initialize echo reply package");
                let sn = echo_reply_package.get_sequence_number();
                // TODO: use TTL
                Ok(Some((n, addr, sn, receive_time)))
            }
        }
    }

    pub(crate) fn new_icmpv4_package(
        &self,
        sequence_number: u16,
    ) -> Option<MutableEchoRequestPacketV4<'static>> {
        let buf = vec![0u8; EchoRequestPacketV4::minimum_packet_size() + PAYLOAD_SIZE];
        let mut package = MutableEchoRequestPacketV4::owned(buf)?;
        package.set_sequence_number(sequence_number);
        package.set_identifier(0);
        package.set_icmp_type(IcmpTypes::EchoRequest);
        package.set_payload(&self.payload);

        package.set_checksum(0_u16);
        let checksum = pnet_packet::icmp::checksum(&IcmpPacket::new(package.packet())?);
        package.set_checksum(checksum);
        Some(package)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icmp::v4::socket::tests::OnReceive;
    use crate::icmp::v4::socket::tests::OnSend;
    use crate::icmp::v4::socket::tests::SocketMock;

    #[test]
    fn test_send_one_ping() {
        let socket_mock = SocketMock::new(OnSend::ReturnDefault, OnReceive::ReturnWouldBlock);
        let icmpv4 = IcmpV4::create();

        let addr = Ipv4Addr::new(127, 0, 0, 1);
        let sequence_number = 1;
        let result = icmpv4.send_one_ping(&socket_mock, addr, sequence_number);

        assert!(result.is_ok());
        socket_mock
            .should_send_number_of_messages(1)
            .should_send_to_address(&IpAddr::V4(addr));
    }

    #[test]
    fn test_try_receive() {
        let socket_mock = SocketMock::new(OnSend::ReturnDefault, OnReceive::ReturnDefault(1));

        let result = IcmpV4::try_receive(&socket_mock);

        assert!(result.is_ok());
        assert!(result.as_ref().unwrap().is_some());
        let (n, addr, _sn, _receive_time) = result.unwrap().unwrap();
        assert!(n >= EchoReplyPacket::minimum_packet_size());
        assert!(addr == Ipv4Addr::new(127, 0, 0, 1));
        socket_mock.should_receive_number_of_messages(1);
    }
}