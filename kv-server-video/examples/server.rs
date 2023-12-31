use anyhow::{Ok, Result};
use futures::SinkExt;
use futures::StreamExt;

use kv::pb::RequestDelete;
use kv::pb::{request::Command, Request, RequestGet, RequestPut, Response};
use tokio::net::TcpListener;
// use tokio_util::codec::LengthDelimitedCodec;
use kv::noise_codec::{NoiseCodec, NOISE_PARAMS};
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let tree = sled::open("./welcome-to-sled").expect("open");
    let addr = "0.0.0.0:8888";
    let listener = TcpListener::bind(addr).await?;
    info!("Listening to: {}", addr);

    loop {
        let (stream, addr) = listener.accept().await?;
        info!("new client: {:?} accepted", addr);
        let shared = tree.clone();
        tokio::spawn(async move {
            // let mut stream = LengthDelimitedCodec::builder()
            //     .length_field_length(2)
            //     .new_framed(stream);
            let mut stream = NoiseCodec::builder(NOISE_PARAMS, false).new_framed(stream)?;
            let data = stream.next().await.unwrap()?;
            info!("<- e");
            stream.send(data.freeze()).await?;
            info!("-> e, ee, s, es");
            let _data = stream.next().await.unwrap()?;
            info!("<- s, se");
            stream.codec_mut().into_transport_mode()?;

            while let Some(Result::Ok(buf)) = stream.next().await {
                let msg: Request = buf.try_into()?;
                info!("Got a command: {:?}", msg);

                let response = match msg.command {
                    Some(Command::Get(RequestGet { key })) => match shared.get(&key) {
                        Result::Ok(Some(v)) => Response::new(key, v.to_vec()),
                        _ => Response::not_found(key),
                    },
                    Some(Command::Put(RequestPut { key, value })) => {
                        let old = shared.insert(key.clone(), value);
                        Response::new(key, old.unwrap_or_default().unwrap_or_default().to_vec())
                    }
                    Some(Command::Delete(RequestDelete { key })) => match shared.remove(&key) {
                        Result::Ok(Some(v)) => Response::new(key, v.to_vec()),
                        _ => Response::new(key, vec![]),
                    },
                    _ => unimplemented!(),
                };

                stream.send(response.into()).await?
            }
            Ok(())
        });
    }
}
