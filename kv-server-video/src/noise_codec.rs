use anyhow::{Ok, Result};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use snow::{HandshakeState, TransportState};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Decoder, Encoder, Framed};

pub const NOISE_PARAMS: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
const HEADER_LEN: usize = 2;
const MAX_FRAME_SIZE: usize = 65535;

pub struct Builder {
    params: &'static str,
    initiator: bool,
}

enum NoiseState {
    Handshake(HandshakeState),
    Transport(TransportState),
    None,
}

impl NoiseState {
    fn write_message(&mut self, payload: &[u8], message: &mut [u8]) -> Result<usize, snow::Error> {
        match self {
            NoiseState::Handshake(state) => state.write_message(payload, message),
            NoiseState::Transport(state) => state.write_message(payload, message),
            NoiseState::None => unreachable!(),
        }
    }
    fn read_message(&mut self, payload: &[u8], message: &mut [u8]) -> Result<usize, snow::Error> {
        match self {
            NoiseState::Handshake(state) => state.read_message(payload, message),
            NoiseState::Transport(state) => state.read_message(payload, message),
            NoiseState::None => unreachable!(),
        }
    }
}

#[allow(dead_code)]
pub struct NoiseCodec {
    builder: Builder,
    state: NoiseState,
}

impl NoiseCodec {
    pub fn builder(params: &'static str, initiator: bool) -> Builder {
        Builder::new(params, initiator)
    }

    pub fn into_transport_mode(&mut self) -> Result<()> {
        if matches!(self.state, NoiseState::Handshake(_)) {
            if let NoiseState::Handshake(s) = std::mem::replace(&mut self.state, NoiseState::None) {
                self.state = NoiseState::Transport(s.into_transport_mode()?)
            }
        }
        Ok(())
    }
}

impl Builder {
    pub fn new(params: &'static str, initiator: bool) -> Self {
        Self { params, initiator }
    }

    fn new_codec(self) -> Result<NoiseCodec> {
        let builder = snow::Builder::new(self.params.parse()?);
        let keypair = builder.generate_keypair()?;
        let builder = builder.local_private_key(&keypair.private);
        let noise = match self.initiator {
            true => builder.build_initiator()?,
            false => builder.build_responder()?,
        };
        Ok(NoiseCodec {
            builder: self,
            state: NoiseState::Handshake(noise),
        })
    }

    pub fn new_framed<T>(self, inner: T) -> Result<Framed<T, NoiseCodec>>
    where
        T: AsyncRead + AsyncWrite,
    {
        let codec = self.new_codec()?;
        Ok(Framed::new(inner, codec))
    }
}

impl Encoder<Bytes> for NoiseCodec {
    type Error = anyhow::Error;

    fn encode(&mut self, item: Bytes, dst: &mut bytes::BytesMut) -> Result<(), Self::Error> {
        let n = item.len();
        if n > MAX_FRAME_SIZE {
            return Err(anyhow::anyhow!("Invalid Input".to_string()));
        }

        let mut buf = [0u8; MAX_FRAME_SIZE];

        let n = self.state.write_message(&item, &mut buf)?;

        dst.reserve(HEADER_LEN + n);

        dst.put_uint(n as u64, HEADER_LEN);

        dst.put_slice(&buf[..n]);

        Ok(())
    }
}

impl Decoder for NoiseCodec {
    type Item = BytesMut;

    type Error = anyhow::Error;

    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.len() < HEADER_LEN {
            return Ok(None);
        }

        let len = src.get_uint(HEADER_LEN) as usize;

        if src.len() < len {
            return Ok(None);
        }

        let mut buf = [0u8; MAX_FRAME_SIZE];

        let payload = src.split_to(len);

        let n = self.state.read_message(&payload, &mut buf)?;

        let decoded = BytesMut::from(&buf[..n]);

        Ok(Some(decoded))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() -> Result<()> {
        let mut client = NoiseCodec::builder(NOISE_PARAMS, true).new_codec()?;
        let mut server = NoiseCodec::builder(NOISE_PARAMS, false).new_codec()?;

        let mut buf = BytesMut::new();

        // (client)
        // -> e
        client
            .encode(Bytes::from_static(b"hello"), &mut buf)
            .unwrap();

        let mut msg = buf.split_to(buf.len());
        // client sent msg out

        // (server)
        // <- e
        let msg = server.decode(&mut msg).unwrap().unwrap();
        // -> e, ee, s, es
        server.encode(msg.freeze(), &mut buf).unwrap();
        let mut msg = buf.split_to(buf.len());
        // server sent msg out

        // (client)
        // <- e, ee, s, es
        let msg = client.decode(&mut msg).unwrap().unwrap();
        // -> s, se
        client.encode(msg.freeze(), &mut buf).unwrap();
        let mut msg = buf.split_to(buf.len());
        // client sent msg out

        // (server)
        // <- s, se
        let msg = server.decode(&mut msg).unwrap().unwrap();
        assert_eq!(msg.freeze().as_ref(), b"hello");

        client.into_transport_mode().unwrap();
        server.into_transport_mode().unwrap();

        Ok(())
    }
}
