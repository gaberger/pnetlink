use tokio_core::reactor::{Handle, PollEvented};
use tokio_core::io::{Codec,EasyBuf};
use tokio_core::io::Io;
use futures::{Future, Poll, Async};
use std::io;
use ::socket;
use ::packet::netlink::{NetlinkPacket,MutableNetlinkPacket,NetlinkMsgFlags,self};
use ::packet::route::{IfInfoPacket,MutableIfInfoPacket};
use ::packet::route::link::{Link};
use packet::netlink::{NLM_F_ACK,NLM_F_REQUEST,NLM_F_DUMP,NLM_F_MATCH,NLM_F_EXCL,NLM_F_CREATE};
use pnet::packet::{Packet,PacketSize,FromPacket};

pub struct NetlinkSocket {
    io: PollEvented<::socket::NetlinkSocket>,
}

impl NetlinkSocket {
    pub fn bind(proto: socket::NetlinkProtocol, groups: u32, handle: &Handle) -> io::Result<NetlinkSocket> {
        let sock = try!(socket::NetlinkSocket::bind(proto, groups));
        NetlinkSocket::new(sock, handle)
    }

    fn new(socket: ::socket::NetlinkSocket, handle: &Handle) -> io::Result<NetlinkSocket> {
        let io = try!(PollEvented::new(socket, handle));
        Ok(NetlinkSocket { io: io })
    }

    /// Test whether this socket is ready to be read or not.
    pub fn poll_read(&self) -> Async<()> {
        self.io.poll_read()
    }

    /// Test whether this socket is writey to be written to or not.
    pub fn poll_write(&self) -> Async<()> {
        self.io.poll_write()
    }
}

impl io::Read for NetlinkSocket {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.io.read(buf)
    }

    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        buf.resize(4096, 0);
        let mut write_at = 0;
        loop {
            match self.read(&mut buf[write_at..]) {
                Ok(n) => {
                    write_at += n;
                },
                Err(e) => {
                    buf.truncate(write_at);
                    return Err(e);
                }
            }
        }
        buf.truncate(write_at);
        return Ok(write_at);
    }
}

impl io::Write for NetlinkSocket {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.io.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.io.flush()
    }
}

impl Io for NetlinkSocket {
    fn poll_read(&mut self) -> Async<()> {
        <NetlinkSocket>::poll_read(self)
    }

    fn poll_write(&mut self) -> Async<()> {
        <NetlinkSocket>::poll_write(self)
    }
}

struct NetlinkCodec {

}

impl Codec for NetlinkCodec {
    type In = NetlinkPacket<'static>;
    type Out = NetlinkPacket<'static>;

    fn decode_eof(&mut self, buf: &mut EasyBuf) -> io::Result<Self::In> {
        println!("DECODE EOF CALLED");

        Ok(NetlinkPacket::owned(buf.as_slice().to_owned()).unwrap())
    }

    fn decode(&mut self, buf: &mut EasyBuf) -> io::Result<Option<Self::In>> {
        let (owned_pkt, len) = {
            let slice = buf.as_slice();
            if slice.len() == 0 {
                return Ok(None);
            }
            if let Some(pkt) = NetlinkPacket::new(slice) {
                //println!("{:?} slice: {}", pkt, slice.len());
                let aligned_len = ::util::align(pkt.get_length() as usize);
                if aligned_len > slice.len() {
                    println!("NEED MORE BYTES");
                    return Ok(None);
                }
                (NetlinkPacket::owned(slice[..pkt.get_length() as usize].to_owned()), aligned_len)
            } else {
                println!("SLICE: {:?}/{}", slice, slice.len());
                unimplemented!();
            }
        };
        buf.drain_to(len as usize);
        return Ok(owned_pkt);
    }

    fn encode(&mut self, msg: Self::Out, buf: &mut Vec<u8>) -> io::Result<()> {
        let data = msg.packet();
        buf.extend_from_slice(data);
        Ok(())
    }
}

pub struct NetlinkRequestBuilder {
    data: Vec<u8>,
}

impl NetlinkRequestBuilder {
    pub fn new(kind: u16, flags: NetlinkMsgFlags) -> Self {
        let len = MutableNetlinkPacket::minimum_packet_size();
        let mut data = vec![0; len];
        {
            let mut pkt = MutableNetlinkPacket::new(&mut data).unwrap();
            pkt.set_length(len as u32);
            pkt.set_kind(kind);
            pkt.set_flags(flags | NLM_F_REQUEST);
        }
        NetlinkRequestBuilder {
            data: data,
        }
    }

    pub fn append<P: PacketSize + Packet>(mut self, data: P) -> Self {
        let data = data.packet();
        let len = data.len();
        let aligned_len = ::util::align(len as usize);
        {
            let mut pkt = MutableNetlinkPacket::new(&mut self.data).unwrap();
            let new_len = pkt.get_length() + len as u32;
            pkt.set_length(new_len as u32);
        }
        self.data.extend_from_slice(data);
        // add padding for alignment
        for _ in len..aligned_len {
            self.data.push(0);
        }
        self
    }

    pub fn build(self) -> NetlinkPacket<'static> {
        NetlinkPacket::owned(self.data).unwrap()
    }
}


#[test]
fn try_tokio_conn() {
    use tokio_core::reactor::Core;
    use tokio_core::io::Io;
    use futures::{Sink,Stream,Future};

    let mut l = Core::new().unwrap();
    let handle = l.handle();
    let sock = NetlinkSocket::bind(socket::NetlinkProtocol::Route, 0, &handle).unwrap();
    println!("Netlink socket bound");
    let framed = Io::framed(sock, NetlinkCodec {});

    let pkt = NetlinkRequestBuilder::new(18 /* RTM GETLINK */, NLM_F_DUMP).append(
        {
            let len = MutableIfInfoPacket::minimum_packet_size();
            let mut data = vec![0; len];
            MutableIfInfoPacket::owned(data).unwrap()
        }
    ).build();
    /*
    let f = framed.send(pkt).and_then(|s| 
        s.into_future().map_err(|(e, _)| {
        println!("E: {:?}", e);
        e
    } ))
    .and_then(|(frame, stream)| {
         println!("RECEIVED FRAME: {:?}", frame); Ok(stream)
    });
    */
    let f = framed.send(pkt).and_then(|stream|
        stream.for_each(|frame| {
            println!("RECEIVED FRAME: {:?}", frame);
            if frame.get_kind() == 16 /* NEW LINK */ {
                Link::dump_link(frame);
            }
            Ok(())
        })
    );
    let s = l.run(f);
}

#[test]
fn try_mio_conn() {
    use mio::*;

    let poll = Poll::new().unwrap();
    let mut sock = socket::NetlinkSocket::bind(socket::NetlinkProtocol::Route, 0).unwrap();
    poll.register(&sock, Token(0), Ready::writable() | Ready::readable(),
              PollOpt::edge()).unwrap();

    let pkt = NetlinkRequestBuilder::new(18 /* RTM GETLINK */, NLM_F_DUMP).append(
        {
            let len = MutableIfInfoPacket::minimum_packet_size();
            let mut data = vec![0; len];
            MutableIfInfoPacket::owned(data).unwrap()
        }
    ).build();

    let mut buf = vec![0;4096];
    let mut pos: usize = 0;
    let mut events = Events::with_capacity(1024);
    let mut written = false;
    loop {
        poll.poll(&mut events, None).unwrap();
        for event in events.iter() {
            match event.token() {
                Token(0) => {
                    println!("EVENT: {:?}", event);
                    if event.kind() == Ready::writable() {
                        use std::io::Write;
                        if !written {
                            println!("WRITABLE");
                            sock.write(pkt.packet()).unwrap();
                            written = true;
                        }
                    }
                    if event.kind() & Ready::readable() == Ready::readable() {
                        use std::io::Read;
                        println!("Reading");
                        'read: loop {
                            match sock.read(&mut buf[pos..]) {
                                Ok(n) => {
                                    if n == 0 {
                                        break 'read;
                                    }
                                    pos += n;
                                    println!("read {}", n);
                                    if pos >= buf.len() - 1 {
                                        println!("Growing buf: len: {} pos: {}", buf.len(), pos);
                                        for _ in 0..buf.len() {
                                            buf.push(0);
                                        }
                                        println!("Growing buf: new len: {} pos: {}", buf.len(), pos);
                                    }
                                },
                                Err(e) => {
                                     println!("err: {:?}", e);
                                     break 'read;
                                },
                            }
                        }
                        if let Some(pkt) = NetlinkPacket::new(&buf) {
                            println!("PKT: {:?}", pkt);
                            let mut cursor = 0;
                            let total_len = buf.len();

                            let mut aligned_len = ::util::align(pkt.get_length() as usize);
                            loop {
                                cursor += aligned_len;
                                if cursor >= total_len {
                                    break;
                                }
                                println!("NEXT PKT @ {:?}", cursor);
                                if let Some(next_pkt) = NetlinkPacket::new(&buf[cursor..]) {
                                    println!("PKT: {:?}", next_pkt);
                                    aligned_len = ::util::align(next_pkt.get_length() as usize);
                                    if aligned_len == 0 {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                },
                _ => {},
            }
        }
    }
}